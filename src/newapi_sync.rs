//! NewAPI / One-API pricing import.
//!
//! Relay gateways built on New API expose an anonymous `GET /api/pricing`
//! catalog. Importing it removes the busywork of transcribing every model's
//! published rate by hand and keeps Yunova's prices in step with the upstream
//! the operator actually buys from.
//!
//! ## Ratio convention
//!
//! New API stores prices as *ratios* rather than currency, anchored by a
//! constant documented in its source as `1 === $0.002 / 1K tokens`:
//!
//! ```text
//! input  $/1M = model_ratio * 2
//! output $/1M = model_ratio * completion_ratio * 2
//! cached $/1M = input $/1M * cache_ratio
//! ```
//!
//! `quota_type = 1` switches a model to per-call billing, where `model_price`
//! is already a plain USD amount and the ratios are unused.
//!
//! Everything is converted to the micro-USD integers [`crate::quota`] bills
//! with, so an imported price is indistinguishable from a hand-entered one.

use axum::{
    Extension, Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::{
    AppState, InstalledState,
    channels::{self, PricingInput},
    db::{DbKind, Pool},
    net_guard,
    quota::MICRO_USD,
};

/// New API's anchor: a ratio of 1.0 bills $0.002 per 1K tokens, i.e. $2.00
/// per 1M tokens. Multiplying a ratio by this yields micro-USD per 1M tokens.
const MICRO_USD_PER_RATIO_UNIT: f64 = 2_000_000.0;

/// Upper bound on the catalog we will parse. A relay listing more models than
/// this is either misconfigured or hostile; refuse rather than allocate.
const MAX_MODELS: usize = 5_000;

// ---------------------------------------------------------------------------
// upstream payload
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct NewApiPricingResponse {
    #[serde(default)]
    pub data: Vec<NewApiModel>,
}

/// One entry of New API's `/api/pricing` catalog. Unknown fields are ignored
/// so a gateway upgrade cannot break the import.
#[derive(Debug, Clone, Deserialize)]
pub struct NewApiModel {
    pub model_name: String,
    /// 0 = token-metered (use the ratios), 1 = per-call (use `model_price`).
    #[serde(default)]
    pub quota_type: i64,
    #[serde(default)]
    pub model_ratio: f64,
    /// Output multiplier relative to `model_ratio`.
    #[serde(default)]
    pub completion_ratio: f64,
    /// Discount applied to cached input tokens; absent when unsupported.
    #[serde(default)]
    pub cache_ratio: Option<f64>,
    /// Flat USD price per call when `quota_type == 1`.
    #[serde(default)]
    pub model_price: f64,
    #[serde(default)]
    pub supported_endpoint_types: Vec<String>,
    #[serde(default)]
    pub enable_groups: Vec<String>,
}

/// Price fields derived from one upstream entry, in micro-USD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DerivedPrice {
    pub kind: Kind,
    pub input_price: i64,
    pub output_price: i64,
    pub cached_input_price: Option<i64>,
    pub per_call_price: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Chat,
    Image,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Chat => "chat",
            Kind::Image => "image",
        }
    }
}

/// Why a model could not be imported. New API supports billing modes Yunova
/// has no equivalent for; importing those anyway would store a zero price and
/// silently give the model away, so they are reported instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unmappable {
    /// Per-call billing on a non-image model. Yunova bills chat strictly by
    /// token and never reads `per_call_price` on a chat row.
    PerCallChat,
    /// Token-priced image model. Yunova bills image generation per call, and
    /// the upstream gives no per-call figure to convert.
    TokenPricedImage,
}

impl Unmappable {
    fn reason(self, model: &str) -> String {
        match self {
            Unmappable::PerCallChat => format!(
                "{model}：上游按次计费（非图像模型），本站对话仅支持按 token 计费，已跳过；如需开放请手动添加"
            ),
            Unmappable::TokenPricedImage => format!(
                "{model}：上游按 token 计费的图像模型，本站图像按次计费，无法换算，已跳过；如需开放请手动填写每次价格"
            ),
        }
    }
}

/// Round a USD amount to micro-USD. Ratios arrive as f64, so this is the one
/// place floating point enters; rounding here keeps every stored price an
/// exact integer that later arithmetic can rely on.
fn to_micro_usd(usd: f64) -> i64 {
    if !usd.is_finite() || usd <= 0.0 {
        return 0;
    }
    (usd * MICRO_USD as f64).round().max(0.0) as i64
}

