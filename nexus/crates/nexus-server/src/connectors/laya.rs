//! Laya connector (local fine-tuned typed decisions) + heuristic fallback.
//! The triage engine chain lives here: jev → laya → heuristic.

use crate::connectors::jev::{self, TriageItem};
use crate::config::ConnectorCfg;
use nexus_core::Graph;
use serde_json::json;

pub async fn health(http: &reqwest::Client, cfg: &ConnectorCfg) -> &'static str {
    match &cfg.laya_url {
        None => "unset",
        Some(url) => {
            // laya serve.py speaks the TypeSafe wire protocol at /v1/systemone
            let ok = http
                .post(format!("{}/v1/systemone", url.trim_end_matches('/')))
                .timeout(std::time::Duration::from_secs(8))
                .header("X-Laya-Domain", "osint-grading")
                .json(&json!({
                    "state": "ping",
                    "questions": {"ping": {"type": "noul", "instructions": "This state is non-empty"}}
                }))
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false);
            if ok { "up" } else { "down" }
        }
    }
}

fn etype_base(etype: &str) -> f64 {
    match etype {
        "person" => 1.0,
        "username" | "alias" => 0.9,
        "email" => 0.85,
        "phone" => 0.8,
        "org" => 0.75,
        "domain" | "wallet" => 0.7,
        "ip" => 0.6,
        _ => 0.4,
    }
}

/// Deterministic heuristic scorer (always available).
pub fn heuristic_triage(g: &Graph, entities: &[nexus_core::Entity]) -> Vec<TriageItem> {
    let evidence_of = |id: &str| g.edges.iter().filter(|e| e.src == id || e.dst == id).count();
    let mut items: Vec<TriageItem> = entities
        .iter()
        .map(|e| {
            let rels = evidence_of(&e.id);
            let score = (etype_base(e.etype.as_str())
                + (rels.min(5) as f64) * 0.04
                + if e.pinned { 0.15 } else { 0.0 })
                .clamp(0.0, 1.0);
            TriageItem {
                entity_id: e.id.clone(),
                score,
                verdict: jev::verdict_for(score).to_string(),
                reason: format!(
                    "heuristic: base {} + relations {} + pinned {}",
                    etype_base(e.etype.as_str()),
                    rels,
                    e.pinned
                ),
            }
        })
        .collect();
    items.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    items
}

/// Laya remote triage — tolerant shape, falls through on any error.
async fn laya_triage(
    http: &reqwest::Client,
    cfg: &ConnectorCfg,
    entities: &[nexus_core::Entity],
) -> Result<Vec<TriageItem>, String> {
    let url = cfg
        .laya_url
        .as_deref()
        .ok_or("laya not configured")?
        .trim_end_matches('/')
        .to_string();
    let mut items = Vec::new();
    for e in entities.iter().take(25) {
        let body = json!({
            "state": format!("{} {}", e.etype, e.label),
            "questions": {"lead_value": {"type": "score", "instructions":
                "How valuable is this entity as an investigation lead?",
                "criteria": ["0 discard", "1 background", "2 worth developing", "3 high-value lead"]}}
        });
        let resp = http
            .post(format!("{url}/v1/systemone"))
            .timeout(std::time::Duration::from_secs(20))
            .header("X-Laya-Domain", "osint-grading")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("laya unreachable ({e})"))?;
        if !resp.status().is_success() {
            return Err(format!("laya HTTP {}", resp.status()));
        }
        let v: serde_json::Value = resp.json().await.map_err(|e| format!("bad laya JSON ({e})"))?;
        if let Some((answer, _confidence)) = jev::parse_answer(&v, "lead_value") {
            let score = jev::score_of(&answer);
            items.push(TriageItem {
                entity_id: e.id.clone(),
                score,
                verdict: jev::verdict_for(score).to_string(),
                reason: format!("laya score {score:.2}"),
            });
        }
    }
    if items.is_empty() {
        return Err("laya returned no usable answers".into());
    }
    items.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    Ok(items)
}

pub struct TriageOutcome {
    pub engine: &'static str,
    pub ranked: Vec<TriageItem>,
}

/// Engine chain: jev → laya → heuristic. First configured+healthy wins.
pub async fn triage(
    http: &reqwest::Client,
    cfg: &ConnectorCfg,
    g: &Graph,
    entities: &[nexus_core::Entity],
) -> TriageOutcome {
    if jev::key_present(cfg) && !entities.is_empty() {
        match jev::triage(http, cfg, g, entities).await {
            Ok(ranked) if !ranked.is_empty() => {
                return TriageOutcome { engine: "jev", ranked };
            }
            Ok(_) => {}
            Err(e) => tracing::debug!("jev triage unavailable: {e}"),
        }
    }
    if cfg.laya_url.is_some() && !entities.is_empty() {
        match laya_triage(http, cfg, entities).await {
            Ok(ranked) => return TriageOutcome { engine: "laya", ranked },
            Err(e) => tracing::debug!("laya triage unavailable: {e}"),
        }
    }
    TriageOutcome {
        engine: "heuristic",
        ranked: heuristic_triage(g, entities),
    }
}

/// Append a decision row (JSONL) for future fine-tuning. Never hard-fails.
pub fn log_decision(path: Option<&std::path::Path>, row: &serde_json::Value) {
    let Some(path) = path else { return };
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let mut line = row.clone();
        if let Some(obj) = line.as_object_mut() {
            obj.insert("ts".into(), json!(nexus_core::now_iso()));
        }
        let _ = writeln!(f, "{line}");
    }
}
