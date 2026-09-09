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

pub fn is_private_ip(ip: IpAddr) -> bool {
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
    if o[0] == 198 && (o[1] == 18 || o[1] == 19) {
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
            return Err(bad("DNS 解析到私网 / 回环地址"));
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
