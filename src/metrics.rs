#![allow(clippy::expect_used)]

use once_cell::sync::Lazy;
use prometheus::{
    core::Collector, Encoder, Histogram, HistogramOpts, IntCounter, IntCounterVec, IntGauge,
    Registry, TextEncoder,
};
use std::sync::Once;

static INIT: Once = Once::new();

pub static REGISTRY: Lazy<Registry> = Lazy::new(Registry::new);

// -----------------------------
// Overview metrics
// -----------------------------

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

pub static CHAT_COST_SAVED_MICRO_USD: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new(
        "aif_chat_cost_saved_micro_usd",
        "Estimated gross chat-completion cost saved in micro-USD",
    )
    .expect("metric aif_chat_cost_saved_micro_usd must be valid")
});

pub static EMBEDDING_COST_MICRO_USD: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new(
        "aif_embedding_cost_micro_usd",
        "Estimated embedding cost in micro-USD",
    )
    .expect("metric aif_embedding_cost_micro_usd must be valid")
});

pub static COST_SAVED_MICRO_USD: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new(
        "aif_cost_saved_micro_usd",
        "Estimated net cost saved in micro-USD",
    )
    .expect("metric aif_cost_saved_micro_usd must be valid")
});

pub static INFLIGHT_REQUESTS: Lazy<IntGauge> = Lazy::new(|| {
    IntGauge::new("aif_inflight_requests", "In-flight requests")
        .expect("metric aif_inflight_requests must be valid")
});

// -----------------------------
// Operational hardening metrics
// -----------------------------

pub static ERRORS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new("aif_errors_total", "Total errors by classification"),
        &["class"],
    )
    .expect("metric aif_errors_total must be valid")
});

pub static READINESS_STATE: Lazy<IntGauge> = Lazy::new(|| {
    IntGauge::new(
        "aif_readiness_state",
        "Whether the service is ready to accept requests (0 or 1)",
    )
    .expect("metric aif_readiness_state must be valid")
});

pub static SHUTDOWN_IN_PROGRESS: Lazy<IntGauge> = Lazy::new(|| {
    IntGauge::new(
        "aif_shutdown_in_progress",
        "Whether graceful shutdown is in progress (0 or 1)",
    )
    .expect("metric aif_shutdown_in_progress must be valid")
});

pub static SHUTDOWN_REJECTIONS_TOTAL: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new(
        "aif_shutdown_rejections_total",
        "Requests rejected because the server was shutting down",
    )
    .expect("metric aif_shutdown_rejections_total must be valid")
});

pub static UPSTREAM_TIMEOUTS_TOTAL: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new(
        "aif_upstream_timeouts_total",
        "Upstream requests that timed out",
    )
    .expect("metric aif_upstream_timeouts_total must be valid")
});

pub static UPSTREAM_REQUEST_DURATION_SECONDS: Lazy<Histogram> = Lazy::new(|| {
    Histogram::with_opts(HistogramOpts::new(
        "aif_upstream_request_duration_seconds",
        "Duration of upstream requests in seconds",
    ))
    .expect("metric aif_upstream_request_duration_seconds must be valid")
});

// -----------------------------
// Embedding provider diagnostics
// -----------------------------

pub static EMBEDDING_TIMEOUTS_TOTAL: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new(
        "aif_embedding_timeouts_total",
        "Embedding provider requests that timed out",
    )
    .expect("metric aif_embedding_timeouts_total must be valid")
});

pub static EMBEDDING_REQUEST_DURATION_SECONDS: Lazy<Histogram> = Lazy::new(|| {
    Histogram::with_opts(HistogramOpts::new(
        "aif_embedding_request_duration_seconds",
        "Duration of embedding provider requests in seconds",
    ))
    .expect("metric aif_embedding_request_duration_seconds must be valid")
});

// -----------------------------
// Semantic diagnostics metrics
// -----------------------------

pub static SEMANTIC_STORE_TOTAL: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new(
        "aif_semantic_store_total",
        "Total number of semantic cache store attempts",
    )
    .expect("metric aif_semantic_store_total must be valid")
});

pub static SEMANTIC_STORE_ERRORS_TOTAL: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new(
        "aif_semantic_store_errors_total",
        "Total number of semantic cache store failures",
    )
    .expect("metric aif_semantic_store_errors_total must be valid")
});

pub static SEMANTIC_LOOKUP_ERRORS_TOTAL: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new(
        "aif_semantic_lookup_errors_total",
        "Total number of semantic cache lookup failures",
    )
    .expect("metric aif_semantic_lookup_errors_total must be valid")
});

pub static SEMANTIC_PROVIDER_ERRORS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_semantic_provider_errors_total",
            "Semantic provider errors by provider and operation",
        ),
        &["provider", "operation"],
    )
    .expect("metric aif_semantic_provider_errors_total must be valid")
});

