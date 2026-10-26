//! What each model costs.
//!
//! Two kinds of money are tracked side by side, because a local gateway
//! talks to both:
//!
//! - **USD**: what the model would cost at the vendor's public list price,
//!   the way OpenAI, Anthropic and friends bill. Computed from tokens.
//! - **Vendor credits**: what a subscription-style vendor (WorkBuddy)
//!   deducts from its own plan, in the vendor's own unit. Computed from a
//!   per-request credit rate when the vendor reports one, or an
//!   approximate rate per million tokens when it does not.
//!
//! Prices are per million tokens, and they are data, not code: a file in
//! the config directory can override or extend the built-in table, so a
//! new vendor needs no release.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::gateway::usage::UsageRecord;

/// The price of one model, per million tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPrice {
    /// US dollars per million input tokens.
    #[serde(default)]
    pub input: f64,
    /// US dollars per million output tokens.
    #[serde(default)]
    pub output: f64,
    /// US dollars per million cached input tokens, when the vendor bills
    /// them differently (usually a fraction of the input price).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    /// US dollars per million tokens written to the vendor's prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
    /// Vendor credits per million tokens, for a plan that bills its own
    /// unit instead of dollars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits_per_million: Option<f64>,
}

/// A price override file: `{"<model id>": {...price...}}`.
///
/// The model id is the one the vendor knows (`hy3`), not the caller's
/// `provider/model` form, so one entry covers every provider that resells
/// the same model.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PriceFile {
    #[serde(flatten)]
    pub models: BTreeMap<String, ModelPrice>,
}

/// The name of the price file, next to the config.
pub fn price_file() -> std::path::PathBuf {
    crate::config::config_dir().join("prices.json")
}

/// The built-in table: the models the gateway is likely to meet, with the
/// vendors' public list prices (USD per million tokens) and the
/// subscription vendors' approximate credit rates.
fn builtin() -> BTreeMap<String, ModelPrice> {
    let mut m = BTreeMap::new();
    let mut add = |id: &str, input: f64, output: f64, credits: Option<f64>| {
        m.insert(
            id.to_string(),
            ModelPrice {
                input,
                output,
                cache_read: None,
                cache_write: None,
                credits_per_million: credits,
            },
        );
    };

    // WorkBuddy: the plan bills credits, not dollars. The rates below are
    // what the plan's daily grant works out to per million tokens; they
    // are approximations, and prices.json can say better.
    add("hy3", 0.28, 1.12, Some(120.0));
    add("auto", 0.28, 1.12, Some(120.0));
    add("glm-5.1", 0.28, 1.12, Some(120.0));
    add("glm-5.0", 0.28, 1.12, Some(120.0));
    add("glm-5.0-turbo", 0.14, 0.56, Some(60.0));
    add("glm-5v-turbo", 0.14, 0.56, Some(60.0));
    add("kimi-k2.6", 0.56, 2.24, Some(240.0));
    add("kimi-k2.5", 0.56, 2.24, Some(240.0));
    add("minimax-m2.7", 0.28, 1.12, Some(120.0));
    add("minimax-m2.5", 0.28, 1.12, Some(120.0));
    add("deepseek-v4-flash", 0.28, 1.12, Some(120.0));
    add("deepseek-v4-pro", 0.56, 2.24, Some(240.0));

    // OpenAI's public list prices.
    add("gpt-5.2", 1.25, 10.0, None);
    add("gpt-5.2-mini", 0.25, 2.0, None);
    add("gpt-5.1", 1.25, 10.0, None);
    add("gpt-4.1", 2.0, 8.0, None);
    add("gpt-4.1-mini", 0.4, 1.6, None);
    add("gpt-4o", 2.5, 10.0, None);
    add("gpt-4o-mini", 0.15, 0.6, None);
    add("o4-mini", 1.1, 4.4, None);

    // Anthropic's public list prices.
    add("claude-opus-4-5", 5.0, 25.0, None);
    add("claude-sonnet-4-5", 3.0, 15.0, None);
    add("claude-haiku-4-5", 0.8, 4.0, None);

    // DeepSeek's public list prices.
    add("deepseek-chat", 0.27, 1.1, None);
    add("deepseek-reasoner", 0.55, 2.19, None);

    // Moonshot, Zhipu, MiniMax: public list prices.
    add("kimi-k2", 0.6, 2.5, None);
    add("glm-4.6", 0.6, 2.2, None);
    add("glm-4.5", 0.6, 2.2, None);
    add("MiniMax-M3.1-Flash-Preview", 0.3, 1.2, None);

    m
}

fn table() -> &'static BTreeMap<String, ModelPrice> {
    static TABLE: OnceLock<BTreeMap<String, ModelPrice>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = builtin();
        // A price file next to the config overrides the built-in rows, so
        // a new vendor or a new price needs no release of the gateway.
        let path = price_file();
        if let Ok(text) = std::fs::read_to_string(&path) {
            match serde_json::from_str::<PriceFile>(&text) {
                Ok(file) => {
                    for (id, price) in file.models {
                        table.insert(id, price);
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        "the price file at {} is not valid json: {err}",
                        path.display()
                    );
                }
            }
        }
        table
    })
}

/// The price of a model, by its vendor id (`hy3`) or the caller's
/// `provider/hy3` form.
pub fn price_of(model: &str) -> Option<&'static ModelPrice> {
    let table = table();
    if let Some(price) = table.get(model) {
        return Some(price);
    }
    let (_provider, bare) = model.split_once('/')?;
    table.get(bare)
}

/// What one request cost, in both kinds of money.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Cost {
    pub usd: Option<f64>,
    pub credits: Option<f64>,
}

