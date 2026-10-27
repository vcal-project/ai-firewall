//! Optional, additive trust roots for HTTP clients talking to private providers.
//!
//! The default reqwest/rustls WebPKI roots remain enabled. Only explicitly
//! configured upstream and embedding clients receive additional CA roots.

use anyhow::{ensure, Context, Result};
use reqwest::{Certificate, ClientBuilder};
use std::fs;

/// Read a PEM CA bundle once at client initialization. A missing setting leaves
/// reqwest's bundled root store unchanged. A configured but unusable bundle
/// fails closed; never silently drop the additional trust roots.
pub fn load_custom_ca_roots(path: Option<&str>, directive: &str) -> Result<Vec<Certificate>> {
    let Some(path) = path else {
        return Ok(Vec::new());
    };

    ensure!(
        !path.trim().is_empty(),
        "{directive} must not be empty when configured"
    );

    let pem =
        fs::read(path).with_context(|| format!("{directive} could not read CA bundle '{path}'"))?;
    ensure!(!pem.is_empty(), "{directive} CA bundle '{path}' is empty");

    let roots = Certificate::from_pem_bundle(&pem).with_context(|| {
        format!("{directive} CA bundle '{path}' contains invalid PEM certificates")
    })?;
    ensure!(
        !roots.is_empty(),
        "{directive} CA bundle '{path}' contains no PEM certificates"
    );

    Ok(roots)
}

/// Add roots without replacing the default WebPKI roots or disabling hostname
/// and certificate verification. Cloning allows buffered and streaming clients
/// to use the exact same certificate snapshot after a single filesystem read.
pub fn with_custom_ca_roots(mut builder: ClientBuilder, roots: &[Certificate]) -> ClientBuilder {
    for root in roots {
        builder = builder.add_root_certificate(root.clone());
    }
    builder
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn fixture_ca_path() -> &'static str {
        concat!(env!("CARGO_MANIFEST_DIR"), "/src/config/testdata/ca.pem")
    }

    #[test]
    fn unset_ca_bundle_keeps_default_client_builder_valid() {
        let roots = load_custom_ca_roots(None, "upstream_ca_bundle").unwrap();
        assert!(roots.is_empty());
        with_custom_ca_roots(reqwest::Client::builder(), &roots)
            .build()
            .unwrap();
    }

    #[test]
    fn pem_bundle_adds_roots_to_multiple_clients() {
        let roots = load_custom_ca_roots(Some(fixture_ca_path()), "upstream_ca_bundle").unwrap();
        assert_eq!(roots.len(), 1);
        // Both clients are built from the same set of parsed roots.
        with_custom_ca_roots(reqwest::Client::builder(), &roots)
            .build()
            .unwrap();
        with_custom_ca_roots(reqwest::Client::builder(), &roots)
            .build()
            .unwrap();
    }

    #[test]
    fn multiple_pem_certificates_are_supported() {
        let pem = include_bytes!("config/testdata/ca.pem");
        let parsed =
            Certificate::from_pem_bundle(&[pem.as_slice(), pem.as_slice()].concat()).unwrap();
        assert_eq!(parsed.len(), 2);
        with_custom_ca_roots(reqwest::Client::builder(), &parsed)
            .build()
            .unwrap();
    }

    #[test]
    fn missing_or_blank_ca_bundle_fails_closed() {
        let err = load_custom_ca_roots(Some("  "), "upstream_ca_bundle")
            .err()
            .unwrap();
        assert!(err.to_string().contains("must not be empty"));
        let missing = Path::new(env!("CARGO_MANIFEST_DIR")).join("nonexistent-ca-bundle.pem");
        let err = load_custom_ca_roots(missing.to_str(), "embedding_ca_bundle")
            .err()
            .unwrap();
        assert!(err
            .to_string()
            .contains("embedding_ca_bundle could not read"));
    }

    #[test]
    fn non_certificate_pem_fails_closed() {
        // Parsing malformed in-memory data must not result in a trusted client.
        let result = Certificate::from_pem_bundle(b"not a PEM certificate");
        assert!(result.is_err() || result.unwrap().is_empty());
    }
}

