//! archon connector — canon/RAG pillar (verified against tulpa_platform
//! pillars.py, 2026-09). Degrades gracefully: archon down NEVER fails a
//! case operation.

use crate::config::ConnectorCfg;
use serde_json::{json, Value};

fn base(cfg: &ConnectorCfg) -> String {
    cfg.archon_url.trim_end_matches('/').to_string()
}

pub struct ArchonAnswer {
    pub ok: bool,
    pub answer: String,
    pub citations: Vec<Value>,
    pub backend: Option<String>,
    pub found: bool,
    pub detail: String,
}

pub async fn health(http: &reqwest::Client, cfg: &ConnectorCfg) -> &'static str {
    let url = format!("{}/api/sandboxes", base(cfg));
    let ok = http
        .get(url)
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);
    if ok { "up" } else { "down" }
}

pub async fn list_sandboxes(http: &reqwest::Client, cfg: &ConnectorCfg) -> Option<Vec<Value>> {
    let url = format!("{}/api/sandboxes", base(cfg));
    let resp = http
        .get(url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v: Value = resp.json().await.ok()?;
    v.as_array().cloned()
}

/// POST /api/ask — never panics, never hard-fails.
pub async fn ask(
    http: &reqwest::Client,
    cfg: &ConnectorCfg,
    question: &str,
    sandbox_id: Option<&str>,
    top_k: u32,
) -> ArchonAnswer {
    let url = format!("{}/api/ask", base(cfg));
    let mut body = json!({"question": question, "top_k": top_k});
    if let Some(sb) = sandbox_id {
        body["sandbox_id"] = json!(sb);
    }
    let resp = match http
        .post(url)
        .timeout(std::time::Duration::from_secs(45))
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return ArchonAnswer {
                ok: false,
                answer: String::new(),
                citations: vec![],
                backend: None,
                found: false,
                detail: format!("archon unreachable ({e})"),
            }
        }
    };
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return ArchonAnswer {
            ok: false,
            answer: String::new(),
            citations: vec![],
            backend: None,
            found: false,
            detail: format!("archon HTTP {status}: {}", text.chars().take(160).collect::<String>()),
        };
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(v) => {
            let answer = v
                .get("answer")
                .and_then(|a| a.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if answer.is_empty() {
                ArchonAnswer {
                    ok: false,
                    answer,
                    citations: vec![],
                    backend: v.get("backend").and_then(|b| b.as_str()).map(String::from),
                    found: false,
                    detail: "archon returned an empty answer".into(),
                }
            } else {
                ArchonAnswer {
                    ok: true,
                    answer,
                    citations: v
                        .get("citations")
                        .and_then(|c| c.as_array())
                        .cloned()
                        .unwrap_or_default(),
                    backend: v.get("backend").and_then(|b| b.as_str()).map(String::from),
                    found: v.get("found").and_then(|f| f.as_bool()).unwrap_or(true),
                    detail: String::new(),
                }
            }
        }
        Err(e) => ArchonAnswer {
            ok: false,
            answer: String::new(),
            citations: vec![],
            backend: None,
            found: false,
            detail: format!("bad archon JSON ({e})"),
        },
    }
}

/// POST /api/sandboxes {"name"} — 200/201 returns the sandbox; on 400/409
/// (likely exists) look it up in the list.
pub async fn ensure_sandbox(
    http: &reqwest::Client,
    cfg: &ConnectorCfg,
    name: &str,
) -> Result<Value, String> {
    let url = format!("{}/api/sandboxes", base(cfg));
    let resp = http
        .post(&url)
        .timeout(std::time::Duration::from_secs(10))
        .json(&json!({"name": name}))
        .send()
        .await
        .map_err(|e| format!("archon unreachable ({e})"))?;
    let status = resp.status();
    if status.is_success() {
        if let Ok(v) = resp.json::<Value>().await {
            return Ok(v);
        }
    }
    // fall through: maybe it already exists
    if let Some(list) = list_sandboxes(http, cfg).await {
        for sb in list {
            if sb.get("name").and_then(|n| n.as_str()) == Some(name) {
                return Ok(sb);
            }
        }
    }
    Err(format!("archon sandbox create failed (HTTP {status})"))
}
