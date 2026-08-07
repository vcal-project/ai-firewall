#![allow(clippy::expect_used)]

use once_cell::sync::Lazy;
use prometheus::{
    core::Collector, Encoder, Histogram, HistogramOpts, HistogramVec, IntCounter, IntCounterVec,
    IntGauge, Registry, TextEncoder,
};
use std::sync::Once;

static INIT: Once = Once::new();

pub static REGISTRY: Lazy<Registry> = Lazy::new(Registry::new);

// -----------------------------
// Cost and savings metric labels
// -----------------------------

pub const COST_TYPE_CHAT: &str = "chat";
pub const COST_TYPE_EMBEDDING: &str = "embedding";

pub const CACHE_TYPE_EXACT: &str = "exact";
pub const CACHE_TYPE_SEMANTIC: &str = "semantic";

pub const EMBEDDING_OPERATION_LOOKUP: &str = "lookup";
pub const EMBEDDING_OPERATION_STORE: &str = "store";

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

pub static CACHE_BYPASS_REQUESTS_TOTAL: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new(
        "aif_cache_bypass_requests_total",
        "Requests that explicitly bypassed exact and semantic cache lookup/store",
    )
    .expect("metric aif_cache_bypass_requests_total must be valid")
});

pub static UPSTREAM_CALLS: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new("aif_upstream_calls_total", "Upstream calls")
        .expect("metric aif_upstream_calls_total must be valid")
});

pub static TOKENS_SAVED: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new("aif_tokens_saved", "Estimated tokens saved")
        .expect("metric aif_tokens_saved must be valid")
});

// Backward-compatible aggregate cost/savings metrics.
// Prefer the labeled v0.1.8 metrics below for new dashboards and accounting logic.
pub static CHAT_COST_SAVED_MICRO_USD: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new(
        "aif_chat_cost_saved_micro_usd",
        "Deprecated aggregate: estimated gross chat-completion cost saved in micro-USD",
    )
    .expect("metric aif_chat_cost_saved_micro_usd must be valid")
});

pub static EMBEDDING_COST_MICRO_USD: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new(
        "aif_embedding_cost_micro_usd",
        "Deprecated aggregate: estimated embedding cost in micro-USD",
    )
    .expect("metric aif_embedding_cost_micro_usd must be valid")
});

pub static COST_SAVED_MICRO_USD: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new(
        "aif_cost_saved_micro_usd",
        "Deprecated aggregate: estimated net cost saved in micro-USD",
    )
    .expect("metric aif_cost_saved_micro_usd must be valid")
});

// -----------------------------
// Cost and savings intelligence metrics
// -----------------------------

/// Prompt/input tokens returned by upstream chat completions, grouped by model.
pub static MODEL_REQUESTS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_model_requests_total",
            "Total upstream chat completion requests by model",
        ),
        &["model"],
    )
    .expect("metric aif_model_requests_total must be valid")
});

pub static MODEL_INPUT_TOKENS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_model_input_tokens_total",
            "Upstream chat prompt/input tokens by model",
        ),
        &["model"],
    )
    .expect("metric aif_model_input_tokens_total must be valid")
});

/// Completion/output tokens returned by upstream chat completions, grouped by model.
pub static MODEL_OUTPUT_TOKENS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_model_output_tokens_total",
            "Upstream chat completion/output tokens by model",
        ),
        &["model"],
    )
    .expect("metric aif_model_output_tokens_total must be valid")
});

/// Estimated upstream chat-completion cost, grouped by model.
pub static MODEL_COST_MICRO_USD_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_model_cost_micro_usd_total",
            "Estimated upstream chat-completion cost in micro-USD by model",
        ),
        &["model"],
    )
    .expect("metric aif_model_cost_micro_usd_total must be valid")
});

pub static CACHE_HITS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new("aif_cache_hits_total", "Cache hits by model and cache type"),
        &["model", "cache_type"],
    )
    .expect("metric aif_cache_hits_total must be valid")
});