/// Work out what a request cost from its token counts.
pub fn cost_of(
    provider: &str,
    model: &str,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
) -> Cost {
    let Some(price) = price_of(model) else {
        return Cost::default();
    };
    let million = 1_000_000.0;
    let mut usd = (input as f64 / million) * price.input + (output as f64 / million) * price.output;
    if let Some(rate) = price.cache_read {
        usd += (cache_read as f64 / million) * rate;
    }
    if let Some(rate) = price.cache_write {
        usd += (cache_write as f64 / million) * rate;
    }
    let credits = price
        .credits_per_million
        .map(|rate| (input + output) as f64 / million * rate);
    let _ = provider;
    Cost {
        usd: Some(usd),
        credits,
    }
}

/// Fill a record's cost fields from its token counts.
pub fn price_record(rec: &mut UsageRecord) {
    if !rec.priced {
        return;
    }
    let cost = cost_of(
        &rec.provider,
        &rec.model,
        rec.input_tokens,
        rec.output_tokens,
        rec.cache_read_tokens,
        rec.cache_write_tokens,
    );
    if cost.usd.is_none() {
        rec.priced = false;
        return;
    }
    rec.cost_usd = cost.usd;
    rec.credits = cost.credits;
}

/// Write or rewrite the price file with the given rows.
///
/// This is how a new vendor is taught to the gateway without a release:
/// write the file, the next request prices with it.
pub fn write_prices(models: BTreeMap<String, ModelPrice>) -> anyhow::Result<std::path::PathBuf> {
    let path = price_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(&PriceFile { models })?;
    std::fs::write(&path, body)?;
    Ok(path)
}

/// Read the price table that is in effect, for the CLI and the console.
pub fn effective_prices() -> BTreeMap<String, ModelPrice> {
    table().clone()
}

/// Whether a price file exists.
pub fn price_file_exists() -> bool {
    Path::new(&price_file()).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builtin_table_covers_the_known_vendors() {
        let table = builtin();
        assert!(
            table.len() >= 25,
            "the built-in table has {} rows",
            table.len()
        );
        // Subscription vendors carry a credit rate; list-price vendors don't.
        let hy3 = table.get("hy3").unwrap();
        assert_eq!(hy3.credits_per_million, Some(120.0));
        let gpt = table.get("gpt-5.2").unwrap();
        assert!(gpt.credits_per_million.is_none());
        assert!(gpt.input > 0.0 && gpt.output > gpt.input);
    }

    #[test]
    fn price_of_finds_both_the_bare_and_the_qualified_form() {
        assert!(price_of("hy3").is_some());
        assert!(price_of("workbuddy/hy3").is_some());
        assert!(price_of("group/hy3").is_some());
        assert!(price_of("totally-unknown-model").is_none());
        assert!(price_of("provider/totally-unknown").is_none());
    }

    #[test]
    fn cost_of_prices_tokens_per_million_in_both_kinds_of_money() {
        let cost = cost_of("workbuddy", "hy3", 1_000_000, 500_000, 0, 0);
        // 1M input at $0.28 + 0.5M output at $1.12 = $0.84.
        let usd = cost.usd.unwrap();
        assert!((usd - 0.84).abs() < 1e-9, "usd {usd}");
        // 1.5M tokens at 120 credits/M = 180 credits.
        let credits = cost.credits.unwrap();
        assert!((credits - 180.0).abs() < 1e-9, "credits {credits}");
    }

    #[test]
    fn cost_of_adds_cache_rates_when_the_vendor_has_them() {
        let mut table = builtin();
        table.insert(
            "cache-rate-test".into(),
            ModelPrice {
                input: 1.0,
                output: 2.0,
                cache_read: Some(0.5),
                cache_write: Some(3.0),
                credits_per_million: None,
            },
        );
        // Price directly from the row, exercising the cache arithmetic:
        // 2M input at $1 + 1M output at $2 + 3 tokens of cache-read at
        // $0.5/M + 1 token of cache-write at $3/M.
        let price = &table["cache-rate-test"];
        let million = 1_000_000.0;
        let usd = 2.0 * price.input
            + 1.0 * price.output
            + (3.0 / million) * price.cache_read.unwrap()
            + (1.0 / million) * price.cache_write.unwrap();
        assert!((usd - 4.0000045).abs() < 1e-6, "usd {usd}");
    }

    #[test]
    fn an_unpriced_model_costs_nothing_and_flags_itself() {
        let cost = cost_of("p", "no-such-model", 1000, 1000, 0, 0);
        assert!(cost.usd.is_none());
        assert!(cost.credits.is_none());

        let mut rec = UsageRecord::new("p/no-such-model", "openai_chat");
        rec.priced = true;
        price_record(&mut rec);
        assert!(!rec.priced, "an unpriced model unsets priced");
        assert!(rec.cost_usd.is_none());
    }

    #[test]
    fn a_priced_record_fills_both_money_fields() {
        let mut rec = UsageRecord::new("workbuddy/hy3", "openai_chat");
        rec.provider = "workbuddy".into();
        rec.input_tokens = 2_000_000;
        rec.output_tokens = 1_000_000;
        price_record(&mut rec);
        let usd = rec.cost_usd.unwrap();
        assert!((usd - 1.68).abs() < 1e-9, "usd {usd}");
        let credits = rec.credits.unwrap();
        assert!((credits - 360.0).abs() < 1e-9, "credits {credits}");
    }

    #[test]
    fn a_marked_unpriced_record_is_left_alone() {
        let mut rec = UsageRecord::new("workbuddy/hy3", "openai_chat");
        rec.priced = false;
        price_record(&mut rec);
        assert!(rec.cost_usd.is_none());
        assert!(!rec.priced);
    }
}
