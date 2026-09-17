use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use reqwest::{Client, Url};

// SSRF guard for user-controlled upstream URLs.
//
// Any authenticated user can supply an `X-Upstream-Url` header on chat /
// models / image proxy endpoints. Without a guard that URL becomes a generic
// HTTP(S) GET/POST primitive targeting whatever network our server can reach
// — AWS/GCP metadata, loopback admin panels, RFC1918 neighbours, Kubernetes
// API servers, etc. This module enforces a host-level allow-public-only
// policy and pins the outgoing connection to the IPs we validated, so DNS
// rebinding between the check and the connect can't pivot into the private
// network.
//
// Admin-configured shared upstreams are *not* routed through this guard —
// the admin is already privileged and can intentionally point at a local
// LLM. Only user-supplied URLs need it.
//
// One address range is deliberately *not* treated as private: the DNS
// fake-ip pool `198.18.0.0/15`. When the host resolves through a transparent
// proxy (mihomo / sing-box / Clash in fake-ip mode), every public hostname
// answers with a synthetic address from that pool, and the real destination
// is resolved by the tunnel at connect time. Rejecting it would block every
// legitimate public upstream — the observed symptom being "DNS 解析到私网 /
// 回环地址" for an ordinary relay domain — while blocking nothing, because a
// fake-ip address is a local placeholder rather than a reachable LAN host.
// Genuinely internal hostnames still resolve to RFC1918 / loopback addresses
// and stay blocked.

/// Synthetic addresses handed out by a fake-ip resolver. Reserved for
/// benchmarking by RFC 2544, never routed on the real Internet, which is why
/// proxies picked it for their fake-ip pool.
pub fn is_dns_fake_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            o[0] == 198 && (o[1] == 18 || o[1] == 19)
        }
        IpAddr::V6(v) => v.to_ipv4_mapped().is_some_and(|v4| {
            let o = v4.octets();
            o[0] == 198 && (o[1] == 18 || o[1] == 19)
        }),
    }
}

pub fn is_private_ip(ip: IpAddr) -> bool {
    if is_dns_fake_ip(ip) {
        return false;
    }
    match ip {
        IpAddr::V4(v) => is_private_ipv4(v),
        IpAddr::V6(v) => is_private_ipv6(v),
    }
}

fn is_private_ipv4(v: Ipv4Addr) -> bool {
    if v.is_loopback() || v.is_unspecified() || v.is_broadcast() || v.is_multicast() {
        return true;
    }
    if v.is_private() || v.is_link_local() || v.is_documentation() {
        return true;
    }
    let o = v.octets();
    if o[0] == 0 {
        return true;
    }
    if o[0] == 100 && (o[1] & 0b1100_0000) == 0b0100_0000 {
        return true;
    }
    if o[0] == 192 && o[1] == 0 && o[2] == 0 {
        return true;
    }
    if o[0] >= 240 {
        return true;
    }
    false
}

fn is_private_ipv6(v: Ipv6Addr) -> bool {
    if v.is_loopback() || v.is_unspecified() || v.is_multicast() {
        return true;
    }
    if v.is_unique_local() {
        return true;
    }
    let seg = v.segments();
    if (seg[0] & 0xffc0) == 0xfe80 {
        return true;
    }
    if (seg[0] & 0xffc0) == 0xfec0 {
        return true;
    }
    if let Some(v4) = v.to_ipv4_mapped() {
        return is_private_ipv4(v4);
    }
    false
}

fn bad(msg: impl Into<String>) -> Response {
    (
        StatusCode::BAD_REQUEST,
        format!("上游 URL 被拒绝：{}", msg.into()),
    )
        .into_response()
}

pub async fn validate_upstream_url(raw: &str) -> Result<(Url, String, Vec<SocketAddr>), Response> {
    let url = Url::parse(raw).map_err(|e| bad(format!("无法解析 URL（{e}）")))?;
    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(bad(format!("不支持的协议 {scheme}，仅允许 http / https")));
    }
    let host = url
        .host_str()
        .ok_or_else(|| bad("URL 缺少 host"))?
        .to_string();
    let port = url
        .port_or_known_default()
        .ok_or_else(|| bad("URL 缺少端口"))?;

    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_ip(ip) {
            return Err(bad("禁止请求私网 / 回环 / 保留地址"));
        }
        return Ok((url, host, vec![SocketAddr::new(ip, port)]));
    }

    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|e| bad(format!("DNS 解析失败：{e}")))?
        .collect();
    if addrs.is_empty() {
        return Err(bad("DNS 未返回任何地址"));
    }
    for a in &addrs {
        if is_private_ip(a.ip()) {
            return Err(bad(format!(
                "DNS 解析到私网 / 回环地址（{host} → {}）",
                a.ip()
            )));
        }
    }
    Ok((url, host, addrs))
}