fn ratio_to_micro_usd_per_1m(ratio: f64) -> i64 {
    if !ratio.is_finite() || ratio <= 0.0 {
        return 0;
    }
    (ratio * MICRO_USD_PER_RATIO_UNIT).round().max(0.0) as i64
}

/// Whether the gateway presents this model as an image generator.
///
/// `supported_endpoint_types` is authoritative when it names an image
/// endpoint, but several gateways tag image models as plain `openai` and only
/// the name distinguishes them — so a name hint is used as a fallback. This
/// only ever routes a model between Yunova's `chat` and `image` kinds; a
/// mistake here cannot make something free, because an unmappable combination
/// is skipped rather than imported at zero.
pub fn looks_like_image(m: &NewApiModel) -> bool {
    if m
        .supported_endpoint_types
        .iter()
        .any(|t| t.contains("image"))
    {
        return true;
    }
    let name = m.model_name.to_ascii_lowercase();
    name.contains("image") || name.contains("-img") || name.contains("dall-e")
}

/// Convert one upstream entry into Yunova's micro-USD price fields, or report
/// why its billing mode has no Yunova equivalent.
pub fn derive_price(m: &NewApiModel) -> Result<DerivedPrice, Unmappable> {
    let is_image = looks_like_image(m);

    // Per-call billing: `model_price` is already USD and the ratios are unset.
    if m.quota_type == 1 {
        if !is_image {
            // Yunova's chat path bills by token only. Storing this as a chat
            // row would make a $1-per-call model completely free.
            return Err(Unmappable::PerCallChat);
        }
        return Ok(DerivedPrice {
            kind: Kind::Image,
            input_price: 0,
            output_price: 0,
            cached_input_price: None,
            per_call_price: to_micro_usd(m.model_price),
        });
    }

    // Token-metered upstream. An image model priced this way has no per-call
    // figure to convert, and `model_price` is 0 — importing it would give
    // image generation away.
    if is_image {
        let per_call = to_micro_usd(m.model_price);
        if per_call <= 0 {
            return Err(Unmappable::TokenPricedImage);
        }
        return Ok(DerivedPrice {
            kind: Kind::Image,
            input_price: 0,
            output_price: 0,
            cached_input_price: None,
            per_call_price: per_call,
        });
    }

    let input_price = ratio_to_micro_usd_per_1m(m.model_ratio);
    let output_price = if m.completion_ratio > 0.0 {
        ratio_to_micro_usd_per_1m(m.model_ratio * m.completion_ratio)
    } else {
        // No completion multiplier means output bills at the input rate.
        input_price
    };
    // A cache ratio of 0 would make cached tokens free, which no provider
    // offers; treat it as "no discount configured" and fall back to the
    // input rate at billing time.
    let cached_input_price = m
        .cache_ratio
        .filter(|r| r.is_finite() && *r > 0.0)
        .map(|r| ratio_to_micro_usd_per_1m(m.model_ratio * r))
        .filter(|v| *v > 0);

    Ok(DerivedPrice {
        kind: Kind::Chat,
        input_price,
        output_price,
        cached_input_price,
        per_call_price: 0,
    })
}

// ---------------------------------------------------------------------------
// request / response
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SyncRequest {
    /// Base URL of the New API gateway, for example `https://relay.example.com`.
    pub base_url: String,
    /// Protocol recorded for imported models. Defaults to `openai` since New
    /// API exposes an OpenAI-compatible surface for everything it serves.
    #[serde(default = "default_protocol")]
    pub protocol: String,
    /// Bind imported models to these channels. Empty leaves bindings alone.
    #[serde(default)]
    pub channel_ids: Option<Vec<i64>>,
    /// Import only models offered to this New API group (for example "Claude").
    #[serde(default)]
    pub group: Option<String>,
    /// Preview without writing. The response is identical either way, so the
    /// admin can review exactly what a real run would store.
    #[serde(default)]
    pub dry_run: bool,
    /// Import models already present in `model_pricing`. Off by default so a
    /// resync cannot silently overwrite a hand-tuned price.
    #[serde(default)]
    pub overwrite_existing: bool,
    /// Import disabled-by-default instead of live. Off by default: a freshly
    /// imported catalog should not start billing until it is reviewed.
    #[serde(default)]
    pub enable_imported: bool,
}

