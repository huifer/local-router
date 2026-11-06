//! The trace: a session's vendor calls, from the vendor's side back to the
//! session.
//!
//! The gateway is the only place that sees both ends of a conversation, so
//! it is the only place that can hold the link. This test walks the whole
//! way: a request with a session id goes out, the vendor sees the trace
//! ids, and the tree comes back — session over request over vendor call.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Once};

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

use local_router::config::{AppState, Config, Provider, ProviderKey};
use local_router::gateway;
use local_router::trace;

/// The environment is process-global: one test file, one home directory.
static INIT: Once = Once::new();

/// Point the gateway's own files (sessions, the usage database) at a
/// directory of this test's own, so nothing lands in the user's.
fn isolated_home(tag: &str) -> std::path::PathBuf {
    INIT.call_once(|| {
        std::env::set_var("LOCAL_ROUTER_DISABLE_SQLITE", "1");
    });
    let dir = std::env::temp_dir().join(format!(
        "local-router-trace-e2e-{}-{}-{}",
        tag,
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("LOCAL_ROUTER_HOME", &dir);
    dir
}

/// A vendor that remembers the headers it was sent.
#[derive(Clone, Default)]
struct Vendor {
    seen: Arc<Mutex<Vec<BTreeMap<String, String>>>>,
}

async fn vendor_chat(
    State(vendor): State<Vendor>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> (StatusCode, Json<Value>) {
    let mut seen = BTreeMap::new();
    for (name, value) in headers.iter() {
        seen.insert(
            name.as_str().to_string(),
            value.to_str().unwrap_or_default().to_string(),
        );
    }
    vendor.seen.lock().unwrap().push(seen);
    let req: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (
        StatusCode::OK,
        Json(json!({
            "id": "chatcmpl-trace-1",
            "object": "chat.completion",
            "model": req["model"].as_str().unwrap_or("m"),
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "hi"},
                "finish_reason": "stop",
            }],
            "usage": {"prompt_tokens": 3, "completion_tokens": 5, "total_tokens": 8},
        })),
    )
}

