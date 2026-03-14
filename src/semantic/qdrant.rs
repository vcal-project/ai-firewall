use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use qdrant_client::{
    qdrant::{
        vectors_config::Config, Condition, CreateCollection, Distance, FieldCondition, Filter,
        Match, PointStruct, SearchPoints, UpsertPoints, Value, VectorParams, VectorsConfig,
    },
    Qdrant,
};
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::{
    core::hashing::sha256_hex,
    embeddings::provider::EmbeddingProvider,
    semantic::semantic_cache::SemanticCache,
    types::{openai::ChatCompletionResponse, semantic::SemanticCacheRecord},
};

pub struct QdrantSemanticCache {
    client: Qdrant,
    embedder: Arc<dyn EmbeddingProvider>,
    collection_name: String,
    similarity_threshold: f32,
}

impl QdrantSemanticCache {
    pub async fn new(
        qdrant_url: String,
        qdrant_api_key: Option<String>,
        collection_name: String,
        vector_size: u64,
        similarity_threshold: f32,
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
    ) -> Result<Option<ChatCompletionResponse>> {
        let vector = self.embedder.embed_text(normalized_prompt).await?;

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

        for point in search_result.result {
            let score = point.score;
            if score < self.similarity_threshold {
                continue;
            }

            let payload = point.payload;
            let raw_response = payload
                .get("response_json")
                .and_then(proto_value_to_json_string)
                .context("missing response_json payload in semantic hit")?;

            let parsed: ChatCompletionResponse =
                serde_json::from_str(&raw_response).context("invalid cached semantic response")?;

            tracing::debug!("semantic hit with score={score:.4}");
            return Ok(Some(parsed));
        }

        Ok(None)
    }

    async fn store(
        &self,
        model: &str,
        normalized_prompt: &str,
        response: &ChatCompletionResponse,
    ) -> Result<()> {
        let vector = self.embedder.embed_text(normalized_prompt).await?;

        let request_hash = sha256_hex(normalized_prompt);

        let record = SemanticCacheRecord {
            request_hash: request_hash.clone(),
            model: model.to_string(),
            normalized_prompt: normalized_prompt.to_string(),
            response: response.clone(),
            created_at_unix: Utc::now().timestamp(),
        };

        let response_json =
            serde_json::to_string(&record.response).context("failed to serialize response_json")?;

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
                    "created_at_unix",
                    json_to_proto_value(JsonValue::Number(record.created_at_unix.into())),
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
