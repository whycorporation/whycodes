//! Per-model USD prices for the `/cost` estimate.
//!
//! Nothing is bundled: prices change and a stale table would print a
//! confident wrong number. Prices come from the provider's `/v1/models`
//! catalog (OpenRouter-style `pricing`) or from `[pricing]` in config; an
//! unknown model simply has no cost.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use serde_json::Value;
use whycodes_core::types::Usage;

/// USD per million tokens. Cache rates fall back to the input rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPrice {
    pub input: f64,
    pub output: f64,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

impl ModelPrice {
    /// Cost of one usage record. Cache fields are additive in [`Usage`]
    /// (Anthropic); OpenAI-style cached input sits inside `input_tokens`
    /// and is billed at the input rate here, so the estimate can run high.
    pub fn cost(&self, usage: &Usage) -> f64 {
        let per = |tokens: u64, rate: f64| tokens as f64 * rate / 1_000_000.0;
        per(usage.input_tokens, self.input)
            + per(usage.output_tokens, self.output)
            + per(
                usage.cache_read_input_tokens.unwrap_or(0),
                self.cache_read.unwrap_or(self.input),
            )
            + per(
                usage.cache_creation_input_tokens.unwrap_or(0),
                self.cache_write.unwrap_or(self.input),
            )
    }
}

/// OpenRouter-style `pricing` object (USD **per token**, often as strings).
pub fn price_from_model_value(m: &Value) -> Option<ModelPrice> {
    let pricing = m.get("pricing")?;
    let per_mtok = |key: &str| {
        let v = pricing.get(key)?;
        let n = v
            .as_f64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))?;
        (n >= 0.0).then_some(n * 1_000_000.0)
    };
    Some(ModelPrice {
        input: per_mtok("prompt")?,
        output: per_mtok("completion")?,
        cache_read: per_mtok("input_cache_read"),
        cache_write: per_mtok("input_cache_write"),
    })
}

static PRICES: LazyLock<RwLock<HashMap<String, ModelPrice>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

fn key(provider: &str, model: &str) -> String {
    format!("{provider}/{model}")
}

/// Record a price (catalog fetch or config). Later calls win.
pub fn remember_price(provider: &str, model: &str, price: ModelPrice) {
    let mut prices = match PRICES.write() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    prices.insert(key(provider, model), price);
}

/// Known price for `provider/model`, if any.
pub fn lookup_price(provider: &str, model: &str) -> Option<ModelPrice> {
    let prices = match PRICES.read() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    prices.get(&key(provider, model)).copied()
}

#[cfg(test)]
#[path = "pricing_tests.rs"]
mod tests;
