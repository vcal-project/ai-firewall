use std::{sync::Arc, time::Duration};

use anyhow::{anyhow, Context};
use reqwest::{header::RETRY_AFTER, Client, StatusCode};
use serde::Serialize;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::{interval, sleep, timeout, Instant, MissedTickBehavior},
};

use crate::{metrics, release};

use super::{EvidenceEvent, EvidenceSink, EVIDENCE_SCHEMA_VERSION};

#[derive(Clone, Debug)]
pub struct BufferedHttpEvidenceSettings {
    pub endpoint: String,
    pub api_key: Option<String>,
    pub producer_instance_id: String,
    pub queue_capacity: usize,
    pub batch_size: usize,
    pub flush_interval: Duration,
    pub request_timeout: Duration,
    pub retry_max_attempts: usize,
    pub retry_initial_backoff: Duration,
    pub retry_max_backoff: Duration,
}

impl BufferedHttpEvidenceSettings {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.endpoint.trim().is_empty() {
            anyhow::bail!("VCAL Audit endpoint must not be empty");
        }
        if self.queue_capacity == 0 {
            anyhow::bail!("VCAL Audit queue_capacity must be greater than zero");
        }
        if self.batch_size == 0 {
            anyhow::bail!("VCAL Audit batch_size must be greater than zero");
        }
        if self.batch_size > self.queue_capacity {
            anyhow::bail!("VCAL Audit batch_size must not exceed queue_capacity");
        }
        if self.flush_interval.is_zero() {
            anyhow::bail!("VCAL Audit flush_interval must be greater than zero");
        }
        if self.request_timeout.is_zero() {
            anyhow::bail!("VCAL Audit request_timeout must be greater than zero");
        }
        if self.retry_max_attempts == 0 {
            anyhow::bail!("VCAL Audit retry_max_attempts must be greater than zero");
        }
        if self.retry_initial_backoff.is_zero() {
            anyhow::bail!("VCAL Audit retry_initial_backoff must be greater than zero");
        }
        if self.retry_max_backoff.is_zero() {
            anyhow::bail!("VCAL Audit retry_max_backoff must be greater than zero");
        }
        if self.retry_max_backoff < self.retry_initial_backoff {
            anyhow::bail!("VCAL Audit retry_max_backoff must be >= retry_initial_backoff");
        }

        Ok(())
    }
}

#[derive(Clone)]
pub struct BufferedHttpEvidenceSink {
    sender: mpsc::Sender<EvidenceEvent>,
}

pub struct BufferedHttpEvidenceHandle {
    shutdown: Option<oneshot::Sender<()>>,
    worker: JoinHandle<()>,
}

#[derive(Serialize)]
struct EvidenceProducer<'a> {
    product: &'a str,
    version: &'a str,
    instance_id: &'a str,
}

#[derive(Serialize)]
struct EvidenceBatch<'a> {
    schema_version: &'a str,
    producer: EvidenceProducer<'a>,
    events: &'a [EvidenceEvent],
}

enum DeliveryFailure {
    Retryable {
        error: anyhow::Error,
        retry_after: Option<Duration>,
    },
    Permanent(anyhow::Error),
}

impl BufferedHttpEvidenceSink {
    pub fn spawn(
        settings: BufferedHttpEvidenceSettings,
    ) -> anyhow::Result<(Arc<Self>, BufferedHttpEvidenceHandle)> {
        settings.validate()?;

        let client = Client::builder()
            .timeout(settings.request_timeout)
            .build()
            .context("failed to build VCAL Audit HTTP client")?;
        let (sender, receiver) = mpsc::channel(settings.queue_capacity);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let worker = tokio::spawn(run_worker(client, settings, receiver, shutdown_rx));

        Ok((
            Arc::new(Self { sender }),
            BufferedHttpEvidenceHandle {
                shutdown: Some(shutdown_tx),
                worker,
            },
        ))
    }
}

#[async_trait::async_trait]
impl EvidenceSink for BufferedHttpEvidenceSink {
    async fn emit(&self, event: EvidenceEvent) -> anyhow::Result<()> {
        match self.sender.try_send(event) {
            Ok(()) => {
                metrics::EVIDENCE_EVENTS_ENQUEUED_TOTAL.inc();
                metrics::EVIDENCE_QUEUE_DEPTH.inc();
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                metrics::EVIDENCE_EVENTS_DROPPED_TOTAL
                    .with_label_values(&["queue_full"])
                    .inc();
                Err(anyhow!("VCAL Audit evidence queue is full"))
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                metrics::EVIDENCE_EVENTS_DROPPED_TOTAL
                    .with_label_values(&["queue_closed"])
                    .inc();
                Err(anyhow!("VCAL Audit evidence queue is closed"))
            }
        }
    }
}

impl BufferedHttpEvidenceHandle {
    pub async fn shutdown(mut self, flush_timeout: Duration) -> anyhow::Result<()> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }

        timeout(flush_timeout, &mut self.worker)
            .await
            .map_err(|_| anyhow!("timed out flushing VCAL Audit evidence queue"))?
            .map_err(|error| anyhow!("VCAL Audit evidence worker failed: {error}"))?;

        Ok(())
    }
}

