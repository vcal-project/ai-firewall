use std::collections::HashMap;

use crate::{
    config::{EmbeddingPrice, ModelPrice},
    types::openai::Usage,
};

pub fn is_priced_model(model: &str, model_prices: &HashMap<String, ModelPrice>) -> bool {
    let model = model.trim();

    if model.is_empty() {
        return false;
    }

    model_prices.contains_key(model)
}

pub fn estimate_micro_usd_saved(
    model: &str,
    usage: &Usage,
    model_prices: &HashMap<String, ModelPrice>,
) -> u64 {
    let model = model.trim();

    if model.is_empty() {
        return 0;
    }

    let Some(price) = model_prices.get(model) else {
        return 0;
    };

    let prompt_cost_micro_usd =
        tokens_to_micro_usd(usage.prompt_tokens, price.input_usd_per_1m_tokens);

    let completion_cost_micro_usd =
        tokens_to_micro_usd(usage.completion_tokens, price.output_usd_per_1m_tokens);

    prompt_cost_micro_usd.saturating_add(completion_cost_micro_usd)
}

pub fn estimate_embedding_micro_usd(
    prompt_tokens: u32,
    embedding_price: Option<&EmbeddingPrice>,
) -> u64 {
    let Some(price) = embedding_price else {
        return 0;
    };

    tokens_to_micro_usd(prompt_tokens, price.usd_per_1m_tokens)
}

#[allow(dead_code)]
pub fn estimate_net_semantic_saved_micro_usd(
    model: &str,
    usage: &Usage,
    embedding_prompt_tokens: u32,
    model_prices: &HashMap<String, ModelPrice>,
    embedding_price: Option<&EmbeddingPrice>,
) -> u64 {
    let gross_saved = estimate_micro_usd_saved(model, usage, model_prices);
    let embedding_cost = estimate_embedding_micro_usd(embedding_prompt_tokens, embedding_price);

    gross_saved.saturating_sub(embedding_cost)
}

fn tokens_to_micro_usd(tokens: u32, usd_per_1m_tokens: f64) -> u64 {
    if !usd_per_1m_tokens.is_finite() || usd_per_1m_tokens <= 0.0 || tokens == 0 {
        return 0;
    }

    let total_micro_usd = (tokens as f64) * usd_per_1m_tokens;

    if !total_micro_usd.is_finite() || total_micro_usd <= 0.0 {
        return 0;
    }

    if total_micro_usd >= u64::MAX as f64 {
        return u64::MAX;
    }

    total_micro_usd.round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EmbeddingPrice, ModelPrice};
    use crate::types::openai::Usage;
    use std::collections::HashMap;

    fn usage(prompt_tokens: u32, completion_tokens: u32) -> Usage {
        Usage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
            extra: serde_json::Map::new(),
        }
    }

    fn model_prices() -> HashMap<String, ModelPrice> {
        let mut prices = HashMap::new();
        prices.insert(
            "gpt-4o-mini-2024-07-18".to_string(),
            ModelPrice {
                input_usd_per_1m_tokens: 0.15,
                output_usd_per_1m_tokens: 0.60,
            },
        );
        prices
    }

    #[test]
    fn priced_model_is_detected() {
        let prices = model_prices();
        assert!(is_priced_model("gpt-4o-mini-2024-07-18", &prices));
    }

    #[test]
    fn priced_model_lookup_trims_whitespace() {
        let prices = model_prices();
        assert!(is_priced_model("  gpt-4o-mini-2024-07-18  ", &prices));
    }

    #[test]
    fn exact_hit_savings_are_calculated_correctly() {
        let prices = model_prices();
        let usage = usage(1_000, 500);

        let saved = estimate_micro_usd_saved("gpt-4o-mini-2024-07-18", &usage, &prices);

        assert_eq!(saved, 450);
    }

    #[test]
    fn exact_hit_savings_trim_model_name() {
        let prices = model_prices();
        let usage = usage(1_000, 500);

        let saved = estimate_micro_usd_saved("  gpt-4o-mini-2024-07-18  ", &usage, &prices);

        assert_eq!(saved, 450);
    }

    #[test]
    fn embedding_cost_is_calculated_correctly() {
        let embedding_price = EmbeddingPrice {
            usd_per_1m_tokens: 0.020,
        };

        let cost = estimate_embedding_micro_usd(1_000, Some(&embedding_price));

        assert_eq!(cost, 20);
    }

    #[test]
    fn semantic_hit_net_savings_subtract_embedding_cost() {
        let prices = model_prices();
        let usage = usage(1_000, 500);
        let embedding_price = EmbeddingPrice {
            usd_per_1m_tokens: 0.020,
        };

        let net = estimate_net_semantic_saved_micro_usd(
            "gpt-4o-mini-2024-07-18",
            &usage,
            1_000,
            &prices,
            Some(&embedding_price),
        );

        assert_eq!(net, 430);
    }

    #[test]
    fn semantic_hit_net_savings_saturate_at_zero() {
        let prices = model_prices();
        let usage = usage(1, 0);

        let embedding_price = EmbeddingPrice {
            usd_per_1m_tokens: 10.0,
        };

        let net = estimate_net_semantic_saved_micro_usd(
            "gpt-4o-mini-2024-07-18",
            &usage,
            1_000,
            &prices,
            Some(&embedding_price),
        );

        assert_eq!(net, 0);
    }

    #[test]
    fn unknown_model_returns_zero_saved_cost() {
        let prices = model_prices();
        let usage = usage(1_000, 500);

        let saved = estimate_micro_usd_saved("unknown-model", &usage, &prices);

        assert_eq!(saved, 0);
    }

    #[test]
    fn missing_embedding_price_returns_zero_embedding_cost() {
        let cost = estimate_embedding_micro_usd(1_000, None);

        assert_eq!(cost, 0);
    }

    #[test]
    fn rounds_micro_usd_to_nearest_integer() {
        let embedding_price = EmbeddingPrice {
            usd_per_1m_tokens: 0.015,
        };

        let cost = estimate_embedding_micro_usd(100, Some(&embedding_price));

        assert_eq!(cost, 2);
    }
}