fn default_protocol() -> String {
    "openai".to_string()
}

#[derive(Debug, Serialize)]
pub struct SyncedModel {
    pub model: String,
    pub kind: String,
    pub input_price: i64,
    pub output_price: i64,
    pub cached_input_price: Option<i64>,
    pub per_call_price: i64,
    /// True when the model already existed in `model_pricing`.
    pub existed: bool,
    /// False when `existed` and `overwrite_existing` was not set.
    pub applied: bool,
}

#[derive(Debug, Serialize)]
pub struct SyncResponse {
    pub dry_run: bool,
    /// Models listed by the upstream catalog.
    pub fetched: usize,
    pub imported: usize,
    pub updated: usize,
    pub skipped_existing: usize,
    /// Models whose upstream billing mode has no Yunova equivalent. Reported
    /// rather than imported, because importing them would store a zero price.
    pub skipped_unsupported: usize,
    pub models: Vec<SyncedModel>,
    /// Non-fatal problems, for example a model whose price could not be read.
    pub warnings: Vec<String>,
}

fn err(s: StatusCode, m: impl Into<String>) -> Response {
    (s, Json(serde_json::json!({ "error": m.into() }))).into_response()
}

/// `GET {base_url}/api/pricing`, normalized so the admin may paste either the
/// gateway root or the full endpoint.
fn pricing_endpoint(base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    if base.ends_with("/api/pricing") {
        base.to_string()
    } else {
        format!("{base}/api/pricing")
    }
}