async fn run_worker(
    client: Client,
    settings: BufferedHttpEvidenceSettings,
    mut receiver: mpsc::Receiver<EvidenceEvent>,
    mut shutdown: oneshot::Receiver<()>,
) {
    let mut batch = Vec::with_capacity(settings.batch_size);
    let mut ticker = interval(settings.flush_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ticker.tick().await;

    loop {
        tokio::select! {
            biased;

            _ = &mut shutdown => {
                while let Ok(event) = receiver.try_recv() {
                    metrics::EVIDENCE_QUEUE_DEPTH.dec();
                    batch.push(event);
                    if batch.len() >= settings.batch_size {
                        flush_batch(&client, &settings, &mut batch).await;
                    }
                }
                flush_batch(&client, &settings, &mut batch).await;
                break;
            }
            event = receiver.recv() => {
                match event {
                    Some(event) => {
                        metrics::EVIDENCE_QUEUE_DEPTH.dec();
                        batch.push(event);
                        if batch.len() >= settings.batch_size {
                            flush_batch(&client, &settings, &mut batch).await;
                        }
                    }
                    None => {
                        flush_batch(&client, &settings, &mut batch).await;
                        break;
                    }
                }
            }
            _ = ticker.tick() => {
                flush_batch(&client, &settings, &mut batch).await;
            }
        }
    }
}

async fn flush_batch(
    client: &Client,
    settings: &BufferedHttpEvidenceSettings,
    batch: &mut Vec<EvidenceEvent>,
) {
    if batch.is_empty() {
        return;
    }

    let events = std::mem::take(batch);
    let event_count = events.len() as u64;
    let started = Instant::now();

    match deliver_with_retry(client, settings, &events).await {
        Ok(()) => {
            metrics::EVIDENCE_BATCHES_TOTAL
                .with_label_values(&["delivered"])
                .inc();
            metrics::EVIDENCE_EVENTS_DELIVERED_TOTAL
                .with_label_values(&["delivered"])
                .inc_by(event_count);
        }
        Err(error) => {
            metrics::EVIDENCE_BATCHES_TOTAL
                .with_label_values(&["failed"])
                .inc();
            metrics::EVIDENCE_EVENTS_DELIVERED_TOTAL
                .with_label_values(&["failed"])
                .inc_by(event_count);
            metrics::EVIDENCE_EVENTS_DROPPED_TOTAL
                .with_label_values(&["retry_exhausted"])
                .inc_by(event_count);
            tracing::warn!(
                error = %error,
                events = event_count,
                "VCAL Audit evidence batch delivery failed"
            );
        }
    }

    metrics::EVIDENCE_DELIVERY_LATENCY_SECONDS.observe(started.elapsed().as_secs_f64());
}

async fn deliver_with_retry(
    client: &Client,
    settings: &BufferedHttpEvidenceSettings,
    events: &[EvidenceEvent],
) -> anyhow::Result<()> {
    let mut backoff = settings.retry_initial_backoff;

    for attempt in 1..=settings.retry_max_attempts {
        match deliver_once(client, settings, events).await {
            Ok(()) => return Ok(()),
            Err(DeliveryFailure::Permanent(error)) => return Err(error),
            Err(DeliveryFailure::Retryable { error, .. })
                if attempt == settings.retry_max_attempts =>
            {
                return Err(error);
            }
            Err(DeliveryFailure::Retryable { error, retry_after }) => {
                metrics::EVIDENCE_RETRIES_TOTAL.inc();
                let retry_delay = retry_after
                    .map(|server_delay| server_delay.max(backoff))
                    .unwrap_or(backoff)
                    .min(settings.retry_max_backoff);
                tracing::warn!(
                    attempt,
                    max_attempts = settings.retry_max_attempts,
                    retry_delay_ms = retry_delay.as_millis() as u64,
                    error = %error,
                    "retrying VCAL Audit evidence delivery"
                );
                sleep(retry_delay).await;
                backoff = backoff.saturating_mul(2).min(settings.retry_max_backoff);
            }
        }
    }

    Err(anyhow!("VCAL Audit evidence delivery exhausted retries"))
}

async fn deliver_once(
    client: &Client,
    settings: &BufferedHttpEvidenceSettings,
    events: &[EvidenceEvent],
) -> Result<(), DeliveryFailure> {
    let payload = EvidenceBatch {
        schema_version: EVIDENCE_SCHEMA_VERSION,
        producer: EvidenceProducer {
            product: release::PRODUCT_NAME,
            version: release::PRODUCT_VERSION,
            instance_id: &settings.producer_instance_id,
        },
        events,
    };

    let mut request = client
        .post(&settings.endpoint)
        .header("X-VCAL-Producer", "ai-firewall")
        .header("X-VCAL-Schema-Version", EVIDENCE_SCHEMA_VERSION)
        .json(&payload);

    if let Some(api_key) = settings.api_key.as_deref() {
        request = request.bearer_auth(api_key);
    }

    let response = request
        .send()
        .await
        .map_err(|error| DeliveryFailure::Retryable {
            error: anyhow!(error),
            retry_after: None,
        })?;
    let status = response.status();

    if status.is_success() {
        return Ok(());
    }

    let retry_after = if status == StatusCode::TOO_MANY_REQUESTS {
        response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(Duration::from_secs)
    } else {
        None
    };
    let body = response.text().await.unwrap_or_default();
    let error = anyhow!("VCAL Audit returned HTTP {status}: {body}");

    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        Err(DeliveryFailure::Retryable { error, retry_after })
    } else {
        Err(DeliveryFailure::Permanent(error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> BufferedHttpEvidenceSettings {
        BufferedHttpEvidenceSettings {
            endpoint: "http://127.0.0.1:8092/v1/events/batch".to_string(),
            api_key: None,
            producer_instance_id: "test-instance".to_string(),
            queue_capacity: 10,
            batch_size: 5,
            flush_interval: Duration::from_millis(100),
            request_timeout: Duration::from_secs(1),
            retry_max_attempts: 1,
            retry_initial_backoff: Duration::from_millis(10),
            retry_max_backoff: Duration::from_millis(100),
        }
    }

    #[test]
    fn rejects_invalid_batch_size() {
        let mut settings = settings();
        settings.batch_size = 11;
        assert!(settings.validate().is_err());
    }

    async fn spawn_status_server(
        statuses: Vec<(u16, Option<&'static str>)>,
    ) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_task = Arc::clone(&calls);

        tokio::spawn(async move {
            for (status, retry_after) in statuses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = vec![0_u8; 8192];
                let _ = socket.read(&mut request).await;
                calls_task.fetch_add(1, Ordering::SeqCst);

                let reason = match status {
                    200 => "OK",
                    429 => "Too Many Requests",
                    500 => "Internal Server Error",
                    503 => "Service Unavailable",
                    _ => "Error",
                };
                let retry_header = retry_after
                    .map(|value| format!("Retry-After: {value}\r\n"))
                    .unwrap_or_default();
                let body = format!("status-{status}");
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}",
                    body.len(), retry_header, body
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });

        (format!("http://{addr}/v1/events/batch"), calls)
    }

    fn test_event() -> EvidenceEvent {
        EvidenceEvent::new(
            uuid::Uuid::new_v4(),
            super::super::EvidenceSource::AiFirewall,
            super::super::EventCategory::System,
            "test.event",
            super::super::EventOutcome::Started,
        )
    }

    #[tokio::test]
    async fn retries_http_500_then_delivers() {
        use std::sync::atomic::Ordering;
        let (endpoint, calls) = spawn_status_server(vec![(500, None), (200, None)]).await;
        let mut cfg = settings();
        cfg.endpoint = endpoint;
        cfg.retry_max_attempts = 2;
        cfg.retry_initial_backoff = Duration::from_millis(1);
        cfg.retry_max_backoff = Duration::from_millis(5);
        let client = Client::builder()
            .timeout(Duration::from_secs(1))
            .build()
            .unwrap();

        deliver_with_retry(&client, &cfg, &[test_event()])
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn retries_http_429_and_honors_retry_after_path() {
        use std::sync::atomic::Ordering;
        let (endpoint, calls) = spawn_status_server(vec![(429, Some("0")), (200, None)]).await;
        let mut cfg = settings();
        cfg.endpoint = endpoint;
        cfg.retry_max_attempts = 2;
        cfg.retry_initial_backoff = Duration::from_millis(1);
        cfg.retry_max_backoff = Duration::from_millis(5);
        let client = Client::builder()
            .timeout(Duration::from_secs(1))
            .build()
            .unwrap();

        deliver_with_retry(&client, &cfg, &[test_event()])
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn retry_exhaustion_returns_error_after_configured_attempts() {
        use std::sync::atomic::Ordering;
        let (endpoint, calls) = spawn_status_server(vec![(503, None), (503, None)]).await;
        let mut cfg = settings();
        cfg.endpoint = endpoint;
        cfg.retry_max_attempts = 2;
        cfg.retry_initial_backoff = Duration::from_millis(1);
        cfg.retry_max_backoff = Duration::from_millis(5);
        let client = Client::builder()
            .timeout(Duration::from_secs(1))
            .build()
            .unwrap();

        let error = deliver_with_retry(&client, &cfg, &[test_event()])
            .await
            .expect_err("503 should exhaust retries");
        assert!(error.to_string().contains("503"));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn reports_queue_full_without_waiting() {
        let (sender, _receiver) = mpsc::channel(1);
        let sink = BufferedHttpEvidenceSink { sender };
        let event = EvidenceEvent::new(
            uuid::Uuid::new_v4(),
            super::super::EvidenceSource::AiFirewall,
            super::super::EventCategory::System,
            "test.event",
            super::super::EventOutcome::Started,
        );

        sink.emit(event.clone()).await.unwrap();
        let error = sink.emit(event).await.unwrap_err();
        assert!(error.to_string().contains("queue is full"));
    }
}
