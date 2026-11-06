//! Optional OTLP tracing export — build feature `otlp`, off by default.
//!
//! The gateway already keeps the whole session tree (`usage.db` + traces.jsonl);
//! this module mirrors those spans to an OpenTelemetry collector over
//! **OTLP/HTTP** (the protobuf binary encoding).
//!
//! It is deliberately **hand-rolled**: no OpenTelemetry SDK, no extra crates —
//! the protobuf wire format here is small, and the project's binary stays
//! lean. The default build compiles this file to no-ops.
//!
//! W3C ids: the internal trace/span ids are strings (`trc_…`, or the caller's
//! `x-trace-id`). They map deterministically to OTel's 16/8-byte ids via
//! SHA-256, so parent/child links survive unchanged, and the raw ids ride along
//! as span attributes so a backend can still search by the string form.

use crate::config::OtlpSettings;

#[cfg(feature = "otlp")]
mod otlp {
    use super::*;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Duration;

    use sha2::{Digest, Sha256};

    /// One-time, one-process exporter. `None` = feature on but no endpoint.
    static EXPORTER: OnceLock<Option<Exporter>> = OnceLock::new();

    /// Start the exporter from `settings.otlp`. Idempotent.
    pub fn init(settings: &OtlpSettings) {
        if EXPORTER.get().is_some() {
            return;
        }
        let exporter = if settings.endpoint.trim().is_empty() {
            None
        } else {
            Some(Exporter::start(settings))
        };
        let _ = EXPORTER.set(exporter);
    }

    /// Flush whatever is queued and stop the exporter.
    pub async fn shutdown() {
        if let Some(Some(exporter)) = EXPORTER.get() {
            exporter.handle.abort();
            let spans = drain(&exporter.queue);
            if !spans.is_empty() {
                exporter.post(spans).await;
            }
        }
    }

    /// Enqueue one span for export. Returns early when nothing is configured.
    pub fn export_span(span: &crate::trace::Span) {
        let Some(exporter) = EXPORTER.get().and_then(|e| e.as_ref()) else {
            return;
        };
        let bytes = encode_span(span);
        let mut queue = exporter.queue.lock().expect("OTLP queue lock");
        if queue.len() >= MAX_BATCH {
            tracing::warn!(
                "the OTLP export queue is full ({MAX_BATCH} spans); dropping this span"
            );
            return;
        }
        queue.push(bytes);
    }

    const MAX_BATCH: usize = 1024;

    struct Exporter {
        client: reqwest::Client,
        url: String,
        resource_prefix: Vec<u8>,
        scope_prefix: Vec<u8>,
        queue: Arc<Mutex<Vec<Vec<u8>>>>,
        handle: tokio::task::JoinHandle<()>,
    }

    impl Exporter {
        fn start(settings: &OtlpSettings) -> Self {
            let url = format!(
                "{}/v1/traces",
                settings.endpoint.trim().trim_end_matches('/')
            );
            let resource_prefix =
                field_bytes(1, &encode_kv("service.name", &settings.service_name));
            let scope_prefix = field_bytes(1, &field_string(1, &settings.service_name));
            let queue = Arc::new(Mutex::new(Vec::new()));
            let client = reqwest::Client::new();
            let handle = run_flush(
                queue.clone(),
                client.clone(),
                url.clone(),
                resource_prefix.clone(),
                scope_prefix.clone(),
            );
            Exporter {
                client,
                url,
                resource_prefix,
                scope_prefix,
                queue,
                handle,
            }
        }

        async fn post(&self, spans: Vec<Vec<u8>>) {
            let body = build_batch(&self.resource_prefix, &self.scope_prefix, &spans);
            if let Err(err) = self
                .client
                .post(&self.url)
                .header(reqwest::header::CONTENT_TYPE, "application/x-protobuf")
                .body(body)
                .send()
                .await
            {
                tracing::warn!("the OTLP export to {} failed: {err}", self.url);
            }
        }
    }

