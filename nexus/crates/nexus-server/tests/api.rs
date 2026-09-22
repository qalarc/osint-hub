//! API integration tests — in-memory store, mock hub/archon, no real network.

use axum::routing::{get, post};
use nexus_server::{build_router, init_state, Config, ConnectorCfg};
use axum::Json;
use serde_json::{json, Value};
use std::path::PathBuf;

fn test_config(dir: &std::path::Path) -> Config {
    Config {
        db_path: dir.join("test.db"),
        web_dist: None,
        port: 0,
        config_path: Some(dir.join("config.json")),
        connectors: ConnectorCfg {
            hub_url: "http://127.0.0.1:1".into(), // nothing listens here
            hub_token: None,
            flowsint_url: None,
            openplanter_bin: None,
            workspaces: dir.join("workspaces"),
            archon_url: "http://127.0.0.1:1".into(),
            laya_url: None,
            jev_url: "https://api.typesafe.invalid".into(),
            jev_api_key: None, // force heuristic engine in tests
            laya_log: Some(dir.join("laya-decisions.jsonl")),
        },
    }
}

async fn spawn_app(cfg: Config) -> String {
    let state = init_state(cfg).await;
    let router = build_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}")
}

async fn api(base: &str, method: &str, path: &str, body: Option<Value>) -> Value {
    let client = reqwest::Client::new();
    let url = format!("{base}{path}");
    let mut req = match method {
        "POST" => client.post(&url),
        "PATCH" => client.patch(&url),
        "DELETE" => client.delete(&url),
        _ => client.get(&url),
    };
    if let Some(b) = body {
        req = req.json(&b);
    }
    let resp = req.send().await.expect("request failed");
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let mut v: Value = serde_json::from_str(&text)
        .unwrap_or_else(|_| json!({"raw": text}));
    if let Some(obj) = v.as_object_mut() {
        obj.insert("_status".into(), json!(status.as_u16()));
    }
    v
}

async fn api_text(base: &str, path: &str) -> (u16, String, Option<String>) {
    let resp = reqwest::Client::new()
        .get(format!("{base}{path}"))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let ctype = resp
        .headers()
        .get("content-type")
        .and_then(|c| c.to_str().ok())
        .map(String::from);
    (status, resp.text().await.unwrap_or_default(), ctype)
}

fn tempdir() -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "nexus-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

// ──────────────────────────────────────────────────────── mock services ──


async fn spawn_mock_hub() -> String {
    async fn health() -> &'static str {
        "ok"
    }
    async fn scan(Json(body): Json<Value>) -> (axum::http::StatusCode, Json<Value>) {
        // verify the fixed payload shape: multi + value object keyed by kind
        assert_eq!(body.get("type").and_then(|t| t.as_str()), Some("multi"), "body: {body}");
        let value = body.get("value").cloned().unwrap_or_default();
        assert!(value.get("username").is_some(), "value must be an object: {value}");
        (axum::http::StatusCode::ACCEPTED, Json::<Value>(json!({"job_id": "j_test"})))
    }
    async fn job() -> Json<Value> {
        Json(json!({
            "id": "j_test",
            "status": "done",
            "results": [
                {"qtype": "username", "tool": "sherlock", "status": "ok",
                 "found": [
                    {"site": "GitHub", "url": "https://github.com/janedoe"},
                    {"site": "Reddit", "url": "https://www.reddit.com/user/janedoe"}
                 ]},
                {"qtype": "username", "tool": "maigret", "status": "ok", "found": []}
            ]
        }))
    }
    let app = axum::Router::new()
        .route("/api/health", get(health))
        .route("/api/scan", post(scan))
        .route("/api/jobs/j_test", get(job));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

