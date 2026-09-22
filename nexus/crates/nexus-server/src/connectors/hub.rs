//! qalarc OSINT hub connector (verified — this repo's own backend).
//!
//! `POST {hub}/api/scan {type:"multi", value, tools, options}` → 202
//! `{job_id}` → poll `GET {hub}/api/jobs/{id}` → ingest results.

use crate::connectors::IngestCounts;
use crate::config::ConnectorCfg;
use nexus_core::{EType, NewEntity, NewEvidence, NewEvent, NewRelation, Store};
use serde_json::{json, Value};

pub struct HubJob {
    pub id: String,
    pub status: String,
    pub results: Vec<HubResult>,
}

#[derive(Debug, Clone)]
pub struct HubResult {
    pub qtype: String,
    pub tool: String,
    pub status: String,
    pub found: Vec<(String, String)>, // (site, url)
    pub info: Vec<(String, String)>,  // key facts (phoneinfoga)
    pub error: Option<String>,
}

pub fn scan_kind_for_etype(etype: &str) -> &'static str {
    match etype {
        "email" => "email",
        "phone" => "phone",
        "image" => "image",
        _ => "username", // username, alias, and anything else text-ish
    }
}

fn tools_for(kind: &str) -> &'static [&'static str] {
    match kind {
        "email" => &["holehe"],
        "phone" => &["phoneinfoga", "ignorant"],
        "image" => &["revimg"],
        _ => &["sherlock", "maigret"],
    }
}

fn auth_headers(cfg: &ConnectorCfg, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match &cfg.hub_token {
        Some(t) => req.bearer_auth(t),
        None => req,
    }
}

pub async fn health(http: &reqwest::Client, cfg: &ConnectorCfg) -> &'static str {
    let url = format!("{}/api/health", cfg.hub_url.trim_end_matches('/'));
    let ok = http
        .get(url)
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);
    if ok {
        "up"
    } else {
        "down"
    }
}

async fn post_scan(
    http: &reqwest::Client,
    cfg: &ConnectorCfg,
    kind: &str,
    value: &str,
) -> Result<String, String> {
    let url = format!("{}/api/scan", cfg.hub_url.trim_end_matches('/'));
    // hub contract (backend/app.py): multi expects value:{username?,phone?,email?};
    // image is a single-type scan.
    let body = match kind {
        "image" => json!({"type": "image", "value": value, "tools": tools_for(kind), "options": {}}),
        k @ ("username" | "phone" | "email") => {
            json!({"type": "multi", "value": {k: value}, "tools": tools_for(k), "options": {}})
        }
        other => {
            return Err(format!("unsupported scan kind {other:?} (username|phone|email|image)"));
        }
    };
    let resp = auth_headers(cfg, http.post(url).json(&body))
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("hub unreachable ({e})"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("hub scan rejected ({status}): {}", truncate(&text, 200)));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("bad hub JSON ({e})"))?;
    v.get("job_id")
        .and_then(|j| j.as_str())
        .map(String::from)
        .ok_or_else(|| format!("hub response missing job_id: {}", truncate(&text, 200)))
}

async fn poll_job<L: FnMut(&str)>(
    http: &reqwest::Client,
    cfg: &ConnectorCfg,
    job_id: &str,
    mut log: L,
) -> Result<HubJob, String> {
    let url = format!("{}/api/jobs/{job_id}", cfg.hub_url.trim_end_matches('/'));
    // fast polls first (tests + snappy short jobs), then settle to 2s; cap ~20min
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20 * 60);
    let mut attempt = 0u32;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err("hub job timed out (20min)".into());
        }
        attempt += 1;
        let resp = auth_headers(cfg, http.get(&url))
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await;
        match resp {
            Ok(r) if r.status().is_success() => {
                let v: Value = r.json().await.map_err(|e| format!("bad job JSON ({e})"))?;
                let status = v
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                if status == "done" || status == "error" || status == "cancelled" {
                    return Ok(parse_job(&v));
                }
                if attempt.is_multiple_of(10) {
                    log(&format!("hub job {job_id}: status={status}"));
                }
            }
            Ok(r) => log(&format!("hub poll HTTP {}", r.status())),
            Err(e) => log(&format!("hub poll error ({e})")),
        }
        let wait = if attempt <= 20 { 400 } else { 2000 };
        tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
    }
}

