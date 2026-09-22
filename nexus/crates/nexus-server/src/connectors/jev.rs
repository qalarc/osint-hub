//! Jev (TypeSafe AI) calibrated-decision connector.
//!
//! REST (verified from typesafe_jev/mcp_server.py):
//! `POST {JEV_URL}` `{"state", "model":"jev-latest", "questions":{name:{
//! type:"choice"|"score"|"noul", instructions, criteria?}}}` Bearer key →
//! 200 map keyed by question name. Never logs or returns the key.

use crate::config::ConnectorCfg;
use serde_json::{json, Value};

pub fn key_present(cfg: &ConnectorCfg) -> bool {
    cfg.jev_api_key.as_deref().map(|k| !k.is_empty()).unwrap_or(false)
}

pub async fn health(http: &reqwest::Client, cfg: &ConnectorCfg) -> &'static str {
    if !key_present(cfg) {
        return "unset";
    }
    match noul(http, cfg, "ping", "This state is non-empty").await {
        Ok(_) => "up",
        Err(_) => "down",
    }
}

/// One raw ask with arbitrary questions; returns the raw response JSON.
pub async fn ask(
    http: &reqwest::Client,
    cfg: &ConnectorCfg,
    state: &str,
    questions: Value,
) -> Result<Value, String> {
    let Some(key) = cfg.jev_api_key.as_deref().filter(|k| !k.is_empty()) else {
        return Err("jev key not configured".into());
    };
    let body = json!({"state": state, "model": "jev-latest", "questions": questions});
    let resp = http
        .post(cfg.jev_url.trim_end_matches('/'))
        .timeout(std::time::Duration::from_secs(30))
        .bearer_auth(key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("jev unreachable ({e})"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("jev HTTP {status}: {}", text.chars().take(200).collect::<String>()));
    }
    serde_json::from_str(&text).map_err(|e| format!("bad jev JSON ({e})"))
}

/// Convenience: a single true/false (`noul`) question → (answer, confidence
/// in the decided direction). Verified response shape:
/// `answers.<name> = {"type":"noul","noul": P(true)}`.
pub async fn noul(
    http: &reqwest::Client,
    cfg: &ConnectorCfg,
    state: &str,
    instructions: &str,
) -> Result<(bool, f64), String> {
    let questions = json!({"q": {"type": "noul", "instructions": instructions}});
    let resp = ask(http, cfg, state, questions).await?;
    let (v, p_true) =
        parse_answer(&resp, "q").ok_or_else(|| format!("jev response missing 'q' answer: {resp}"))?;
    let yes = v.as_bool().unwrap_or(false);
    Ok((yes, if yes { p_true } else { 1.0 - p_true }))
}

/// Extract (answer value, confidence) from a jev response. VERIFIED shapes:
/// `{"answers": {"<name>": {"type":"noul","noul":0.6}}}` (noul = P(true)),
/// `{"answers": {"<name>": {"type":"score","score":2.99,"confidence":0.83,
/// "probabilities":{...}}}}`, choice: `{"choice":"<label>",...}`.
pub fn parse_answer(resp: &Value, name: &str) -> Option<(Value, f64)> {
    let a = resp
        .get("answers")
        .and_then(|x| x.get(name))
        .or_else(|| resp.get(name))?;
    match a.get("type").and_then(|t| t.as_str()) {
        Some("noul") => {
            let p = a.get("noul").and_then(|v| v.as_f64()).unwrap_or(0.5);
            Some((json!(p >= 0.5), p.clamp(0.0, 1.0)))
        }
        Some("score") => {
            let s = a.get("score").and_then(|v| v.as_f64())?;
            let c = a.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.5);
            Some((json!(s), c.clamp(0.0, 1.0)))
        }
        Some("choice") => {
            let ch = a.get("choice").cloned()?;
            let c = a.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.5);
            Some((ch, c.clamp(0.0, 1.0)))
        }
        _ => {
            // legacy tolerance: direct answer/value fields
            let answer = a
                .get("answer")
                .or_else(|| a.get("value"))
                .or_else(|| a.get("choice"))
                .or_else(|| a.get("score"))
                .cloned()?;
            let confidence = a
                .get("confidence")
                .or_else(|| a.get("conf"))
                .and_then(|c| c.as_f64())
                .unwrap_or(0.5);
            Some((answer, confidence.clamp(0.0, 1.0)))
        }
    }
}

/// Normalize a score answer to 0..1 (jev is already calibrated — no blend).
pub fn score_of(answer: &Value) -> f64 {
    let raw = match answer {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.trim().parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    };
    if raw <= 1.0 {
        raw.clamp(0.0, 1.0)
    } else if raw <= 4.0 {
        raw / 4.0
    } else if raw <= 100.0 {
        raw / 100.0
    } else {
        1.0
    }
}

pub struct TriageItem {
    pub entity_id: String,
    pub score: f64,
    pub verdict: String,
    pub reason: String,
}

/// Jev lead-triage over up to 25 unpinned entities (buffer_unordered 8).
pub async fn triage(
    http: &reqwest::Client,
    cfg: &ConnectorCfg,
    g: &nexus_core::Graph,
    entities: &[nexus_core::Entity],
) -> Result<Vec<TriageItem>, String> {
    use futures::StreamExt;
    use std::sync::Arc;

    let conn_count = |id: &str| g.edges.iter().filter(|e| e.src == id || e.dst == id).count();
    let jobs: Vec<_> = entities
        .iter()
        .take(25)
        .map(|e| {
            let conns: Vec<String> = g
                .edges
                .iter()
                .filter(|r| r.src == e.id || r.dst == e.id)
                .take(4)
                .map(|r| g.label(if r.src == e.id { &r.dst } else { &r.src }))
                .map(String::from)
                .collect();
            let state = format!(
                "Investigation lead for triage. type={} label={:?}; evidence_count={}; relations={}; notable connections: {}",
                e.etype,
                e.label,
                conn_count(&e.id),
                conn_count(&e.id),
                if conns.is_empty() { "none yet".to_string() } else { conns.join(", ") }
            );
            (e.id.clone(), state)
        })
        .collect();

    let http = Arc::new(http.clone());
    let cfg = Arc::new(cfg.clone());
    let results = futures::stream::iter(jobs)
        .map(|(id, state)| {
            let http = http.clone();
            let cfg = cfg.clone();
            async move {
                let questions = json!({"lead": {"type": "score", "instructions":
                    "How valuable is this entity as an investigation lead? Score 0-4: 0 discard, 1 background, 2 worth developing, 3 strong lead, 4 critical lead.",
                    "criteria": ["0 discard", "1 background", "2 worth developing", "3 strong lead", "4 critical lead"]}});
                let out = ask(&http, &cfg, &state, questions).await;
                (id, out)
            }
        })
        .buffer_unordered(8)
        .collect::<Vec<_>>()
        .await;

    let mut items = Vec::new();
    for (id, out) in results {
        match out.as_ref().map(|v| parse_answer(v, "lead")) {
            Ok(Some((answer, confidence))) => {
                let score = score_of(&answer);
                items.push(TriageItem {
                    entity_id: id,
                    score,
                    verdict: verdict_for(score).to_string(),
                    reason: format!("jev score {score:.2} (conf {confidence:.2})"),
                });
            }
            Ok(None) => {} // malformed — skip
            Err(e) => return Err(e.clone()),
        }
    }
    items.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    Ok(items)
}

pub fn verdict_for(score: f64) -> &'static str {
    if score >= 0.8 {
        "high-value lead"
    } else if score >= 0.5 {
        "worth developing"
    } else {
        "background"
    }
}