fn provider(base_url: String) -> Provider {
    Provider {
        id: "mock".into(),
        name: "mock".into(),
        kind: "custom".into(),
        enabled: true,
        protocol: local_router::protocol::ir::Protocol::OpenAiChat,
        base_url: Some(base_url),
        anthropic_base_url: None,
        responses_base_url: None,
        gemini_base_url: None,
        keys: vec![ProviderKey::new("sk-mock")],
        models: vec!["mock-small".into()],
        auto_include: Vec::new(),
        catalog: None,
        headers: Default::default(),
        user_agent: None,
        forward_headers: Vec::new(),
        remove_headers: Vec::new(),
        header_order: Vec::new(),
        oauth: None,
        proxy: None,
        group_only: false,
        auth: None,
        chat_path: None,
        stream_only: false,
        hold_when_exhausted: false,
        max_concurrency: None,
        rpm: None,
        tpm: None,
        media_models: Vec::new(),
        media_paths: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sessions_vendor_calls_are_found_again_by_their_trace() {
    let dir = isolated_home("trace");

    // The vendor.
    let vendor = Vendor::default();
    let vendor_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let vendor_port = vendor_listener.local_addr().unwrap().port();
    let v = vendor.clone();
    tokio::spawn(async move {
        axum::serve(
            vendor_listener,
            Router::new()
                .route("/v1/chat/completions", post(vendor_chat))
                .with_state(v),
        )
        .await
        .unwrap();
    });

    // The gateway, with its own config and its own files.
    let config = Config {
        providers: vec![provider(format!("http://127.0.0.1:{vendor_port}/v1"))],
        groups: vec![],
        settings: Default::default(),
        caller_keys: vec![],
        ..Default::default()
    };
    let config_path = dir.join("config.json");
    let store = Arc::new(local_router::config::Store::at(&config_path));
    store.save(&config).unwrap();
    let state = AppState::load(store).await.unwrap();

    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_port = gateway_listener.local_addr().unwrap().port();
    let app = gateway::router(gateway::Gateway::new(state));
    tokio::spawn(async move {
        axum::serve(gateway_listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    // A request, with the session the agent sent.
    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "http://127.0.0.1:{gateway_port}/v1/chat/completions"
        ))
        .header("authorization", "Bearer local-router")
        .header("x-session-id", "sess-42")
        .header("user-agent", "claude-code/1.0")
        .json(&json!({
            "model": "mock/mock-small",
            "messages": [{"role": "user", "content": "帮我看看这个网关"}],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    // The vendor was told which trace the call belongs to.
    let seen = vendor.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    let headers = &seen[0];
    let trace_id = headers
        .get(trace::TRACE_HEADER)
        .expect("the vendor saw the trace id")
        .clone();
    assert!(!trace_id.is_empty());
    assert_eq!(
        headers.get(trace::SESSION_HEADER).map(String::as_str),
        Some("sess-42")
    );
    assert!(headers.get(trace::SPAN_HEADER).is_some());

    // The tree came back: session over request over vendor call.
    let trace_path = dir.join("traces.jsonl");
    let spans = trace::read_all(&trace_path);
    // The stream of writes may still be settling; the file is written
    // before the response is sent, so it is there.
    assert!(
        !spans.is_empty(),
        "the trace log is empty at {trace_path:?}"
    );

    let session = spans
        .iter()
        .find(|s| s.kind == trace::SpanKind::Session)
        .expect("the session has a span");
    assert_eq!(session.session_id.as_deref(), Some("sess-42"));
    assert_eq!(session.trace_id, trace_id);

    let request = spans
        .iter()
        .find(|s| s.kind == trace::SpanKind::Request)
        .expect("the request has a span");
    assert_eq!(
        request.parent_span_id.as_deref(),
        Some(session.span_id.as_str())
    );
    assert_eq!(request.agent.as_deref(), Some("claude-code"));

    let upstream = spans
        .iter()
        .find(|s| s.kind == trace::SpanKind::Upstream)
        .expect("the vendor call has a span");
    assert_eq!(
        upstream.parent_span_id.as_deref(),
        Some(request.span_id.as_str())
    );
    assert_eq!(upstream.provider.as_deref(), Some("mock"));
    assert_eq!(upstream.attempt, Some(1));
    assert!(upstream.ok);
    assert_eq!(upstream.input_tokens, 3);
    assert_eq!(upstream.output_tokens, 5);
    // The vendor's own id is kept, so its log can be read back into ours.
    assert_eq!(
        upstream.upstream_request_id.as_deref(),
        Some("chatcmpl-trace-1")
    );

    // The tree builds: the vendor call hangs under the request, which
    // hangs under the session.
    let tree = trace::tree(&spans);
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].span.kind, trace::SpanKind::Session);
    assert_eq!(tree[0].children.len(), 1);
    assert_eq!(tree[0].children[0].children.len(), 1);

    // And the vendor's own id finds the call back.
    let found = trace::spans_by_upstream_id(&spans, "chatcmpl-trace-1");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].session_id.as_deref(), Some("sess-42"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_callers_own_trace_id_is_kept_all_the_way_down() {
    let dir = isolated_home("trace-own");

    let vendor = Vendor::default();
    let vendor_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let vendor_port = vendor_listener.local_addr().unwrap().port();
    let v = vendor.clone();
    tokio::spawn(async move {
        axum::serve(
            vendor_listener,
            Router::new()
                .route("/v1/chat/completions", post(vendor_chat))
                .with_state(v),
        )
        .await
        .unwrap();
    });

    let config = Config {
        providers: vec![provider(format!("http://127.0.0.1:{vendor_port}/v1"))],
        groups: vec![],
        settings: Default::default(),
        caller_keys: vec![],
        ..Default::default()
    };
    let store = Arc::new(local_router::config::Store::at(dir.join("config.json")));
    store.save(&config).unwrap();
    let state = AppState::load(store).await.unwrap();
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_port = gateway_listener.local_addr().unwrap().port();
    let app = gateway::router(gateway::Gateway::new(state));
    tokio::spawn(async move {
        axum::serve(gateway_listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "http://127.0.0.1:{gateway_port}/v1/chat/completions"
        ))
        .header("authorization", "Bearer local-router")
        .header("x-trace-id", "task-7")
        .header("x-task-id", "sub-agent-2")
        .json(&json!({
            "model": "mock/mock-small",
            "messages": [{"role": "user", "content": "second step"}],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    // The vendor saw the caller's own trace id.
    let seen = vendor.seen.lock().unwrap().clone();
    assert_eq!(
        seen[0].get(trace::TRACE_HEADER).map(String::as_str),
        Some("task-7")
    );
    assert_eq!(
        seen[0].get(trace::TASK_HEADER).map(String::as_str),
        Some("sub-agent-2")
    );

    // And the gateway kept the same id, with the step the caller named.
    let spans = trace::read_all(&dir.join("traces.jsonl"));
    let request = spans
        .iter()
        .find(|s| s.kind == trace::SpanKind::Request)
        .expect("the request has a span");
    assert_eq!(request.trace_id, "task-7");
    assert_eq!(request.task_id.as_deref(), Some("sub-agent-2"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_shows_its_whole_chain_across_the_callers_own_tasks() {
    let dir = isolated_home("trace-session");

    let vendor = Vendor::default();
    let vendor_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let vendor_port = vendor_listener.local_addr().unwrap().port();
    let v = vendor.clone();
    tokio::spawn(async move {
        axum::serve(
            vendor_listener,
            Router::new()
                .route("/v1/chat/completions", post(vendor_chat))
                .with_state(v),
        )
        .await
        .unwrap();
    });

    let config = Config {
        providers: vec![provider(format!("http://127.0.0.1:{vendor_port}/v1"))],
        groups: vec![],
        settings: Default::default(),
        caller_keys: vec![],
        ..Default::default()
    };
    let store = Arc::new(local_router::config::Store::at(dir.join("config.json")));
    store.save(&config).unwrap();
    let state = AppState::load(store).await.unwrap();
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_port = gateway_listener.local_addr().unwrap().port();
    let app = gateway::router(gateway::Gateway::new(state));
    tokio::spawn(async move {
        axum::serve(gateway_listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    // Two steps of one session, each with the caller's own task id.
    let client = reqwest::Client::new();
    for (task, content) in [("task-a", "第一步"), ("task-b", "第二步")] {
        let response = client
            .post(format!(
                "http://127.0.0.1:{gateway_port}/v1/chat/completions"
            ))
            .header("authorization", "Bearer local-router")
            .header("x-session-id", "sess-whole")
            .header("x-trace-id", task)
            .json(&json!({
                "model": "mock/mock-small",
                "messages": [{"role": "user", "content": content}],
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
    }

    // Two traces — one per task — but one session.
    let spans = trace::read_all(&dir.join("traces.jsonl"));
    let traces: Vec<&str> = spans
        .iter()
        .map(|s| s.trace_id.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(traces.len(), 2, "each task is its own trace");

    // The session view is the whole story: both steps, one tree.
    let node = trace::session_view(&spans, "sess-whole").expect("the session has a chain");
    assert_eq!(node.span.kind, trace::SpanKind::Session);
    assert_eq!(
        node.children.len(),
        2,
        "both requests are under the session"
    );
    for child in &node.children {
        assert_eq!(child.children.len(), 1, "each request has its vendor call");
        assert_eq!(
            child.children[0].span.provider.as_deref(),
            Some("mock"),
            "the vendor call hangs under its request"
        );
    }

    // And the session summary counts the whole thing.
    let summaries = trace::summarize_sessions(&spans);
    assert_eq!(summaries.len(), 1);
    let s = &summaries[0];
    assert_eq!(s.session_id, "sess-whole");
    assert_eq!(s.requests, 2);
    assert_eq!(s.upstream_calls, 2);
    assert_eq!(
        s.traces.len(),
        2,
        "the session spans both of the caller's tasks"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