pub static SEMANTIC_SKIPS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new("aif_semantic_skips_total", "Semantic cache skips by reason"),
        &["reason"],
    )
    .expect("metric aif_semantic_skips_total must be valid")
});

pub static SEMANTIC_CANDIDATES_CHECKED_TOTAL: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new(
        "aif_semantic_candidates_checked_total",
        "Semantic cache candidates checked",
    )
    .expect("metric aif_semantic_candidates_checked_total must be valid")
});

pub static SEMANTIC_THRESHOLD_RESULTS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_semantic_threshold_results_total",
            "Semantic threshold decisions",
        ),
        &["result"],
    )
    .expect("metric aif_semantic_threshold_results_total must be valid")
});

pub static SEMANTIC_EXPIRED_ENTRIES_SKIPPED_TOTAL: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new(
        "aif_semantic_expired_entries_skipped_total",
        "Expired semantic cache entries skipped during lookup",
    )
    .expect("metric aif_semantic_expired_entries_skipped_total must be valid")
});

pub static SEMANTIC_LOOKUP_DURATION_SECONDS: Lazy<Histogram> = Lazy::new(|| {
    Histogram::with_opts(HistogramOpts::new(
        "aif_semantic_lookup_duration_seconds",
        "Duration of semantic cache lookups in seconds",
    ))
    .expect("metric aif_semantic_lookup_duration_seconds must be valid")
});

pub fn init() {
    Lazy::force(&SEMANTIC_STORE_TOTAL);
    Lazy::force(&SEMANTIC_STORE_ERRORS_TOTAL);
    Lazy::force(&SEMANTIC_LOOKUP_ERRORS_TOTAL);
    Lazy::force(&SEMANTIC_PROVIDER_ERRORS_TOTAL);
    Lazy::force(&SEMANTIC_SKIPS_TOTAL);
    Lazy::force(&SEMANTIC_CANDIDATES_CHECKED_TOTAL);
    Lazy::force(&SEMANTIC_THRESHOLD_RESULTS_TOTAL);
    Lazy::force(&SEMANTIC_LOOKUP_DURATION_SECONDS);
    Lazy::force(&SEMANTIC_EXPIRED_ENTRIES_SKIPPED_TOTAL);
    Lazy::force(&EMBEDDING_TIMEOUTS_TOTAL);
    Lazy::force(&EMBEDDING_REQUEST_DURATION_SECONDS);

    INIT.call_once(|| {
        let collectors: Vec<Box<dyn Collector>> = vec![
            Box::new(REQUESTS_TOTAL.clone()),
            Box::new(CACHE_EXACT_HITS.clone()),
            Box::new(CACHE_SEMANTIC_HITS.clone()),
            Box::new(SEMANTIC_STORE_TOTAL.clone()),
            Box::new(SEMANTIC_STORE_ERRORS_TOTAL.clone()),
            Box::new(SEMANTIC_LOOKUP_ERRORS_TOTAL.clone()),
            Box::new(SEMANTIC_PROVIDER_ERRORS_TOTAL.clone()),
            Box::new(SEMANTIC_SKIPS_TOTAL.clone()),
            Box::new(CACHE_MISSES.clone()),
            Box::new(UPSTREAM_CALLS.clone()),
            Box::new(TOKENS_SAVED.clone()),
            Box::new(CHAT_COST_SAVED_MICRO_USD.clone()),
            Box::new(EMBEDDING_COST_MICRO_USD.clone()),
            Box::new(COST_SAVED_MICRO_USD.clone()),
            Box::new(INFLIGHT_REQUESTS.clone()),
            Box::new(ERRORS_TOTAL.clone()),
            Box::new(READINESS_STATE.clone()),
            Box::new(SHUTDOWN_IN_PROGRESS.clone()),
            Box::new(SHUTDOWN_REJECTIONS_TOTAL.clone()),
            Box::new(UPSTREAM_TIMEOUTS_TOTAL.clone()),
            Box::new(UPSTREAM_REQUEST_DURATION_SECONDS.clone()),
            Box::new(SEMANTIC_CANDIDATES_CHECKED_TOTAL.clone()),
            Box::new(SEMANTIC_THRESHOLD_RESULTS_TOTAL.clone()),
            Box::new(SEMANTIC_EXPIRED_ENTRIES_SKIPPED_TOTAL.clone()),
            Box::new(SEMANTIC_LOOKUP_DURATION_SECONDS.clone()),
            Box::new(EMBEDDING_TIMEOUTS_TOTAL.clone()),
            Box::new(EMBEDDING_REQUEST_DURATION_SECONDS.clone()),
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