fn parse_job(v: &Value) -> HubJob {
    let mut results = Vec::new();
    if let Some(arr) = v.get("results").and_then(|r| r.as_array()) {
        for r in arr {
            let found = r
                .get("found")
                .and_then(|f| f.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|h| {
                            let site = h.get("site").and_then(|s| s.as_str())?;
                            let url = h.get("url").and_then(|u| u.as_str()).unwrap_or("");
                            Some((site.to_string(), url.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let info = r
                .get("info")
                .and_then(|f| f.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|kv| {
                            let k = kv.get("k").and_then(|s| s.as_str())?;
                            let val = kv.get("v").and_then(|s| s.as_str()).unwrap_or("");
                            Some((k.to_string(), val.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            results.push(HubResult {
                qtype: r
                    .get("qtype")
                    .and_then(|s| s.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                tool: r
                    .get("tool")
                    .and_then(|s| s.as_str())
                    .unwrap_or("hub")
                    .to_string(),
                status: r
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                found,
                info,
                error: r
                    .get("error")
                    .and_then(|s| s.as_str())
                    .map(String::from),
            });
        }
    }
    HubJob {
        id: v
            .get("id")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown")
            .to_string(),
        status: v
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown")
            .to_string(),
        results,
    }
}

fn qtype_to_etype(qtype: &str) -> Option<EType> {
    match qtype {
        "username" => Some(EType::Username),
        "phone" => Some(EType::Phone),
        "email" => Some(EType::Email),
        "image" => Some(EType::Image),
        _ => None,
    }
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Run a multi-scan and ingest everything into the case.
#[allow(clippy::too_many_arguments)]
pub async fn scan_and_ingest(
    http: &reqwest::Client,
    cfg: &ConnectorCfg,
    store: &Store,
    case_id: &str,
    target: Option<&nexus_core::Entity>,
    kind: &str,
    value: &str,
    jev_gate: bool,
    mut log: impl FnMut(&str),
) -> Result<(String, IngestCounts), String> {
    if kind == "username" && value.split_whitespace().count() > 1 {
        return Err(
            "username scans need a single handle (no spaces). For full names use              ▶ openplanter research or ▶ web pivots instead — or add the person's              username as its own entity."
                .into(),
        );
    }
    let job_id = post_scan(http, cfg, kind, value).await?;
    log(&format!("hub job {job_id} queued for {kind} “{value}”"));
    let job = poll_job(http, cfg, &job_id, &mut log).await?;
    let mut counts = IngestCounts::default();

    for res in &job.results {
        if let Some(err) = &res.error {
            log(&format!("hub {}/{} error: {}", res.tool, res.qtype, truncate(err, 120)));
        }
        let Some(hit_etype) = qtype_to_etype(&res.qtype) else {
            continue;
        };
        let tool_hits = res.found.len();
        for (site, url) in &res.found {
            // optional calibrated noise gate — FLAG, never drop
            let mut confidence = 0.9f64;
            let mut note = String::new();
            if jev_gate && crate::connectors::jev::key_present(cfg) {
                let state = format!(
                    "OSINT username scan hit. Tool: {}. Site: {}. URL: {url}. Searched value: “{value}”.",
                    res.tool, site
                );
                match crate::connectors::jev::noul(
                    http,
                    cfg,
                    &state,
                    "Is this likely a GENUINE profile/account match (not a coincidental or false hit)?",
                )
                .await
                {
                    Ok((yes, conf)) => {
                        if !yes {
                            confidence = 0.5;
                            note = format!("jev gate flagged (conf {conf:.2}) — verify manually");
                            log(&format!("jev gate flagged {site}: {note}"));
                        }
                    }
                    Err(e) => log(&format!("jev gate unavailable ({e})")),
                }
            }

            // entity (dedup via normalized key)
            let outcome = store
                .add_entity(
                    case_id,
                    NewEntity::new(hit_etype, site)
                        .with_confidence(confidence)
                        .with_data(json!({"platform": site, "url": url, "scan_value": value})),
                )
                .map_err(|e| e.to_string())?;
            let hit_id = outcome.entity.id.clone();
            if !outcome.deduplicated {
                counts.entities += 1;
            }

            // relation from the scanned target (when in-case)
            if let Some(t) = target {
                if store
                    .add_relation(
                        NewRelation::new(case_id, &t.id, &hit_id, "uses")
                            .with_confidence(confidence)
                            .with_source(format!("hub:{}", res.tool))
                            .with_evidence(json!({"title": format!("{}: {}", res.tool, site), "url": url})),
                    )
                    .is_ok()
                {
                    counts.relations += 1;
                }
            }

            // evidence
            let ev = store
                .add_evidence(
                    NewEvidence::new(case_id, &hit_id, "tool_result")
                        .with_title(format!("{}: {}", res.tool, site))
                        .with_url(url)
                        .with_snippet(note.as_str())
                        .with_raw(json!({"tool": res.tool, "site": site, "qtype": res.qtype, "job": job.id}))
                        .with_confidence(confidence),
                )
                .map_err(|e| e.to_string())?;
            counts.evidence += 1;
            let _ = ev;
        }

        // phoneinfoga-style key facts → target entity data (only new keys)
        if !res.info.is_empty() {
            if let Some(t) = target {
                if let Ok(Some(mut ent)) = store.get_entity(&t.id) {
                    if let Some(obj) = ent.data.as_object_mut() {
                        for (k, v) in &res.info {
                            obj.entry(k.clone()).or_insert(json!(v));
                        }
                    }
                    let _ = store.update_entity(
                        &t.id,
                        nexus_core::EntityPatch {
                            label: None,
                            data: Some(ent.data.clone()),
                            pinned: None,
                            confidence: None,
                        },
                    );
                }
            }
        }

        let _ = store.add_timeline(
            NewEvent::new(case_id, "scan", format!("hub scan {}: {tool_hits} hits", res.tool))
                .with_entity(if let Some(t) = target { t.id.clone() } else { String::new() })
                .with_detail(json!({"tool": res.tool, "qtype": res.qtype, "hits": tool_hits}))
                .with_source("hub"),
        );
    }

    let total: usize = job.results.iter().map(|r| r.found.len()).sum();
    Ok((format!("{total} sites hit across {} tool results", job.results.len()), counts))
}