pub fn guarded_client(host: &str, addrs: &[SocketAddr]) -> Result<Client, Response> {
    guarded_client_with_timeout(host, addrs, Duration::from_secs(180))
}

pub fn guarded_client_with_timeout(
    host: &str,
    addrs: &[SocketAddr],
    timeout: Duration,
) -> Result<Client, Response> {
    reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(host, addrs)
        .build()
        .map_err(|e| bad(format!("HTTP 客户端构建失败：{e}")))
}

pub async fn client_for_upstream(
    shared_client: &Client,
    url: &str,
    is_channel: bool,
) -> Result<Client, Response> {
    if is_channel {
        return Ok(shared_client.clone());
    }
    let (_parsed, host, addrs) = validate_upstream_url(url).await?;
    guarded_client(&host, &addrs)
}

pub async fn client_for_upstream_with_timeout(
    shared_client: &Client,
    url: &str,
    is_channel: bool,
    timeout: Duration,
) -> Result<Client, Response> {
    if is_channel {
        return Ok(shared_client.clone());
    }
    let (_parsed, host, addrs) = validate_upstream_url(url).await?;
    guarded_client_with_timeout(&host, &addrs, timeout)
}

/// Build a client for the optional local video test mode. Private addresses
/// stay blocked by default because the URL is user-controlled; self-hosted
/// installations can explicitly opt in when their ComfyUI/OpenAI-compatible
/// service runs on the same machine or LAN.
pub async fn client_for_video_upstream(
    shared_client: &Client,
    url: &str,
    timeout: Duration,
) -> Result<Client, Response> {
    if crate::runtime_env::var("YUNOVA_ALLOW_PRIVATE_VIDEO_UPSTREAM")
        .ok()
        .as_deref()
        == Some("1")
    {
        return Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| bad(format!("HTTP 客户端构建失败：{e}")));
    }
    client_for_upstream_with_timeout(shared_client, url, false, timeout).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn fake_ip_answers_are_not_treated_as_private() {
        // A transparent-proxy resolver answers every public hostname from its
        // fake-ip pool, so rejecting the pool blocks all real upstreams.
        for addr in [
            "198.18.0.130",
            "198.18.2.221",
            "198.19.255.255",
            "::ffff:198.18.1.208",
        ] {
            assert!(is_dns_fake_ip(ip(addr)), "{addr} should be a fake-ip");
            assert!(!is_private_ip(ip(addr)), "{addr} must stay reachable");
        }
    }

    #[test]
    fn real_internal_addresses_stay_blocked() {
        for addr in [
            "127.0.0.1",
            "10.1.51.1",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "192.0.0.1",
            "0.0.0.0",
            "240.0.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:10.1.0.1",
        ] {
            assert!(is_private_ip(ip(addr)), "{addr} must stay blocked");
            assert!(!is_dns_fake_ip(ip(addr)), "{addr} is not a fake-ip");
        }
    }

    #[test]
    fn public_addresses_are_allowed() {
        for addr in ["1.1.1.1", "114.66.55.93", "2606:4700:4700::1111"] {
            assert!(!is_private_ip(ip(addr)), "{addr} must be allowed");
        }
    }

    #[tokio::test]
    async fn literal_hosts_are_validated_without_dns() {
        assert!(validate_upstream_url("ftp://example.com").await.is_err());
        assert!(
            validate_upstream_url("http://127.0.0.1:3000")
                .await
                .is_err()
        );
        let (_url, host, addrs) = validate_upstream_url("https://198.18.2.221/api/pricing")
            .await
            .expect("a fake-ip literal is a proxy placeholder, not a LAN host");
        assert_eq!(host, "198.18.2.221");
        assert_eq!(addrs, vec!["198.18.2.221:443".parse().unwrap()]);
    }
}
