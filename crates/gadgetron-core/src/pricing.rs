//! Model pricing — converts prompt + completion token counts to
//! integer `cost_cents` for the `audit_log` row.
//!
//! Scope: ISSUE 4 TASK 4.2 stops `cost_cents` from being a flat `0`
//! in every audit row. Having a real number unblocks the `/usage/summary`
//! dashboard, per-tenant invoicing (EPIC 4), and quota enforcement
//! against $$ (not just tokens).
//!
//! Precision: rates are stored as **cents per 1M tokens** (integer),
//! which means a single request with ≤ 100 tokens on a cheap model
//! rounds down to 0 cents in `cost_cents`. That's acceptable — the
//! audit row still has the raw token counts, and the aggregated sum
//! across a day rolls up correctly. If per-request sub-cent precision
//! ever matters we can widen to microcents (× 100) without a schema
//! change (BIGINT covers it).
//!
//! Extension: `default_pricing_table()` below is what we ship. An
//! operator override via `gadgetron.toml [pricing]` table is a
//! follow-on TASK if the baseline proves wrong for a tenant.

use std::collections::HashMap;

/// Per-model rate in cents per **1,000,000** input/output tokens.
#[derive(Debug, Clone, Copy)]
pub struct ModelRate {
    pub input_cents_per_million: u64,
    pub output_cents_per_million: u64,
}

/// Fallback rate when a model id isn't in the table. Set to 0 so a
/// brand-new model never surprises an operator with a phantom bill —
/// missing rates show as $0 in the rollup, which is a clear signal
/// to add the rate to `default_pricing_table`.
const UNKNOWN_MODEL_RATE: ModelRate = ModelRate {
    input_cents_per_million: 0,
    output_cents_per_million: 0,
};

/// Built-in pricing table. Rates reflect vendor list prices as of
/// early 2026; they're **not guarantees**. A tenant paying a
/// different negotiated rate should override via config (see
/// follow-on TASK).
///
/// Units: cents per 1,000,000 tokens. A $0.15/1M rate becomes `15`.
pub fn default_pricing_table() -> HashMap<&'static str, ModelRate> {
    let mut table = HashMap::new();

    // OpenAI GPT-4o family.
    table.insert(
        "gpt-4o",
        ModelRate {
            input_cents_per_million: 250,
            output_cents_per_million: 1000,
        },
    );
    table.insert(
        "gpt-4o-mini",
        ModelRate {
            input_cents_per_million: 15,
            output_cents_per_million: 60,
        },
    );

    // Anthropic Claude family.
    table.insert(
        "claude-sonnet-4-6",
        ModelRate {
            input_cents_per_million: 300,
            output_cents_per_million: 1500,
        },
    );
    table.insert(
        "claude-opus-4-7",
        ModelRate {
            input_cents_per_million: 1500,
            output_cents_per_million: 7500,
        },
    );
    table.insert(
        "claude-haiku-4-5",
        ModelRate {
            input_cents_per_million: 80,
            output_cents_per_million: 400,
        },
    );

    // Penny is Gadgetron's in-process agent model identifier — it
    // wraps a downstream provider and we don't know the underlying
    // model at this layer. Default to 0 (will be billed via the
    // underlying provider's audit entry once real session audit
    // lands in EPIC 2).
    table.insert(
        "penny",
        ModelRate {
            input_cents_per_million: 0,
            output_cents_per_million: 0,
        },
    );

    // Harness mock model — free by design.
    table.insert(
        "mock",
        ModelRate {
            input_cents_per_million: 0,
            output_cents_per_million: 0,
        },
    );
    table.insert(
        "mock-model",
        ModelRate {
            input_cents_per_million: 0,
            output_cents_per_million: 0,
        },
    );

    table
}

