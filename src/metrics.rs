#![allow(clippy::expect_used)]

use once_cell::sync::Lazy;
use prometheus::{Encoder, IntCounter, IntCounterVec, IntGauge, Registry, TextEncoder};
use std::sync::Once;

static INIT: Once = Once::new();

pub static REGISTRY: Lazy<Registry> = Lazy::new(Registry::new);

pub static REQUESTS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new("aif_requests_total", "Total requests"),
        &["endpoint"],
    )
    .expect("metric aif_requests_total must be valid")
});

pub static CACHE_EXACT_HITS: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new("aif_cache_exact_hits", "Exact cache hits")
        .expect("metric aif_cache_exact_hits must be valid")
});

pub static CACHE_SEMANTIC_HITS: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new("aif_cache_semantic_hits", "Semantic cache hits")
        .expect("metric aif_cache_semantic_hits must be valid")
});

pub static CACHE_MISSES: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new("aif_cache_misses", "Cache misses")
        .expect("metric aif_cache_misses must be valid")
});

pub static UPSTREAM_CALLS: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new("aif_upstream_calls_total", "Upstream calls")
        .expect("metric aif_upstream_calls_total must be valid")
});

pub static TOKENS_SAVED: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new("aif_tokens_saved", "Estimated tokens saved")
        .expect("metric aif_tokens_saved must be valid")
});

pub static COST_SAVED_MICRO_USD: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new(
        "aif_cost_saved_micro_usd",
        "Estimated cost saved in micro-USD",
    )
    .expect("metric aif_cost_saved_micro_usd must be valid")
});

pub static INFLIGHT_REQUESTS: Lazy<IntGauge> = Lazy::new(|| {
    IntGauge::new("aif_inflight_requests", "In-flight requests")
        .expect("metric aif_inflight_requests must be valid")
});

pub fn init() {
    INIT.call_once(|| {
        let collectors: Vec<Box<dyn prometheus::core::Collector>> = vec![
            Box::new(REQUESTS_TOTAL.clone()),
            Box::new(CACHE_EXACT_HITS.clone()),
            Box::new(CACHE_SEMANTIC_HITS.clone()),
            Box::new(CACHE_MISSES.clone()),
            Box::new(UPSTREAM_CALLS.clone()),
            Box::new(TOKENS_SAVED.clone()),
            Box::new(COST_SAVED_MICRO_USD.clone()),
            Box::new(INFLIGHT_REQUESTS.clone()),
        ];

        for c in collectors {
            REGISTRY
                .register(c)
                .expect("failed to register Prometheus collector");
        }
    });
}

pub fn render() -> Result<String, String> {
    let encoder = TextEncoder::new();
    let metric_families = REGISTRY.gather();
    let mut buffer = Vec::new();
    encoder
        .encode(&metric_families, &mut buffer)
        .map_err(|e| e.to_string())?;
    String::from_utf8(buffer).map_err(|e| e.to_string())
}