async fn spawn_mock_archon() -> String {
    async fn sandboxes() -> Json<Value> {
        Json(json!([{"id": "sb_1", "name": "canon", "library_id": "canon"}]))
    }
    async fn ask(Json(body): Json<Value>) -> Json<Value> {
        Json(json!({
            "answer": format!("Canon says: {}", body.get("question").and_then(|q| q.as_str()).unwrap_or("")),
            "citations": [{"source": "canon/entries/x.md"}],
            "libraries_searched": ["canon"],
            "backend": "test",
            "found": true
        }))
    }
    let app = axum::Router::new()
        .route("/api/sandboxes", get(sandboxes).post(post_sandbox))
        .route("/api/ask", axum::routing::post(ask));
    async fn post_sandbox(Json(_): Json<Value>) -> (axum::http::StatusCode, Json<Value>) {
        (
            axum::http::StatusCode::CREATED,
            Json::<Value>(json!({"id": "sb_new", "name": "nexus-test"})),
        )
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

// ────────────────────────────────────────────────────────────────── tests ──

#[tokio::test]
async fn t01_health_has_connector_map() {
    let dir = tempdir();
    let base = spawn_app(test_config(&dir)).await;
    let v = api(&base, "GET", "/api/health", None).await;
    assert_eq!(v["_status"], 200);
    assert_eq!(v["ok"], true);
    let connectors = v["connectors"].as_object().expect("connectors map");
    for name in ["hub", "flowsint", "openplanter", "archon", "laya", "jev"] {
        assert!(connectors.contains_key(name), "missing connector {name}");
    }
}

#[tokio::test]
async fn t02_case_crud_roundtrip() {
    let dir = tempdir();
    let base = spawn_app(test_config(&dir)).await;
    let created = api(
        &base, "POST", "/api/cases",
        Some(json!({"name": "Case 41", "notes": "test", "tags": ["osint"]})),
    ).await;
    assert_eq!(created["_status"], 201);
    let id = created["case"]["id"].as_str().unwrap().to_string();
    assert_eq!(created["case"]["name"], "Case 41");

    let listed = api(&base, "GET", "/api/cases", None).await;
    assert_eq!(listed["cases"].as_array().unwrap().len(), 1);

    let patched = api(&base, "PATCH", &format!("/api/cases/{id}"), Some(json!({"status": "archived"}))).await;
    assert_eq!(patched["case"]["status"], "archived");

    let got = api(&base, "GET", &format!("/api/cases/{id}"), None).await;
    assert_eq!(got["stats"]["entities"], 0);

    let deleted = api(&base, "DELETE", &format!("/api/cases/{id}"), None).await;
    assert_eq!(deleted["ok"], true);
}

#[tokio::test]
async fn t03_entity_add_dedup() {
    let dir = tempdir();
    let base = spawn_app(test_config(&dir)).await;
    let case = api(&base, "POST", "/api/cases", Some(json!({"name": "dedup"}))).await;
    let id = case["case"]["id"].as_str().unwrap();

    let e1 = api(&base, "POST", &format!("/api/cases/{id}/entities"),
        Some(json!({"etype": "email", "label": "Jane@Example.com"}))).await;
    assert_eq!(e1["deduplicated"], false);

    let e2 = api(&base, "POST", &format!("/api/cases/{id}/entities"),
        Some(json!({"etype": "email", "label": "jane@example.com"}))).await;
    assert_eq!(e2["deduplicated"], true, "same normalized email should dedup");
    assert_eq!(e1["entity"]["id"], e2["entity"]["id"]);

    let bad = api(&base, "POST", &format!("/api/cases/{id}/entities"),
        Some(json!({"etype": "wibble", "label": "x"}))).await;
    assert_eq!(bad["_status"], 400);
}

#[tokio::test]
async fn t04_graph_analysis_maps() {
    let dir = tempdir();
    let base = spawn_app(test_config(&dir)).await;
    let case = api(&base, "POST", "/api/cases", Some(json!({"name": "graph"}))).await;
    let id = case["case"]["id"].as_str().unwrap();
    let a = api(&base, "POST", &format!("/api/cases/{id}/entities"),
        Some(json!({"etype": "person", "label": "Jane"}))).await;
    let b = api(&base, "POST", &format!("/api/cases/{id}/entities"),
        Some(json!({"etype": "username", "label": "janedoe"}))).await;
    let (aid, bid) = (a["entity"]["id"].as_str().unwrap().to_string(), b["entity"]["id"].as_str().unwrap().to_string());

    let rel = api(&base, "POST", "/api/relations",
        Some(json!({"case_id": id, "src": aid, "dst": bid, "rel": "uses", "source": "test"}))).await;
    assert_eq!(rel["_status"], 201, "relation: {rel}");

    let g = api(&base, "GET", &format!("/api/cases/{id}/graph"), None).await;
    assert_eq!(g["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(g["edges"].as_array().unwrap().len(), 1);
    assert!(g["analysis"]["pagerank"].as_object().is_some());
    assert!(g["analysis"]["degree"].as_object().is_some());

    // shortest path a → b is direct
    let p = api(&base, "GET",
        &format!("/api/cases/{id}/path?a={aid}&b={bid}"), None).await;
    assert_eq!(p["path"].as_array().map(|a| a.len()), Some(2));
}

#[tokio::test]
async fn t05_doc_patch_render_roundtrip() {
    let dir = tempdir();
    let base = spawn_app(test_config(&dir)).await;
    let case = api(&base, "POST", "/api/cases", Some(json!({"name": "docs"}))).await;
    let id = case["case"]["id"].as_str().unwrap();
    let e = api(&base, "POST", &format!("/api/cases/{id}/entities"),
        Some(json!({"etype": "person", "label": "Jane"}))).await;
    let eid = e["entity"]["id"].as_str().unwrap();

    // render seeds skeleton
    let rendered = api(&base, "POST", &format!("/api/entities/{eid}/doc/render"), None).await;
    let doc = rendered["doc"].as_str().unwrap();
    assert!(doc.contains("# Jane"), "skeleton H1: {doc}");

    // human prose under Summary + Open questions survives re-render
    let patched = api(&base, "PATCH", &format!("/api/entities/{eid}/doc"),
        Some(json!({"markdown": "# Jane\n\n## Summary\n\nShe is a test subject.\n\n## Open questions\n\nWho is Jane really?\n\n## Connections\n\n(stale)\n"}))).await;
    assert_eq!(patched["_status"], 200);
    let rerendered = api(&base, "POST", &format!("/api/entities/{eid}/doc/render"), None).await;
    let doc2 = rerendered["doc"].as_str().unwrap();
    assert!(doc2.contains("She is a test subject."), "Summary prose preserved: {doc2}");
    assert!(doc2.contains("Who is Jane really?"), "Open questions preserved");
}

#[tokio::test]
async fn t06_dossier_markdown() {
    let dir = tempdir();
    let base = spawn_app(test_config(&dir)).await;
    let case = api(&base, "POST", "/api/cases", Some(json!({"name": "Dossier Target"}))).await;
    let id = case["case"]["id"].as_str().unwrap();
    let e = api(&base, "POST", &format!("/api/cases/{id}/entities"),
        Some(json!({"etype": "username", "label": "targetuser"}))).await;
    let eid = e["entity"]["id"].as_str().unwrap();
    api(&base, "POST", "/api/evidence",
        Some(json!({"case_id": id, "subject": eid, "kind": "manual",
                    "title": "note", "url": "https://example.com/x"}))).await;

    let (status, body, ctype) = api_text(&base, &format!("/api/cases/{id}/dossier.md")).await;
    assert_eq!(status, 200);
    assert!(ctype.unwrap().starts_with("text/markdown"));
    assert!(body.contains("Dossier Target"), "has case name");
    assert!(body.contains("## Evidence appendix"), "has evidence appendix");
    assert!(body.contains("## Key entities"));

    let (status2, brief, _) = api_text(&base, &format!("/api/cases/{id}/brief.md")).await;
    assert_eq!(status2, 200);
    assert!(brief.contains("## Claim ledger"), "brief has claim ledger");
}

#[tokio::test]
async fn t07_admin_config_redacts_secrets() {
    let dir = tempdir();
    let base = spawn_app(test_config(&dir)).await;
    let v = api(&base, "POST", "/api/admin/config",
        Some(json!({"jev_api_key": "super-secret-key", "hub_token": "hub-secret", "hub_url": "http://127.0.0.1:9999"}))).await;
    assert_eq!(v["ok"], true);
    let text = serde_json::to_string(&v).unwrap();
    assert!(!text.contains("super-secret-key"), "jev key must be redacted");
    assert!(!text.contains("hub-secret"), "hub token must be redacted");
    assert_eq!(v["config"]["hub_url"], "http://127.0.0.1:9999");
    // persisted overlay file exists with 0600-ish content
    assert!(dir.join("config.json").exists(), "overlay persisted");
}

#[tokio::test]
async fn t08_hub_scan_end_to_end_with_mock() {
    let dir = tempdir();
    let hub = spawn_mock_hub().await;
    let mut cfg = test_config(&dir);
    cfg.connectors.hub_url = hub;
    let base = spawn_app(cfg).await;

    let case = api(&base, "POST", "/api/cases", Some(json!({"name": "scan case"}))).await;
    let id = case["case"]["id"].as_str().unwrap().to_string();
    let e = api(&base, "POST", &format!("/api/cases/{id}/entities"),
        Some(json!({"etype": "username", "label": "janedoe"}))).await;
    let eid = e["entity"]["id"].as_str().unwrap().to_string();

    let task = api(&base, "POST", &format!("/api/cases/{id}/tasks"),
        Some(json!({"kind": "hub_scan", "target": eid}))).await;
    assert_eq!(task["_status"], 202, "task: {task}");
    let tid = task["task"]["id"].as_str().unwrap().to_string();

    // poll to completion
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut done = false;
    while std::time::Instant::now() < deadline {
        let t = api(&base, "GET", &format!("/api/tasks/{tid}"), None).await;
        if t["task"]["status"] == "done" || t["task"]["status"] == "error" {
            assert_eq!(t["task"]["status"], "done", "task failed: {t}");
            done = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    assert!(done, "task did not finish in time");

    let g = api(&base, "GET", &format!("/api/cases/{id}/graph"), None).await;
    let nodes = g["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 3, "target + 2 hits: {nodes:?}");
    let ingested = api(&base, "GET", &format!("/api/tasks/{tid}"), None).await;
    assert_eq!(ingested["task"]["ingested"]["entities"], 2);
    assert_eq!(ingested["task"]["ingested"]["relations"], 2);
    assert_eq!(ingested["task"]["ingested"]["evidence"], 2);
}

#[tokio::test]
async fn t09_triage_heuristic_without_remote_engines() {
    let dir = tempdir();
    let base = spawn_app(test_config(&dir)).await;
    let case = api(&base, "POST", "/api/cases", Some(json!({"name": "triage"}))).await;
    let id = case["case"]["id"].as_str().unwrap();
    api(&base, "POST", &format!("/api/cases/{id}/entities"),
        Some(json!({"etype": "person", "label": "Very Important Person"}))).await;

    let t = api(&base, "POST", "/api/triage", Some(json!({"case_id": id}))).await;
    assert_eq!(t["_status"], 200);
    assert_eq!(t["engine"], "heuristic", "no jev key, no laya → heuristic");
    assert!(!t["ranked"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn t10_archon_ask_attaches_evidence() {
    let dir = tempdir();
    let archon = spawn_mock_archon().await;
    let mut cfg = test_config(&dir);
    cfg.connectors.archon_url = archon;
    let base = spawn_app(cfg).await;

    let case = api(&base, "POST", "/api/cases", Some(json!({"name": "grounded"}))).await;
    let id = case["case"]["id"].as_str().unwrap().to_string();

    let status = api(&base, "GET", "/api/archon/status", None).await;
    assert_eq!(status["up"], true);

    let ans = api(&base, "POST", "/api/archon/ask",
        Some(json!({"question": "who is jane?", "case_id": id}))).await;
    assert_eq!(ans["ok"], true);
    assert!(ans["answer"].as_str().unwrap().contains("who is jane?"));
    assert_eq!(ans["citations"].as_array().unwrap().len(), 1);
    assert!(ans["evidence_id"].is_string(), "evidence attached");

    let g = api(&base, "GET", &format!("/api/cases/{id}/graph"), None).await;
    assert!(g["nodes"].as_array().unwrap().iter().any(|n| n["label"].as_str().unwrap_or("").starts_with("Archon:")));
}

#[tokio::test]
async fn t11_unknown_route_is_json_error() {
    let dir = tempdir();
    let base = spawn_app(test_config(&dir)).await;
    let v = api(&base, "GET", "/api/cases/c_nope", None).await;
    assert_eq!(v["_status"], 404);
    assert!(v["error"].as_str().is_some());
}