// Real TLS handshakes without external services, containers, or openssl at test
// runtime. The test-only certificate is signed by a private CA and has a DNS
// SAN of localhost (not 127.0.0.1). Never ship its private key in an image.
#[cfg(test)]
mod loopback_tls_tests {
    use crate::embeddings::openai::OpenAiEmbeddingProvider;
    use crate::embeddings::provider::EmbeddingProvider;
    use crate::types::openai::{ChatCompletionRequest, ChatMessage};
    use crate::upstream::llm::LlmUpstream;
    use crate::upstream::openai::OpenAiUpstream;
    use futures_util::StreamExt;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use tokio_rustls::rustls::ServerConfig;
    use tokio_rustls::TlsAcceptor;

    const CERT_DER: &[u8] = include_bytes!("../tests/tls-fixtures/server.der");
    const KEY_DER: &[u8] = include_bytes!("../tests/tls-fixtures/server-key.der");
    const CA_PEM: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/tls-fixtures/ca.pem");

    async fn start_loopback_https() -> u16 {
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(KEY_DER.to_vec()));
        let certs = vec![CertificateDer::from(CERT_DER.to_vec())];
        // Both `aws_lc_rs` and `ring` may be enabled via the dependency graph.
        // Select the provider for this test server explicitly instead of relying
        // on a process-global default (which can be ambiguous in test builds).
        let provider = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider();
        let config = ServerConfig::builder_with_provider(std::sync::Arc::new(provider))
            .with_safe_default_protocol_versions()
            .expect("test TLS provider must support default protocol versions")
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .expect("fixture certificate and key must match");
        let acceptor = TlsAcceptor::from(std::sync::Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    if let Ok(mut stream) = acceptor.accept(socket).await {
                        let mut received = Vec::new();
                        let mut buf = [0u8; 4096];
                        // Read complete HTTP/1.1 headers and any request body
                        // before responding, to avoid truncating a POST upload.
                        while received.len() < 128 * 1024 {
                            let Ok(n) = stream.read(&mut buf).await else {
                                return;
                            };
                            if n == 0 {
                                return;
                            }
                            received.extend_from_slice(&buf[..n]);
                            if let Some(end) = received.windows(4).position(|w| w == b"\r\n\r\n") {
                                let header_end = end + 4;
                                let headers = String::from_utf8_lossy(&received[..header_end]);
                                let len = headers
                                    .lines()
                                    .find_map(|line| {
                                        line.split_once(':')
                                            .filter(|(name, _)| {
                                                name.eq_ignore_ascii_case("content-length")
                                            })
                                            .and_then(|(_, value)| {
                                                value.trim().parse::<usize>().ok()
                                            })
                                    })
                                    .unwrap_or(0);
                                if received.len() >= header_end + len {
                                    break;
                                }
                            }
                        }
                        let path = String::from_utf8_lossy(&received);
                        let (kind, body) = if path.starts_with("POST /v1/embeddings ") {
                            (
                                "application/json",
                                r#"{"data":[{"embedding":[0.1,0.2,0.3]}],"model":"test-embedding","usage":{"prompt_tokens":2,"total_tokens":2}}"#,
                            )
                        } else if path.starts_with("POST /v1/chat/completions ") {
                            if path.contains("\"stream\":true") {
                                (
                                    "text/event-stream",
                                    "data: {\"choices\":[]}\n\ndata: [DONE]\n\n",
                                )
                            } else {
                                (
                                    "application/json",
                                    r#"{"id":"chatcmpl-test","object":"chat.completion","created":1700000000,"model":"test-chat","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#,
                                )
                            }
                        } else {
                            (
                                "application/json",
                                r#"{"object":"list","data":[{"id":"test-chat","object":"model"}]}"#,
                            )
                        };
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                        let _ = stream.shutdown().await;
                    }
                });
            }
        });
        port
    }

    fn upstream(url: String, ca: Option<&str>) -> OpenAiUpstream {
        OpenAiUpstream::new(url, "none".into(), Duration::from_secs(5), ca).unwrap()
    }

    fn embedding(url: String, ca: Option<&str>) -> OpenAiEmbeddingProvider {
        OpenAiEmbeddingProvider::new(
            url,
            "none".into(),
            "test-embedding".into(),
            Duration::from_secs(5),
            ca,
        )
        .unwrap()
    }

    fn chat_request(streaming: bool) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "test-chat".into(),
            messages: vec![ChatMessage {
                role: "user".into(),
                content: serde_json::Value::String("hello".into()),
                name: None,
                extra: Default::default(),
            }],
            temperature: None,
            top_p: None,
            max_tokens: None,
            stream: Some(streaming),
            extra: Default::default(),
        }
    }

    #[tokio::test]
    async fn configured_ca_verifies_upstream_and_embedding_and_streaming() {
        let port = start_loopback_https().await;
        let url = format!("https://localhost:{port}/v1");
        let upstream = upstream(url.clone(), Some(CA_PEM));
        let models = upstream.list_models().await.expect("verified /v1/models");
        assert_eq!(models["data"][0]["id"], "test-chat");

        let response = upstream
            .chat_completion(&chat_request(false))
            .await
            .expect("buffered chat client must trust its configured CA");
        assert_eq!(response.model, "test-chat");

        let mut stream = upstream
            .chat_completion_stream(&chat_request(true))
            .await
            .expect("streaming client must trust exactly the same CA")
            .body;
        let mut sse_body = Vec::new();
        while let Some(chunk) = stream.next().await {
            sse_body.extend_from_slice(&chunk.expect("TLS SSE body must remain readable"));
        }
        assert!(String::from_utf8_lossy(&sse_body).contains("data:"));

        let embedded = embedding(url, Some(CA_PEM))
            .embed_text("hello")
            .await
            .expect("embedding client must trust its configured CA");
        assert_eq!(embedded.embedding.len(), 3);
    }

    #[tokio::test]
    async fn default_roots_do_not_trust_private_ca() {
        let port = start_loopback_https().await;
        let url = format!("https://localhost:{port}/v1");
        assert!(upstream(url.clone(), None).list_models().await.is_err());
        assert!(upstream(url.clone(), None)
            .chat_completion(&chat_request(false))
            .await
            .is_err());
        assert!(upstream(url.clone(), None)
            .chat_completion_stream(&chat_request(true))
            .await
            .is_err());
        assert!(embedding(url, None).embed_text("hello").await.is_err());
    }

    #[tokio::test]
    async fn custom_ca_does_not_disable_hostname_verification() {
        let port = start_loopback_https().await;
        // The fixture only has DNS:localhost, not IP:127.0.0.1.
        let ip_url = format!("https://127.0.0.1:{port}/v1");
        assert!(upstream(ip_url.clone(), Some(CA_PEM))
            .list_models()
            .await
            .is_err());
        assert!(upstream(ip_url.clone(), Some(CA_PEM))
            .chat_completion(&chat_request(false))
            .await
            .is_err());
        assert!(upstream(ip_url.clone(), Some(CA_PEM))
            .chat_completion_stream(&chat_request(true))
            .await
            .is_err());
        assert!(embedding(ip_url, Some(CA_PEM))
            .embed_text("hello")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn wrong_ca_does_not_trust_the_private_server() {
        let port = start_loopback_https().await;
        let url = format!("https://localhost:{port}/v1");
        let unrelated_ca = concat!(env!("CARGO_MANIFEST_DIR"), "/src/config/testdata/ca.pem");
        assert!(upstream(url.clone(), Some(unrelated_ca))
            .list_models()
            .await
            .is_err());
        assert!(upstream(url.clone(), Some(unrelated_ca))
            .chat_completion(&chat_request(false))
            .await
            .is_err());
        assert!(embedding(url, Some(unrelated_ca))
            .embed_text("hello")
            .await
            .is_err());
    }
}