pub static REQUEST_COST_MICRO_USD_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_request_cost_micro_usd_total",
            "Estimated request-related cost in micro-USD by model and cost type",
        ),
        &["model", "cost_type"],
    )
    .expect("metric aif_request_cost_micro_usd_total must be valid")
});

pub static GROSS_SAVED_MICRO_USD_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_gross_saved_micro_usd_total",
            "Gross avoided upstream chat-completion cost in micro-USD by model and cache type",
        ),
        &["model", "cache_type"],
    )
    .expect("metric aif_gross_saved_micro_usd_total must be valid")
});

pub static NET_SAVED_MICRO_USD_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_net_saved_micro_usd_total",
            "Net saved cost in micro-USD by model and cache type after accounting for overhead",
        ),
        &["model", "cache_type"],
    )
    .expect("metric aif_net_saved_micro_usd_total must be valid")
});

pub static EMBEDDING_OVERHEAD_MICRO_USD_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_embedding_overhead_micro_usd_total",
            "Estimated embedding overhead in micro-USD by model and operation",
        ),
        &["model", "operation"],
    )
    .expect("metric aif_embedding_overhead_micro_usd_total must be valid")
});

pub static INFLIGHT_REQUESTS: Lazy<IntGauge> = Lazy::new(|| {
    IntGauge::new("aif_inflight_requests", "In-flight requests")
        .expect("metric aif_inflight_requests must be valid")
});

// -----------------------------
// Operational hardening metrics
// -----------------------------

pub static BACKPRESSURE_REJECTIONS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_backpressure_rejections_total",
            "Requests rejected by bounded concurrency controls",
        ),
        &["scope"],
    )
    .expect("metric aif_backpressure_rejections_total must be valid")
});

pub static DEPENDENCY_FAILURES_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_dependency_failures_total",
            "Dependency failures by dependency and stable class",
        ),
        &["dependency", "class"],
    )
    .expect("metric aif_dependency_failures_total must be valid")
});

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
// VCAL evidence delivery metrics
// -----------------------------

pub static EVIDENCE_EVENTS_ENQUEUED_TOTAL: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new(
        "aif_evidence_events_enqueued_total",
        "VCAL evidence events accepted by the producer-side queue",
    )
    .expect("metric aif_evidence_events_enqueued_total must be valid")
});

pub static EVIDENCE_EVENTS_DROPPED_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_evidence_events_dropped_total",
            "VCAL evidence events dropped by reason",
        ),
        &["reason"],
    )
    .expect("metric aif_evidence_events_dropped_total must be valid")
});

pub static EVIDENCE_EVENTS_DELIVERED_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_evidence_events_delivered_total",
            "VCAL evidence events processed by HTTP delivery result",
        ),
        &["result"],
    )
    .expect("metric aif_evidence_events_delivered_total must be valid")
});

pub static EVIDENCE_BATCHES_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_evidence_batches_total",
            "VCAL evidence HTTP batches by delivery result",
        ),
        &["result"],
    )
    .expect("metric aif_evidence_batches_total must be valid")
});

pub static EVIDENCE_QUEUE_DEPTH: Lazy<IntGauge> = Lazy::new(|| {
    IntGauge::new(
        "aif_evidence_queue_depth",
        "Current number of VCAL evidence events waiting in the producer-side queue",
    )
    .expect("metric aif_evidence_queue_depth must be valid")
});

pub static EVIDENCE_DELIVERY_LATENCY_SECONDS: Lazy<Histogram> = Lazy::new(|| {
    Histogram::with_opts(HistogramOpts::new(
        "aif_evidence_delivery_latency_seconds",
        "Time spent delivering a VCAL evidence batch, including retries",
    ))
    .expect("metric aif_evidence_delivery_latency_seconds must be valid")
});

