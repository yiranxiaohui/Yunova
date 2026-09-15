//! Token-usage extraction from upstream chat responses.
//!
//! Chat billing happens *after* the response, so the proxy must recover the
//! provider's own token counts. Each protocol reports them differently, and
//! third-party relays mix styles, so the parsers below accept every shape we
//! have observed and keep the last non-zero value seen.
//!
//! Precision matters: an undercount silently gives away quota, and an
//! overcount overcharges a user. When no usage can be found at all the caller
//! must treat the request as unbilled rather than guessing.

use crate::quota::TokenUsage;

/// Accumulates usage across the frames of one streamed response.
///
/// Providers differ in whether usage arrives once at the end (OpenAI
/// Responses, Gemini) or is split across a start frame carrying input tokens
/// and delta frames carrying a running output total (Anthropic). Tracking the
/// maximum of each counter handles both without double counting, because every
/// provider reports cumulative — never incremental — totals.
#[derive(Debug, Default, Clone, Copy)]
pub struct UsageAccumulator {
    usage: TokenUsage,
}

impl UsageAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn usage(&self) -> TokenUsage {
        self.usage
    }

    fn merge(&mut self, other: TokenUsage) {
        self.usage.input = self.usage.input.max(other.input);
        self.usage.output = self.usage.output.max(other.output);
        self.usage.cached_input = self.usage.cached_input.max(other.cached_input);
    }

    /// Feed one decoded JSON value from the stream (an SSE `data:` payload or
    /// a whole non-streamed body).
    pub fn ingest_json(&mut self, v: &serde_json::Value) {
        if let Some(u) = extract_usage(v) {
            self.merge(u);
        }
    }

    /// Feed a raw SSE chunk. Only `data:` lines are parsed; `[DONE]` and
    /// comments/heartbeats are ignored. Safe to call on partial frames — a
    /// line that does not parse as JSON is skipped.
    pub fn ingest_sse_chunk(&mut self, chunk: &str) {
        for line in chunk.lines() {
            let line = line.trim_start();
            let Some(payload) = line.strip_prefix("data:") else {
                continue;
            };
            let payload = payload.trim();
            if payload.is_empty() || payload == "[DONE]" {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(payload) {
                self.ingest_json(&v);
            }
        }
    }
}

fn as_i64(v: Option<&serde_json::Value>) -> i64 {
    v.and_then(|x| x.as_i64()).unwrap_or(0).max(0)
}

/// Pull token counts out of one JSON value, trying every known layout.
/// Returns None when the value carries no usage information.
pub fn extract_usage(v: &serde_json::Value) -> Option<TokenUsage> {
    // Gemini: { usageMetadata: { promptTokenCount, candidatesTokenCount,
    //                            cachedContentTokenCount, thoughtsTokenCount } }
    if let Some(um) = v.get("usageMetadata") {
        let input = as_i64(um.get("promptTokenCount"));
        // Thinking tokens are billed as output by Google but reported apart.
        let output =
            as_i64(um.get("candidatesTokenCount")) + as_i64(um.get("thoughtsTokenCount"));
        let cached = as_i64(um.get("cachedContentTokenCount"));
        if input > 0 || output > 0 {
            return Some(TokenUsage { input, output, cached_input: cached });
        }
    }

    // OpenAI Responses: the completion event nests the final usage under
    // `response`. Anthropic's message_start nests it under `message`.
    for key in ["response", "message"] {
        if let Some(inner) = v.get(key) {
            if let Some(u) = inner.get("usage").and_then(parse_usage_object) {
                return Some(u);
            }
        }
    }

    // Top-level `usage`: OpenAI chat-completions, Anthropic message_delta,
    // and most relay implementations.
    if let Some(u) = v.get("usage").and_then(parse_usage_object) {
        return Some(u);
    }

    None
}