    fn run_flush(
        queue: Arc<Mutex<Vec<Vec<u8>>>>,
        client: reqwest::Client,
        url: String,
        resource_prefix: Vec<u8>,
        scope_prefix: Vec<u8>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let spans = drain(&queue);
                if spans.is_empty() {
                    continue;
                }
                let body = build_batch(&resource_prefix, &scope_prefix, &spans);
                if let Err(err) = client
                    .post(&url)
                    .header(reqwest::header::CONTENT_TYPE, "application/x-protobuf")
                    .body(body)
                    .send()
                    .await
                {
                    tracing::warn!("the OTLP export to {url} failed: {err}");
                }
            }
        })
    }

    /// Build one `ExportTraceServiceRequest` message from precomputed
    /// resource/scope prefixes and a batch of encoded spans.
    fn build_batch(resource_prefix: &[u8], scope_prefix: &[u8], spans: &[Vec<u8>]) -> Vec<u8> {
        let mut scope_spans = scope_prefix.to_vec();
        for span in spans {
            scope_spans.extend(field_bytes(2, span));
        }
        let mut resource_spans = resource_prefix.to_vec();
        resource_spans.extend(field_bytes(2, &scope_spans));
        field_bytes(1, &resource_spans)
    }

    fn drain(queue: &Arc<Mutex<Vec<Vec<u8>>>>) -> Vec<Vec<u8>> {
        let mut q = queue.lock().expect("OTLP queue lock");
        std::mem::take(&mut *q)
    }

    // ------------------------------------------------------ protobuf core --

    fn varint(mut value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        while value >= 0x80 {
            out.push((value as u8 & 0x7f) | 0x80);
            value >>= 7;
        }
        out.push(value as u8);
        out
    }

    fn field_bytes(field: u32, payload: &[u8]) -> Vec<u8> {
        let mut out = varint(((field as u64) << 3) | 2);
        out.extend_from_slice(&varint(payload.len() as u64));
        out.extend_from_slice(payload);
        out
    }

    fn field_string(field: u32, value: &str) -> Vec<u8> {
        field_bytes(field, value.as_bytes())
    }

    fn field_varint(field: u32, value: u64) -> Vec<u8> {
        let mut out = varint(((field as u64) << 3) | 0);
        out.extend_from_slice(&varint(value));
        out
    }

    fn field_fixed64(field: u32, value: u64) -> Vec<u8> {
        let mut out = varint(((field as u64) << 3) | 1);
        out.extend_from_slice(&value.to_le_bytes());
        out
    }

    fn field_fixed32(field: u32, value: u32) -> Vec<u8> {
        let mut out = varint(((field as u64) << 3) | 5);
        out.extend_from_slice(&value.to_le_bytes());
        out
    }

    fn hash_to(input: &[u8], len: usize) -> Vec<u8> {
        let digest = Sha256::digest(input);
        digest[..len].to_vec()
    }

    // ------------------------------------------------------- span encoding --

    fn encode_kv(key: &str, value: &str) -> Vec<u8> {
        let mut out = field_string(1, key); // KeyValue.key
        out.extend(field_bytes(2, &field_string(1, value))); // AnyValue.string_value
        out
    }

    fn kind_num(kind: crate::trace::SpanKind) -> u64 {
        match kind {
            crate::trace::SpanKind::Session => 1,  // INTERNAL
            crate::trace::SpanKind::Request => 2,  // SERVER
            crate::trace::SpanKind::Upstream => 3, // CLIENT
        }
    }

    fn unix_nano(s: &str) -> u64 {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.timestamp_nanos_opt().unwrap_or(0) as u64)
            .unwrap_or(0)
    }

    fn encode_span(span: &crate::trace::Span) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(field_bytes(1, &hash_to(span.trace_id.as_bytes(), 16)));
        out.extend(field_bytes(2, &hash_to(span.span_id.as_bytes(), 8)));
        if let Some(parent) = &span.parent_span_id {
            out.extend(field_bytes(4, &hash_to(parent.as_bytes(), 8)));
        }
        out.extend(field_fixed32(16, 1)); // W3C trace-flags SAMPLED
        out.extend(field_string(5, &span.name));
        out.extend(field_varint(6, kind_num(span.kind)));
        out.extend(field_fixed64(7, unix_nano(&span.started_at)));
        if let Some(ended) = &span.ended_at {
            out.extend(field_fixed64(8, unix_nano(ended)));
        }
        for (key, value) in span_attributes(span) {
            out.extend(field_bytes(9, &encode_kv(key, &value)));
        }
        out
    }

    fn span_attributes(span: &crate::trace::Span) -> Vec<(&'static str, String)> {
        let mut a = vec![("local_router.trace_id", span.trace_id.clone())];
        if let Some(v) = &span.session_id {
            a.push(("local_router.session_id", v.clone()));
        }
        if let Some(v) = &span.task_id {
            a.push(("local_router.task_id", v.clone()));
        }
        if let Some(v) = &span.provider {
            a.push(("local_router.provider", v.clone()));
        }
        if let Some(v) = &span.provider_key_id {
            a.push(("local_router.provider_key", v.clone()));
        }
        if !span.model.is_empty() {
            a.push(("local_router.model", span.model.clone()));
        }
        if let Some(v) = &span.attempt {
            a.push(("local_router.attempt", v.to_string()));
        }
        if span.status != 0 {
            a.push(("local_router.status", span.status.to_string()));
        }
        if span.skipped {
            a.push(("local_router.skipped", "true".into()));
        }
        if let Some(v) = &span.error {
            a.push(("local_router.error", v.clone()));
        }
        if let Some(v) = &span.agent {
            a.push(("local_router.agent", v.clone()));
        }
        if let Some(v) = &span.protocol {
            a.push(("local_router.protocol", v.clone()));
        }
        a
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_span_encodes_its_ids_and_parent() {
            let span = crate::trace::Span {
                trace_id: "trc_abc".into(),
                span_id: "sp_1".into(),
                parent_span_id: Some("parent_sp".into()),
                kind: crate::trace::SpanKind::Upstream,
                name: "deepseek · deepseek-chat".into(),
                session_id: Some("sess".into()),
                task_id: None,
                agent: None,
                project: None,
                caller_key_id: None,
                model: "deepseek-chat".into(),
                provider: Some("deepseek".into()),
                provider_key_id: Some("k1".into()),
                protocol: Some("openai_chat".into()),
                attempt: Some(2),
                requests: 1,
                status: 200,
                ok: true,
                skipped: false,
                error: None,
                latency_ms: 12,
                input_tokens: 10,
                output_tokens: 20,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                reasoning_tokens: 0,
                cost_usd: None,
                credits: None,
                started_at: "2026-01-01T00:00:00Z".into(),
                ended_at: Some("2026-01-01T00:00:00.012Z".into()),
                upstream_request_id: None,
                messages: 1,
                preview: None,
            };
            let bytes = encode_span(&span);
            let trace_id = read_bytes(1, &bytes);
            let span_id = read_bytes(2, &bytes);
            let parent = read_bytes(4, &bytes);
            assert_eq!(trace_id.len(), 16, "trace_id is 16 bytes");
            assert_eq!(span_id.len(), 8, "span_id is 8 bytes");
            assert_eq!(parent.len(), 8, "parent_span_id is 8 bytes");
            assert_eq!(
                parent,
                hash_to(span.parent_span_id.as_deref().unwrap().as_bytes(), 8),
                "the parent's own id is the hash of the same string"
            );
        }

        /// Parse the length-delimited fields of a protobuf message.
        fn read_bytes(field: u32, body: &[u8]) -> Vec<u8> {
            let mut pos = 0;
            while pos < body.len() {
                let tag = read_varint(body, &mut pos);
                let num = tag >> 3;
                let wire = tag & 7;
                if wire != 2 {
                    match wire {
                        0 => {
                            while pos < body.len() && body[pos] >= 0x80 {
                                pos += 1;
                            }
                            pos += 1;
                        }
                        1 => pos += 8,
                        5 => pos += 4,
                        _ => return Vec::new(),
                    }
                    continue;
                }
                let len = read_varint(body, &mut pos) as usize;
                let payload = &body[pos..pos + len];
                if num == field as u64 {
                    return payload.to_vec();
                }
                pos += len;
            }
            Vec::new()
        }

        fn read_varint(body: &[u8], pos: &mut usize) -> u64 {
            let mut out = 0u64;
            let mut shift = 0;
            while *pos < body.len() {
                let b = body[*pos];
                *pos += 1;
                out |= ((b & 0x7f) as u64) << shift;
                if b < 0x80 {
                    break;
                }
                shift += 7;
            }
            out
        }
    }
}

#[cfg(not(feature = "otlp"))]
mod otlp {
    use super::*;

    /// No-op without the `otlp` feature.
    pub fn init(_settings: &OtlpSettings) {}
    pub async fn shutdown() {}
    pub fn export_span(_span: &crate::trace::Span) {}
}

pub use otlp::{export_span, init, shutdown};