pub static EVIDENCE_RETRIES_TOTAL: Lazy<IntCounter> = Lazy::new(|| {
    IntCounter::new(
        "aif_evidence_retries_total",
        "VCAL evidence HTTP delivery retries",
    )
    .expect("metric aif_evidence_retries_total must be valid")
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

// -----------------------------
// VCAL Guard orchestration metrics
// -----------------------------

pub static GUARD_HOOK_CALLS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_guard_hook_calls_total",
            "AI Firewall guard orchestration hook calls by guard, phase, and outcome",
        ),
        &["guard", "phase", "outcome"],
    )
    .expect("metric aif_guard_hook_calls_total must be valid")
});

pub static GUARD_HOOK_ERRORS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_guard_hook_errors_total",
            "AI Firewall guard orchestration hook errors by guard and phase",
        ),
        &["guard", "phase"],
    )
    .expect("metric aif_guard_hook_errors_total must be valid")
});

pub static GUARD_REJECTIONS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_guard_rejections_total",
            "Requests rejected by AI Firewall guard orchestration",
        ),
        &["guard"],
    )
    .expect("metric aif_guard_rejections_total must be valid")
});

pub static GUARD_FINDINGS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_guard_findings_total",
            "Privacy/security findings reported to AI Firewall by guard modules",
        ),
        &["guard", "kind", "severity"],
    )
    .expect("metric aif_guard_findings_total must be valid")
});

pub static GUARD_TRANSFORMATIONS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_guard_transformations_total",
            "Request/response transformations performed by AI Firewall guard orchestration",
        ),
        &["guard", "phase", "action"],
    )
    .expect("metric aif_guard_transformations_total must be valid")
});

pub static GUARD_MAPPINGS_CREATED_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_guard_mappings_created_total",
            "Placeholder mappings created by guard modules and tracked by AI Firewall",
        ),
        &["guard"],
    )
    .expect("metric aif_guard_mappings_created_total must be valid")
});

pub static GUARD_NON_STRING_CONTENT_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_guard_non_string_content_total",
            "Non-string OpenAI message content left unchanged by guard adapters",
        ),
        &["guard", "phase"],
    )
    .expect("metric aif_guard_non_string_content_total must be valid")
});

pub static GUARD_REQUESTS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_guard_requests_total",
            "AI Firewall guard orchestration requests by guard, stage, and result",
        ),
        &["guard", "stage", "result"],
    )
    .expect("metric aif_guard_requests_total must be valid")
});

pub static GUARD_LATENCY_SECONDS: Lazy<HistogramVec> = Lazy::new(|| {
    HistogramVec::new(
        HistogramOpts::new(
            "aif_guard_latency_seconds",
            "AI Firewall guard orchestration latency in seconds by guard and stage",
        ),
        &["guard", "stage"],
    )
    .expect("metric aif_guard_latency_seconds must be valid")
});

pub static SECURITY_BLOCKS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_security_blocks_total",
            "Security Guard blocks observed by AI Firewall by stage and rule ID",
        ),
        &["stage", "rule_id"],
    )
    .expect("metric aif_security_blocks_total must be valid")
});

pub static PRIVACY_RESTORE_SKIPPED_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    IntCounterVec::new(
        prometheus::Opts::new(
            "aif_privacy_restore_skipped_total",
            "Privacy Guard restore operations skipped by AI Firewall by reason",
        ),
        &["reason"],
    )
    .expect("metric aif_privacy_restore_skipped_total must be valid")
});

pub fn observe_guard_request(guard: &str, stage: &str, result: &str) {
    GUARD_REQUESTS_TOTAL
        .with_label_values(&[guard, stage, result])
        .inc();
}

pub fn observe_guard_latency_seconds(guard: &str, stage: &str, seconds: f64) {
    GUARD_LATENCY_SECONDS
        .with_label_values(&[guard, stage])
        .observe(seconds);
}