/// Parse a `usage` object in either OpenAI (`prompt_tokens`/`completion_tokens`)
/// or Anthropic (`input_tokens`/`output_tokens`) naming.
fn parse_usage_object(u: &serde_json::Value) -> Option<TokenUsage> {
    if !u.is_object() {
        return None;
    }

    // Anthropic cache fields: cache_read_input_tokens are billed at the
    // discounted cached rate; cache_creation_input_tokens are billed at (or
    // above) the normal input rate, so they stay in the plain input bucket.
    let anthropic_cache_read = as_i64(u.get("cache_read_input_tokens"));
    let anthropic_cache_write = as_i64(u.get("cache_creation_input_tokens"));

    // OpenAI reports cached tokens inside a details object.
    let openai_cached = u
        .get("prompt_tokens_details")
        .or_else(|| u.get("input_tokens_details"))
        .map(|d| as_i64(d.get("cached_tokens")))
        .unwrap_or(0);

    let mut input = as_i64(u.get("input_tokens")).max(as_i64(u.get("prompt_tokens")));
    let output = as_i64(u.get("output_tokens")).max(as_i64(u.get("completion_tokens")));

    // Anthropic's input_tokens excludes cached reads/writes; add them back so
    // `input` is always the full prompt size and `cached_input` a subset of it.
    if anthropic_cache_read > 0 || anthropic_cache_write > 0 {
        input += anthropic_cache_read + anthropic_cache_write;
    }

    let cached_input = anthropic_cache_read.max(openai_cached).min(input);

    if input <= 0 && output <= 0 {
        return None;
    }
    Some(TokenUsage { input, output, cached_input })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn openai_responses_completion_event() {
        let v = json!({
            "type": "response.completed",
            "response": {
                "usage": {
                    "input_tokens": 1200,
                    "output_tokens": 340,
                    "input_tokens_details": { "cached_tokens": 1024 }
                }
            }
        });
        assert_eq!(
            extract_usage(&v),
            Some(TokenUsage { input: 1200, output: 340, cached_input: 1024 })
        );
    }

    #[test]
    fn openai_chat_completions_style_relay() {
        let v = json!({
            "usage": {
                "prompt_tokens": 800,
                "completion_tokens": 120,
                "prompt_tokens_details": { "cached_tokens": 256 }
            }
        });
        assert_eq!(
            extract_usage(&v),
            Some(TokenUsage { input: 800, output: 120, cached_input: 256 })
        );
    }

    #[test]
    fn anthropic_splits_usage_across_start_and_delta_frames() {
        let mut acc = UsageAccumulator::new();
        acc.ingest_json(&json!({
            "type": "message_start",
            "message": { "usage": { "input_tokens": 500, "output_tokens": 1 } }
        }));
        acc.ingest_json(&json!({
            "type": "message_delta",
            "usage": { "output_tokens": 275 }
        }));
        // input survives from the start frame, output takes the final total.
        assert_eq!(
            acc.usage(),
            TokenUsage { input: 500, output: 275, cached_input: 0 }
        );
    }

    #[test]
    fn anthropic_cache_tokens_expand_input_and_mark_the_cached_subset() {
        let v = json!({
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50,
                "cache_read_input_tokens": 4000,
                "cache_creation_input_tokens": 200
            }
        });
        // Full prompt = 100 + 4000 + 200; only the 4000 read hits the cheap rate.
        assert_eq!(
            extract_usage(&v),
            Some(TokenUsage { input: 4300, output: 50, cached_input: 4000 })
        );
    }

    #[test]
    fn gemini_counts_thinking_tokens_as_output() {
        let v = json!({
            "usageMetadata": {
                "promptTokenCount": 900,
                "candidatesTokenCount": 120,
                "thoughtsTokenCount": 380,
                "cachedContentTokenCount": 640
            }
        });
        assert_eq!(
            extract_usage(&v),
            Some(TokenUsage { input: 900, output: 500, cached_input: 640 })
        );
    }

    #[test]
    fn sse_chunks_are_parsed_and_done_is_ignored() {
        let mut acc = UsageAccumulator::new();
        acc.ingest_sse_chunk(": heartbeat\n\n");
        acc.ingest_sse_chunk("event: response.completed\ndata: {\"response\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":20}}}\n\n");
        acc.ingest_sse_chunk("data: [DONE]\n\n");
        assert_eq!(
            acc.usage(),
            TokenUsage { input: 10, output: 20, cached_input: 0 }
        );
    }

    #[test]
    fn a_split_or_malformed_frame_never_panics_or_invents_usage() {
        let mut acc = UsageAccumulator::new();
        acc.ingest_sse_chunk("data: {\"response\":{\"usa");
        acc.ingest_sse_chunk("ge\":{\"input_tokens\":10}}}\n\n");
        assert!(acc.usage().is_empty());
    }

    #[test]
    fn frames_without_usage_yield_nothing() {
        assert_eq!(extract_usage(&json!({"type": "content_block_delta"})), None);
        assert_eq!(extract_usage(&json!({"usage": {}})), None);
        assert_eq!(extract_usage(&json!({"usage": "nonsense"})), None);
    }

    #[test]
    fn repeated_cumulative_totals_are_not_summed() {
        let mut acc = UsageAccumulator::new();
        for _ in 0..3 {
            acc.ingest_json(&json!({"usage": {"input_tokens": 100, "output_tokens": 40}}));
        }
        assert_eq!(
            acc.usage(),
            TokenUsage { input: 100, output: 40, cached_input: 0 }
        );
    }
}