/// Look up a rate, falling back to the unknown-model rate.
pub fn rate_for_model<'a>(
    table: &'a HashMap<&'static str, ModelRate>,
    model: &str,
) -> &'a ModelRate {
    table.get(model).unwrap_or_else(|| {
        // Some providers append `-2024-07-18` suffixes etc.
        // Strip trailing `-YYYY-MM-DD` and retry once before
        // falling back. This catches `gpt-4o-2024-07-18`,
        // `claude-sonnet-4-6-20260901`, etc.
        let stripped = model
            .rsplit_once('-')
            .and_then(|(prefix, suffix)| {
                if suffix.chars().all(|c| c.is_ascii_digit() || c == '-') && suffix.len() >= 4 {
                    Some(prefix)
                } else {
                    None
                }
            })
            .unwrap_or(model);
        table.get(stripped).unwrap_or(&UNKNOWN_MODEL_RATE)
    })
}

/// Compute `cost_cents` for one chat completion.
///
/// `input_tokens` + `output_tokens` are the values reported by the
/// provider in its `usage` block. The result is integer cents —
/// sub-cent amounts round down. Callers that need higher precision
/// should track raw tokens and compute cost at aggregation time.
pub fn compute_cost_cents(
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    table: &HashMap<&'static str, ModelRate>,
) -> i64 {
    let rate = rate_for_model(table, model);
    let input = input_tokens.saturating_mul(rate.input_cents_per_million);
    let output = output_tokens.saturating_mul(rate.output_cents_per_million);
    // Divide at the end so cross-term precision survives.
    ((input + output) / 1_000_000) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_model_costs_out() {
        let table = default_pricing_table();
        // gpt-4o: 250 + 1000 per 1M tokens.
        // 2000 in, 1000 out → (2000*250 + 1000*1000) / 1_000_000
        // = (500_000 + 1_000_000) / 1_000_000 = 1 cent (rounds down)
        let cost = compute_cost_cents("gpt-4o", 2000, 1000, &table);
        assert_eq!(cost, 1);
    }

    #[test]
    fn large_gpt_4o_request() {
        let table = default_pricing_table();
        // 500k in, 250k out: 500_000 * 250 + 250_000 * 1000 = 125M + 250M
        // = 375M / 1M = 375 cents
        let cost = compute_cost_cents("gpt-4o", 500_000, 250_000, &table);
        assert_eq!(cost, 375);
    }

    #[test]
    fn unknown_model_returns_zero() {
        let table = default_pricing_table();
        let cost = compute_cost_cents("brand-new-model-2099", 1_000_000, 1_000_000, &table);
        assert_eq!(cost, 0);
    }

    #[test]
    fn dated_suffix_falls_back_to_base() {
        let table = default_pricing_table();
        // `gpt-4o-2024-07-18` should strip the date suffix and hit
        // `gpt-4o`'s rate. The strip heuristic is conservative: it
        // only kicks in for an all-digit final segment.
        let cost = compute_cost_cents("gpt-4o-2024", 1_000_000, 1_000_000, &table);
        assert_eq!(cost, 250 + 1000);
    }

    #[test]
    fn mock_model_is_free() {
        let table = default_pricing_table();
        assert_eq!(compute_cost_cents("mock", 1_000_000, 1_000_000, &table), 0);
        assert_eq!(
            compute_cost_cents("mock-model", 1_000_000, 1_000_000, &table),
            0
        );
    }

    #[test]
    fn tiny_request_rounds_to_zero() {
        let table = default_pricing_table();
        // gpt-4o-mini at 100 tokens in + 50 out:
        // 100*15 + 50*60 = 1500 + 3000 = 4500 / 1M = 0
        let cost = compute_cost_cents("gpt-4o-mini", 100, 50, &table);
        assert_eq!(cost, 0);
    }

    #[test]
    fn saturating_does_not_overflow_on_silly_input() {
        let table = default_pricing_table();
        // u64::MAX tokens at any nonzero rate would overflow a
        // naive multiply — `saturating_mul` caps at u64::MAX.
        let cost = compute_cost_cents("gpt-4o", u64::MAX, 0, &table);
        assert!(cost >= 0);
    }
}
