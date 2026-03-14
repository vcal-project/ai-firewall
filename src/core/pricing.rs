use std::collections::HashMap;

use crate::{config::ModelPrice, types::openai::Usage};

pub fn estimate_micro_usd_saved(
    model: &str,
    usage: &Usage,
    model_prices: &HashMap<String, ModelPrice>,
) -> u64 {
    let Some(price) = model_prices.get(model) else {
        return 0;
    };

    let prompt_cost_micro_usd = (usage.prompt_tokens as f64)
        * usd_per_1m_tokens_to_micro_usd_per_token(price.input_usd_per_1m_tokens);

    let completion_cost_micro_usd = (usage.completion_tokens as f64)
        * usd_per_1m_tokens_to_micro_usd_per_token(price.output_usd_per_1m_tokens);

    let total_micro_usd = prompt_cost_micro_usd + completion_cost_micro_usd;

    if !total_micro_usd.is_finite() || total_micro_usd <= 0.0 {
        return 0;
    }

    if total_micro_usd >= u64::MAX as f64 {
        return u64::MAX;
    }

    total_micro_usd.round() as u64
}

fn usd_per_1m_tokens_to_micro_usd_per_token(usd_per_1m_tokens: f64) -> f64 {
    usd_per_1m_tokens
}