pub async fn admin_sync_pricing(
    axum::extract::State(state): axum::extract::State<AppState>,
    Extension(installed): Extension<InstalledState>,
    Json(req): Json<SyncRequest>,
) -> Response {
    if let Err(e) = channels::validate_protocol_public(&req.protocol) {
        return err(StatusCode::BAD_REQUEST, e);
    }
    let endpoint = pricing_endpoint(&req.base_url);
    if endpoint.len() > 512 {
        return err(StatusCode::BAD_REQUEST, "Base URL 过长");
    }

    // Admin-supplied URL: route through the SSRF guard so this endpoint cannot
    // be used to probe the internal network.
    let client = match net_guard::client_for_upstream(&state.http, &endpoint, false).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let resp = match client
        .get(&endpoint)
        .header("accept", "application/json")
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return err(StatusCode::BAD_GATEWAY, "无法连接到上游站点，请检查 Base URL"),
    };
    if !resp.status().is_success() {
        let code = resp.status();
        return err(
            StatusCode::BAD_GATEWAY,
            format!("上游返回 {code}；请确认该站点是 NewAPI 且 /api/pricing 允许匿名访问"),
        );
    }
    let body = match resp.text().await {
        Ok(t) => t,
        Err(_) => return err(StatusCode::BAD_GATEWAY, "读取上游响应失败"),
    };
    let parsed: NewApiPricingResponse = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            return err(
                StatusCode::BAD_GATEWAY,
                format!("上游返回的不是 NewAPI 定价格式：{e}"),
            );
        }
    };
    if parsed.data.len() > MAX_MODELS {
        return err(
            StatusCode::BAD_GATEWAY,
            format!("上游返回了 {} 个模型，超出上限 {MAX_MODELS}", parsed.data.len()),
        );
    }

    match apply_sync(&installed.pool, installed.kind, &req, parsed.data).await {
        Ok(r) => Json(r).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Filter, price and persist the fetched catalog. Split out from the handler
/// so the import rules are testable without a live gateway.
async fn apply_sync(
    pool: &Pool,
    kind: DbKind,
    req: &SyncRequest,
    models: Vec<NewApiModel>,
) -> Result<SyncResponse, sqlx::Error> {
    let existing: std::collections::HashSet<String> = channels::list_pricing(pool, kind)
        .await?
        .into_iter()
        .map(|p| p.model)
        .collect();

    let fetched = models.len();
    let mut out = SyncResponse {
        dry_run: req.dry_run,
        fetched,
        imported: 0,
        updated: 0,
        skipped_existing: 0,
        skipped_unsupported: 0,
        models: Vec::new(),
        warnings: Vec::new(),
    };

    for m in models {
        let name = m.model_name.trim().to_string();
        if name.is_empty() {
            continue;
        }
        if let Some(group) = req.group.as_deref().filter(|g| !g.is_empty()) {
            if !m.enable_groups.iter().any(|g| g == group) {
                continue;
            }
        }

        let price = match derive_price(&m) {
            Ok(p) => p,
            Err(reason) => {
                out.skipped_unsupported += 1;
                out.warnings.push(reason.reason(&name));
                continue;
            }
        };
        let existed = existing.contains(&name);
        if existed && !req.overwrite_existing {
            out.skipped_existing += 1;
            out.models.push(SyncedModel {
                model: name,
                kind: price.kind.as_str().to_string(),
                input_price: price.input_price,
                output_price: price.output_price,
                cached_input_price: price.cached_input_price,
                per_call_price: price.per_call_price,
                existed,
                applied: false,
            });
            continue;
        }

        // A model with no price at all would import as free and be billed as
        // free. Flag it instead of silently giving usage away.
        let priced = price.input_price > 0 || price.output_price > 0 || price.per_call_price > 0;
        if !priced {
            out.warnings
                .push(format!("{name}：上游价格为 0，已按免费导入"));
        }

        if !req.dry_run {
            let input = PricingInput {
                model: name.clone(),
                kind: price.kind.as_str().to_string(),
                channel_ids: req.channel_ids.clone(),
                display_name: None,
                enabled: req.enable_imported,
                protocol: req.protocol.clone(),
                context_limit: None,
                input_price: price.input_price,
                output_price: price.output_price,
                cached_input_price: price.cached_input_price,
                per_call_price: price.per_call_price,
                base_price: 0,
                per_second_price: 0,
                allowed_seconds: None,
                size_rules: None,
            };
            channels::upsert_price(pool, kind, &input).await?;
        }

        if existed {
            out.updated += 1;
        } else {
            out.imported += 1;
        }
        out.models.push(SyncedModel {
            model: name,
            kind: price.kind.as_str().to_string(),
            input_price: price.input_price,
            output_price: price.output_price,
            cached_input_price: price.cached_input_price,
            per_call_price: price.per_call_price,
            existed,
            applied: true,
        });
    }

    out.models.sort_by(|a, b| a.model.cmp(&b.model));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(name: &str) -> NewApiModel {
        NewApiModel {
            model_name: name.to_string(),
            quota_type: 0,
            model_ratio: 1.0,
            completion_ratio: 1.0,
            cache_ratio: None,
            model_price: 0.0,
            supported_endpoint_types: vec!["openai".into()],
            enable_groups: vec!["default".into()],
        }
    }

    fn priced(m: &NewApiModel) -> DerivedPrice {
        derive_price(m).expect("model should be mappable")
    }

    #[test]
    fn ratio_one_is_two_dollars_per_million_tokens() {
        // New API anchors ratios at "1 === $0.002 / 1K tokens".
        let p = priced(&model("m"));
        assert_eq!(p.input_price, 2_000_000);
        assert_eq!(p.output_price, 2_000_000);
    }

    #[test]
    fn completion_ratio_scales_only_the_output_price() {
        // claude-sonnet-5 on a live gateway: ratio 1, completion 5, cache 0.1
        // → $2/1M in, $10/1M out, $0.20/1M cached.
        let mut m = model("claude-sonnet-5");
        m.completion_ratio = 5.0;
        m.cache_ratio = Some(0.1);
        let p = priced(&m);
        assert_eq!(p.input_price, 2_000_000);
        assert_eq!(p.output_price, 10_000_000);
        assert_eq!(p.cached_input_price, Some(200_000));
    }

    #[test]
    fn a_fractional_ratio_survives_rounding_to_micro_usd() {
        // MiniMax-M3: ratio 0.15 → $0.30/1M in, completion 4 → $1.20/1M out.
        let mut m = model("MiniMax-M3");
        m.model_ratio = 0.15;
        m.completion_ratio = 4.0;
        let p = priced(&m);
        assert_eq!(p.input_price, 300_000);
        assert_eq!(p.output_price, 1_200_000);
    }

    #[test]
    fn a_large_ratio_converts_without_precision_loss() {
        // grok-chat-fast: ratio 37.5 → $75/1M.
        let mut m = model("grok-chat-fast");
        m.model_ratio = 37.5;
        m.completion_ratio = 2.0;
        let p = priced(&m);
        assert_eq!(p.input_price, 75_000_000);
        assert_eq!(p.output_price, 150_000_000);
    }

    #[test]
    fn a_per_call_image_model_uses_model_price_directly() {
        // grok-imagine-image: quota_type 1, $0.05 per call, tagged only
        // "openai" — the name is what identifies it as an image model.
        let mut m = model("grok-imagine-image");
        m.quota_type = 1;
        m.model_ratio = 0.0;
        m.model_price = 0.05;
        let p = priced(&m);
        assert_eq!(p.kind, Kind::Image);
        assert_eq!(p.per_call_price, 50_000);
        // Token rates stay zero so the call is not billed twice.
        assert_eq!(p.input_price, 0);
        assert_eq!(p.output_price, 0);
    }

    /// Regression: a live gateway bills `claude-sonnet-4-8` at $1 per call.
    /// Yunova's chat path reads only token rates, so importing this as a chat
    /// row would serve a $1 model for free. It must be skipped instead.
    #[test]
    fn a_per_call_chat_model_is_skipped_rather_than_imported_free() {
        let mut m = model("claude-sonnet-4-8");
        m.quota_type = 1;
        m.model_ratio = 0.0;
        m.model_price = 1.0;
        m.supported_endpoint_types = vec!["anthropic".into(), "openai".into()];
        assert_eq!(derive_price(&m), Err(Unmappable::PerCallChat));
    }

    /// Regression: `gpt-image-1` is priced per token upstream but Yunova bills
    /// images per call, and the upstream `model_price` is 0. Importing it
    /// would give image generation away.
    #[test]
    fn a_token_priced_image_model_is_skipped_rather_than_imported_free() {
        let mut m = model("gpt-image-1");
        m.model_ratio = 2.5;
        m.completion_ratio = 8.0;
        m.model_price = 0.0;
        m.supported_endpoint_types = vec!["image-generation".into(), "openai".into()];
        assert_eq!(derive_price(&m), Err(Unmappable::TokenPricedImage));
    }

    #[test]
    fn image_models_are_detected_by_endpoint_or_name() {
        let mut by_endpoint = model("some-generator");
        by_endpoint.supported_endpoint_types = vec!["image-generation".into(), "openai".into()];
        assert!(looks_like_image(&by_endpoint));

        // Gateways that tag image models as plain "openai" are common.
        assert!(looks_like_image(&model("grok-imagine-image")));
        assert!(looks_like_image(&model("dall-e-3")));

        let mut chat = model("claude-sonnet-5");
        chat.supported_endpoint_types = vec!["anthropic".into(), "openai".into()];
        assert!(!looks_like_image(&chat));
    }

    #[test]
    fn a_missing_completion_ratio_bills_output_at_the_input_rate() {
        let mut m = model("weird");
        m.completion_ratio = 0.0;
        let p = priced(&m);
        assert_eq!(p.output_price, p.input_price);
    }

    #[test]
    fn a_zero_cache_ratio_is_treated_as_no_discount_rather_than_free() {
        let mut m = model("m");
        m.cache_ratio = Some(0.0);
        assert_eq!(priced(&m).cached_input_price, None);
    }

    #[test]
    fn negative_or_nonfinite_ratios_never_produce_negative_prices() {
        let mut m = model("hostile");
        m.model_ratio = -5.0;
        assert_eq!(priced(&m).input_price, 0);

        m.model_ratio = f64::NAN;
        assert_eq!(priced(&m).input_price, 0);

        m.model_ratio = f64::INFINITY;
        assert_eq!(priced(&m).input_price, 0);
    }

    #[test]
    fn the_endpoint_accepts_either_a_root_url_or_the_full_path() {
        assert_eq!(
            pricing_endpoint("https://relay.example.com"),
            "https://relay.example.com/api/pricing"
        );
        assert_eq!(
            pricing_endpoint("https://relay.example.com/"),
            "https://relay.example.com/api/pricing"
        );
        assert_eq!(
            pricing_endpoint("https://relay.example.com/api/pricing"),
            "https://relay.example.com/api/pricing"
        );
    }

    #[test]
    fn unknown_upstream_fields_do_not_break_parsing() {
        // A gateway upgrade that adds fields must not fail the import.
        let raw = r#"{"data":[{"model_name":"m","quota_type":0,"model_ratio":1.5,
                     "completion_ratio":2,"brand_new_field":{"x":1}}],"extra":true}"#;
        let parsed: NewApiPricingResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.data.len(), 1);
        assert_eq!(priced(&parsed.data[0]).input_price, 3_000_000);
    }
}
