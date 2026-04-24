use std::{sync::Arc, time::Instant};

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use qdrant_client::{
    qdrant::{
        points_selector::PointsSelectorOneOf, vectors_config::Config, Condition, CreateCollection,
        DeletePoints, Distance, FieldCondition, Filter, Match, PointStruct, PointsSelector, Range,
        SearchPoints, UpsertPoints, Value, VectorParams, VectorsConfig,
    },
    Qdrant,
};
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::{
    core::hashing::sha256_hex,
    embeddings::provider::EmbeddingProvider,
    metrics,
    semantic::semantic_cache::{SemanticCache, SemanticLookupHit},
    types::{openai::ChatCompletionResponse, semantic::SemanticCacheRecord},
};

pub struct QdrantSemanticCache {
    client: Qdrant,
    embedder: Arc<dyn EmbeddingProvider>,
    collection_name: String,
    similarity_threshold: f32,
    semantic_retention_seconds: usize,
}

impl QdrantSemanticCache {
    pub async fn new(
        qdrant_url: String,
        qdrant_api_key: Option<String>,
        collection_name: String,
        vector_size: u64,
        similarity_threshold: f32,
        semantic_retention_seconds: usize,
        embedder: Arc<dyn EmbeddingProvider>,
    ) -> Result<Self> {
        let mut builder = Qdrant::from_url(&qdrant_url);
        if let Some(api_key) = qdrant_api_key {
            builder = builder.api_key(api_key);
        }
        let client = builder.build().context("failed to build Qdrant client")?;

        ensure_collection(&client, &collection_name, vector_size).await?;

        Ok(Self {
            client,
            embedder,
            collection_name,
            similarity_threshold,
            semantic_retention_seconds,
        })
    }
}

async fn ensure_collection(client: &Qdrant, collection_name: &str, vector_size: u64) -> Result<()> {
    let collections = client.list_collections().await?;
    let exists = collections
        .collections
        .iter()
        .any(|c| c.name == collection_name);

    if exists {
        return Ok(());
    }

    client
        .create_collection(CreateCollection {
            collection_name: collection_name.to_string(),
            vectors_config: Some(VectorsConfig {
                config: Some(Config::Params(VectorParams {
                    size: vector_size,
                    distance: Distance::Cosine.into(),
                    ..Default::default()
                })),
            }),
            ..Default::default()
        })
        .await
        .with_context(|| format!("failed creating Qdrant collection {}", collection_name))?;

    Ok(())
}