pub fn observe_security_block(stage: &str, rule_id: Option<&str>) {
    SECURITY_BLOCKS_TOTAL
        .with_label_values(&[stage, rule_id.unwrap_or("unknown")])
        .inc();
}

pub fn observe_privacy_restore_skipped(reason: &str) {
    PRIVACY_RESTORE_SKIPPED_TOTAL
        .with_label_values(&[reason])
        .inc();
}

pub fn observe_backpressure_rejection(scope: &str) {
    BACKPRESSURE_REJECTIONS_TOTAL
        .with_label_values(&[scope])
        .inc();
}

pub fn evidence_delivery_snapshot() -> (i64, u64, u64, u64) {
    (
        EVIDENCE_QUEUE_DEPTH.get(),
        EVIDENCE_EVENTS_ENQUEUED_TOTAL.get(),
        EVIDENCE_EVENTS_DROPPED_TOTAL
            .with_label_values(&["queue_full"])
            .get()
            + EVIDENCE_EVENTS_DROPPED_TOTAL
                .with_label_values(&["queue_closed"])
                .get()
            + EVIDENCE_EVENTS_DROPPED_TOTAL
                .with_label_values(&["retry_exhausted"])
                .get(),
        EVIDENCE_EVENTS_DELIVERED_TOTAL
            .with_label_values(&["delivered"])
            .get(),
    )
}

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
    Lazy::force(&GUARD_HOOK_CALLS_TOTAL);
    Lazy::force(&GUARD_HOOK_ERRORS_TOTAL);
    Lazy::force(&GUARD_REJECTIONS_TOTAL);
    Lazy::force(&GUARD_FINDINGS_TOTAL);
    Lazy::force(&GUARD_TRANSFORMATIONS_TOTAL);
    Lazy::force(&GUARD_MAPPINGS_CREATED_TOTAL);
    Lazy::force(&GUARD_NON_STRING_CONTENT_TOTAL);
    Lazy::force(&GUARD_REQUESTS_TOTAL);
    Lazy::force(&GUARD_LATENCY_SECONDS);
    Lazy::force(&SECURITY_BLOCKS_TOTAL);
    Lazy::force(&PRIVACY_RESTORE_SKIPPED_TOTAL);
    Lazy::force(&EVIDENCE_EVENTS_ENQUEUED_TOTAL);
    Lazy::force(&EVIDENCE_EVENTS_DROPPED_TOTAL);
    Lazy::force(&EVIDENCE_EVENTS_DELIVERED_TOTAL);
    Lazy::force(&EVIDENCE_BATCHES_TOTAL);
    Lazy::force(&EVIDENCE_QUEUE_DEPTH);
    Lazy::force(&EVIDENCE_DELIVERY_LATENCY_SECONDS);
    Lazy::force(&EVIDENCE_RETRIES_TOTAL);
    Lazy::force(&EMBEDDING_TIMEOUTS_TOTAL);
    Lazy::force(&EMBEDDING_REQUEST_DURATION_SECONDS);
    Lazy::force(&BACKPRESSURE_REJECTIONS_TOTAL);
    Lazy::force(&DEPENDENCY_FAILURES_TOTAL);
    Lazy::force(&MODEL_REQUESTS_TOTAL);
    Lazy::force(&MODEL_INPUT_TOKENS_TOTAL);
    Lazy::force(&MODEL_OUTPUT_TOKENS_TOTAL);
    Lazy::force(&MODEL_COST_MICRO_USD_TOTAL);
    Lazy::force(&CACHE_HITS_TOTAL);
    Lazy::force(&CACHE_BYPASS_REQUESTS_TOTAL);
    Lazy::force(&REQUEST_COST_MICRO_USD_TOTAL);
    Lazy::force(&GROSS_SAVED_MICRO_USD_TOTAL);
    Lazy::force(&NET_SAVED_MICRO_USD_TOTAL);
    Lazy::force(&EMBEDDING_OVERHEAD_MICRO_USD_TOTAL);

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
            Box::new(CACHE_BYPASS_REQUESTS_TOTAL.clone()),
            Box::new(UPSTREAM_CALLS.clone()),
            Box::new(TOKENS_SAVED.clone()),
            Box::new(CHAT_COST_SAVED_MICRO_USD.clone()),
            Box::new(EMBEDDING_COST_MICRO_USD.clone()),
            Box::new(COST_SAVED_MICRO_USD.clone()),
            Box::new(MODEL_REQUESTS_TOTAL.clone()),
            Box::new(MODEL_INPUT_TOKENS_TOTAL.clone()),
            Box::new(MODEL_OUTPUT_TOKENS_TOTAL.clone()),
            Box::new(MODEL_COST_MICRO_USD_TOTAL.clone()),
            Box::new(CACHE_HITS_TOTAL.clone()),
            Box::new(REQUEST_COST_MICRO_USD_TOTAL.clone()),
            Box::new(GROSS_SAVED_MICRO_USD_TOTAL.clone()),
            Box::new(NET_SAVED_MICRO_USD_TOTAL.clone()),
            Box::new(EMBEDDING_OVERHEAD_MICRO_USD_TOTAL.clone()),
            Box::new(INFLIGHT_REQUESTS.clone()),
            Box::new(ERRORS_TOTAL.clone()),
            Box::new(BACKPRESSURE_REJECTIONS_TOTAL.clone()),
            Box::new(DEPENDENCY_FAILURES_TOTAL.clone()),
            Box::new(READINESS_STATE.clone()),
            Box::new(SHUTDOWN_IN_PROGRESS.clone()),
            Box::new(SHUTDOWN_REJECTIONS_TOTAL.clone()),
            Box::new(UPSTREAM_TIMEOUTS_TOTAL.clone()),
            Box::new(UPSTREAM_REQUEST_DURATION_SECONDS.clone()),
            Box::new(SEMANTIC_CANDIDATES_CHECKED_TOTAL.clone()),
            Box::new(SEMANTIC_THRESHOLD_RESULTS_TOTAL.clone()),
            Box::new(SEMANTIC_EXPIRED_ENTRIES_SKIPPED_TOTAL.clone()),
            Box::new(SEMANTIC_LOOKUP_DURATION_SECONDS.clone()),
            Box::new(GUARD_HOOK_CALLS_TOTAL.clone()),
            Box::new(GUARD_HOOK_ERRORS_TOTAL.clone()),
            Box::new(GUARD_REJECTIONS_TOTAL.clone()),
            Box::new(GUARD_FINDINGS_TOTAL.clone()),
            Box::new(GUARD_TRANSFORMATIONS_TOTAL.clone()),
            Box::new(GUARD_MAPPINGS_CREATED_TOTAL.clone()),
            Box::new(GUARD_NON_STRING_CONTENT_TOTAL.clone()),
            Box::new(GUARD_REQUESTS_TOTAL.clone()),
            Box::new(GUARD_LATENCY_SECONDS.clone()),
            Box::new(SECURITY_BLOCKS_TOTAL.clone()),
            Box::new(PRIVACY_RESTORE_SKIPPED_TOTAL.clone()),
            Box::new(EVIDENCE_EVENTS_ENQUEUED_TOTAL.clone()),
            Box::new(EVIDENCE_EVENTS_DROPPED_TOTAL.clone()),
            Box::new(EVIDENCE_EVENTS_DELIVERED_TOTAL.clone()),
            Box::new(EVIDENCE_BATCHES_TOTAL.clone()),
            Box::new(EVIDENCE_QUEUE_DEPTH.clone()),
            Box::new(EVIDENCE_DELIVERY_LATENCY_SECONDS.clone()),
            Box::new(EVIDENCE_RETRIES_TOTAL.clone()),
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