#[async_trait]
impl SemanticCache for QdrantSemanticCache {
    async fn lookup(
        &self,
        model: &str,
        normalized_prompt: &str,
    ) -> Result<Option<SemanticLookupHit>> {
        let started = Instant::now();

        let result = async {
            let embedding_result = self.embedder.embed_text(normalized_prompt).await?;
            let vector = embedding_result.embedding.clone();
            let embedding_usage = embedding_result.usage.clone();

            let search_result = self
                .client
                .search_points(SearchPoints {
                    collection_name: self.collection_name.clone(),
                    vector,
                    limit: 3,
                    with_payload: Some(true.into()),
                    filter: Some(Filter {
                        must: vec![Condition {
                            condition_one_of: Some(
                                qdrant_client::qdrant::condition::ConditionOneOf::Field(
                                    FieldCondition {
                                        key: "model".to_string(),
                                        r#match: Some(Match {
                                            match_value: Some(
                                                qdrant_client::qdrant::r#match::MatchValue::Keyword(
                                                    model.to_string(),
                                                ),
                                            ),
                                        }),
                                        ..Default::default()
                                    },
                                ),
                            ),
                        }],
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .await
                .context("Qdrant semantic search failed")?;

            let now = Utc::now().timestamp();

            for point in search_result.result {
                metrics::SEMANTIC_CANDIDATES_CHECKED_TOTAL.inc();

                let payload = point.payload;

                let expires_at = match payload.get("expires_at").and_then(proto_value_to_i64) {
                    Some(v) => v,
                    None => {
                        metrics::SEMANTIC_EXPIRED_ENTRIES_SKIPPED_TOTAL.inc();
                        tracing::debug!(
                            model = %model,
                            "semantic candidate skipped because expires_at is missing"
                        );
                        continue;
                    }
                };

                if expires_at <= now {
                    metrics::SEMANTIC_EXPIRED_ENTRIES_SKIPPED_TOTAL.inc();
                    tracing::debug!(
                        model = %model,
                        expires_at = expires_at,
                        now = now,
                        "semantic candidate skipped because it is expired"
                    );
                    continue;
                }

                let score = point.score;
                if score < self.similarity_threshold {
                    metrics::SEMANTIC_THRESHOLD_RESULTS_TOTAL
                        .with_label_values(&["fail"])
                        .inc();

                    tracing::debug!(
                        model = %model,
                        score = score,
                        threshold = self.similarity_threshold,
                        "semantic candidate rejected below threshold"
                    );

                    continue;
                }

                metrics::SEMANTIC_THRESHOLD_RESULTS_TOTAL
                    .with_label_values(&["pass"])
                    .inc();

                let raw_response = payload
                    .get("response_json")
                    .and_then(proto_value_to_json_string)
                    .context("missing response_json payload in semantic hit")?;

                let parsed: ChatCompletionResponse = serde_json::from_str(&raw_response)
                    .context("invalid cached semantic response")?;

                tracing::debug!(
                    model = %model,
                    score = score,
                    threshold = self.similarity_threshold,
                    "semantic hit"
                );

                return Ok(Some(SemanticLookupHit {
                    response: parsed,
                    embedding_usage: embedding_usage.clone(),
                }));
            }

            Ok(None)
        }
        .await;

        metrics::SEMANTIC_LOOKUP_DURATION_SECONDS.observe(started.elapsed().as_secs_f64());

        result
    }

    async fn store(
        &self,
        model: &str,
        normalized_prompt: &str,
        response: &ChatCompletionResponse,
    ) -> Result<()> {
        metrics::SEMANTIC_STORE_TOTAL.inc();

        let result = async {
            let embedding_result = self.embedder.embed_text(normalized_prompt).await?;
            let vector = embedding_result.embedding;

            let request_hash = sha256_hex(normalized_prompt);

            let inserted_at = Utc::now().timestamp();
            let expires_at = inserted_at + self.semantic_retention_seconds as i64;

            let record = SemanticCacheRecord {
                request_hash: request_hash.clone(),
                model: model.to_string(),
                normalized_prompt: normalized_prompt.to_string(),
                response: response.clone(),
                inserted_at,
                expires_at,
            };

            let response_json = serde_json::to_string(&record.response)
                .context("failed to serialize response_json")?;

            let point = PointStruct::new(
                Uuid::new_v4().to_string(),
                vector,
                [
                    (
                        "request_hash",
                        json_to_proto_value(JsonValue::String(record.request_hash)),
                    ),
                    (
                        "model",
                        json_to_proto_value(JsonValue::String(record.model)),
                    ),
                    (
                        "normalized_prompt",
                        json_to_proto_value(JsonValue::String(record.normalized_prompt)),
                    ),
                    (
                        "inserted_at",
                        json_to_proto_value(JsonValue::Number(record.inserted_at.into())),
                    ),
                    (
                        "expires_at",
                        json_to_proto_value(JsonValue::Number(record.expires_at.into())),
                    ),
                    (
                        "response_json",
                        json_to_proto_value(JsonValue::String(response_json)),
                    ),
                ],
            );

            self.client
                .upsert_points(UpsertPoints {
                    collection_name: self.collection_name.clone(),
                    points: vec![point],
                    wait: Some(false),
                    ..Default::default()
                })
                .await
                .context("Qdrant upsert failed")?;

            Ok(())
        }
        .await;

        if result.is_err() {
            metrics::SEMANTIC_STORE_ERRORS_TOTAL.inc();
        }

        result
    }
}

fn json_to_proto_value(v: JsonValue) -> Value {
    match v {
        JsonValue::Null => Value {
            kind: Some(qdrant_client::qdrant::value::Kind::NullValue(0)),
        },
        JsonValue::Bool(b) => Value {
            kind: Some(qdrant_client::qdrant::value::Kind::BoolValue(b)),
        },
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value {
                    kind: Some(qdrant_client::qdrant::value::Kind::IntegerValue(i)),
                }
            } else if let Some(f) = n.as_f64() {
                Value {
                    kind: Some(qdrant_client::qdrant::value::Kind::DoubleValue(f)),
                }
            } else {
                Value {
                    kind: Some(qdrant_client::qdrant::value::Kind::NullValue(0)),
                }
            }
        }
        JsonValue::String(s) => Value {
            kind: Some(qdrant_client::qdrant::value::Kind::StringValue(s)),
        },
        JsonValue::Array(_) | JsonValue::Object(_) => Value {
            kind: Some(qdrant_client::qdrant::value::Kind::StringValue(
                v.to_string(),
            )),
        },
    }
}

fn proto_value_to_json_string(v: &Value) -> Option<String> {
    match &v.kind {
        Some(qdrant_client::qdrant::value::Kind::StringValue(s)) => Some(s.clone()),
        _ => None,
    }
}

fn proto_value_to_i64(v: &Value) -> Option<i64> {
    match &v.kind {
        Some(qdrant_client::qdrant::value::Kind::IntegerValue(i)) => Some(*i),
        Some(qdrant_client::qdrant::value::Kind::DoubleValue(f)) => Some(*f as i64),
        _ => None,
    }
}

pub async fn prune_expired_semantic_cache_entries(
    qdrant_url: String,
    qdrant_api_key: Option<String>,
    collection_name: String,
) -> Result<()> {
    let mut builder = Qdrant::from_url(&qdrant_url);

    if let Some(api_key) = qdrant_api_key {
        builder = builder.api_key(api_key);
    }

    let client = builder.build().context("failed to build Qdrant client")?;

    let now = Utc::now().timestamp();

    tracing::info!(
        collection = %collection_name,
        now = now,
        "pruning expired semantic cache entries from Qdrant"
    );

    client
        .delete_points(DeletePoints {
            collection_name: collection_name.clone(),
            points: Some(PointsSelector {
                points_selector_one_of: Some(PointsSelectorOneOf::Filter(Filter {
                    must: vec![Condition {
                        condition_one_of: Some(
                            qdrant_client::qdrant::condition::ConditionOneOf::Field(
                                FieldCondition {
                                    key: "expires_at".to_string(),
                                    range: Some(Range {
                                        lt: Some(now as f64),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                },
                            ),
                        ),
                    }],
                    ..Default::default()
                })),
            }),
            wait: Some(true),
            ..Default::default()
        })
        .await
        .with_context(|| {
            format!(
                "failed to prune expired semantic cache entries from Qdrant collection {}",
                collection_name
            )
        })?;

    tracing::info!(
        collection = %collection_name,
        "expired semantic cache pruning completed; exact deleted count is not reported"
    );

    Ok(())
}
