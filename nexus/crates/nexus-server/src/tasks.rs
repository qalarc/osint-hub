//! Task engine: async connector runs with live SSE logs.
//! Tasks are in-memory (restart clears the queue — case data persists).

use crate::state::AppState;
use nexus_core::{NewEntity, Store};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::{broadcast, mpsc};

#[derive(Debug, Clone)]
pub struct LogLine {
    pub ts: String,
    pub level: String,
    pub msg: String,
}

pub struct TaskRec {
    pub id: String,
    pub case_id: String,
    pub kind: String,
    pub status: Mutex<String>, // queued|running|done|error
    pub target: Option<String>,
    pub params: Value,
    pub created_at: String,
    pub finished_at: Mutex<Option<String>>,
    pub summary: Mutex<String>,
    pub ingested: Mutex<crate::connectors::IngestCounts>,
    pub logs: Mutex<Vec<LogLine>>,
    pub log_tx: broadcast::Sender<String>,
}

impl TaskRec {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "case_id": self.case_id,
            "kind": self.kind,
            "status": self.status.lock().unwrap().clone(),
            "target": self.target,
            "params": self.params,
            "created_at": self.created_at,
            "finished_at": self.finished_at.lock().unwrap().clone(),
            "summary": self.summary.lock().unwrap().clone(),
            "ingested": self.ingested.lock().unwrap().clone(),
            "logs": self.logs.lock().unwrap().iter().map(|l| json!({
                "task_id": self.id, "level": l.level, "msg": l.msg, "ts": l.ts,
            })).collect::<Vec<_>>(),
        })
    }

    pub fn snapshot(&self) -> Value {
        let mut v = self.to_json();
        v["logs"] = json!([]); // snapshot without log bulk
        v
    }
}

pub struct TaskEngine {
    tx: mpsc::Sender<Arc<TaskRec>>,
    reg: RwLock<Vec<Arc<TaskRec>>>,
}

impl TaskEngine {
    pub fn new() -> (Arc<Self>, mpsc::Receiver<Arc<TaskRec>>) {
        let (tx, rx) = mpsc::channel(64);
        (Arc::new(Self { tx, reg: RwLock::new(Vec::new()) }), rx)
    }

    pub fn spawn_task(
        &self,
        case_id: &str,
        kind: &str,
        target: Option<String>,
        params: Value,
    ) -> Arc<TaskRec> {
        let (log_tx, _) = broadcast::channel(1024);
        let rec = Arc::new(TaskRec {
            id: nexus_core::new_id("t"),
            case_id: case_id.to_string(),
            kind: kind.to_string(),
            status: Mutex::new("queued".into()),
            target,
            params,
            created_at: nexus_core::now_iso(),
            finished_at: Mutex::new(None),
            summary: Mutex::new(String::new()),
            ingested: Mutex::new(Default::default()),
            logs: Mutex::new(Vec::new()),
            log_tx,
        });
        {
            let mut reg = self.reg.write().unwrap();
            reg.insert(0, rec.clone());
            reg.truncate(100);
        }
        if self.tx.try_send(rec.clone()).is_err() {
            *rec.status.lock().unwrap() = "error".into();
            *rec.summary.lock().unwrap() = "task queue full".into();
        }
        rec
    }

    pub fn get(&self, id: &str) -> Option<Arc<TaskRec>> {
        self.reg
            .read()
            .unwrap()
            .iter()
            .find(|t| t.id == id)
            .cloned()
    }

    pub fn list(&self, case_id: Option<&str>, limit: usize) -> Vec<Arc<TaskRec>> {
        self.reg
            .read()
            .unwrap()
            .iter()
            .filter(|t| case_id.map(|c| t.case_id == c).unwrap_or(true))
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn latest_for_case(&self, case_id: &str) -> Option<Arc<TaskRec>> {
        self.list(Some(case_id), 1).into_iter().next()
    }
}

/// Log into the record + push to live subscribers.
pub fn log_line(rec: &TaskRec, level: &str, msg: &str) {
    let line = LogLine {
        ts: nexus_core::now_iso(),
        level: level.to_string(),
        msg: msg.to_string(),
    };
    {
        let mut logs = rec.logs.lock().unwrap();
        logs.push(line.clone());
        if logs.len() > 500 {
            let drop = logs.len() - 500;
            logs.drain(0..drop);
        }
    }
    let _ = rec.log_tx.send(serde_json::json!({
        "task_id": rec.id, "level": line.level, "msg": line.msg, "ts": line.ts,
    }).to_string());
}

/// Worker loop — two of these run for the lifetime of the server.
pub async fn worker(state: Arc<AppState>, mut rx: mpsc::Receiver<Arc<TaskRec>>) {
    while let Some(rec) = rx.recv().await {
        run_task(state.clone(), rec).await;
    }
}

/// Worker entry — public because the dispatcher in state.rs invokes it.
pub async fn run_task_public(state: Arc<AppState>, rec: Arc<TaskRec>) {
    run_task(state, rec).await;
}

async fn run_task(state: Arc<AppState>, rec: Arc<TaskRec>) {
    *rec.status.lock().unwrap() = "running".into();
    state.bus.emit("task_update", &rec.case_id);
    let case = state.store.get_case(&rec.case_id).ok().flatten();
    let case_slug = case.as_ref().map(|c| c.slug.clone()).unwrap_or_default();

    let cfg = state.connectors();
    let kind = rec.kind.as_str();
    let target = rec
        .target
        .as_deref()
        .and_then(|id| state.store.get_entity(id).ok().flatten());

    let outcome: Result<(String, crate::connectors::IngestCounts), String> = match kind {
        "hub_scan" => run_hub_scan(&state, &cfg, &rec, target.as_ref()).await,
        "flowsint_enrich" => run_flowsint(&state, &cfg, &rec, target.as_ref()).await,
        "openplanter_research" => run_openplanter(&state, &cfg, &rec, target.as_ref(), &case_slug).await,
        "archon_ground" => run_archon(&state, &cfg, &rec, target.as_ref()).await,
        "web_pivots" => run_web_pivots(&state, &rec, target.as_ref()),
        other => Err(format!("unknown task kind: {other}")),
    };

    match outcome {
        Ok((summary, counts)) => {
            *rec.status.lock().unwrap() = "done".into();
            *rec.summary.lock().unwrap() = summary.clone();
            *rec.ingested.lock().unwrap() = counts;
            log_line(&rec, "info", &format!("done: {summary}"));
        }
        Err(e) => {
            *rec.status.lock().unwrap() = "error".into();
            *rec.summary.lock().unwrap() = e.clone();
            log_line(&rec, "error", &e);
        }
    }
    *rec.finished_at.lock().unwrap() = Some(nexus_core::now_iso());
    state.bus.emit("task_update", &rec.case_id);
    state.bus.emit("case_updated", &rec.case_id);
}

fn param_str(rec: &TaskRec, key: &str) -> Option<String> {
    rec.params.get(key).and_then(|v| v.as_str()).map(String::from)
}

async fn run_hub_scan(
    state: &Arc<AppState>,
    cfg: &crate::config::ConnectorCfg,
    rec: &Arc<TaskRec>,
    target: Option<&nexus_core::Entity>,
) -> Result<(String, crate::connectors::IngestCounts), String> {
    let (kind, value) = match (param_str(rec, "scan"), param_str(rec, "value")) {
        (Some(scan), Some(v)) => (scan, v),
        (Some(scan), None) => {
            let t = target.ok_or("hub_scan: value or target entity required")?;
            (scan, t.label.clone())
        }
        (None, Some(v)) => ("username".to_string(), v),
        (None, None) => {
            let t = target.ok_or("hub_scan: value or target entity required")?;
            (crate::connectors::hub::scan_kind_for_etype(t.etype.as_str()).to_string(), t.label.clone())
        }
    };
    let logf = |m: &str| log_line(rec, "info", m);
    let jev_gate = rec.params.get("jev_gate").and_then(|v| v.as_bool()).unwrap_or(false);
    crate::connectors::hub::scan_and_ingest(
        &state.http, cfg, &state.store, &rec.case_id, target, &kind, &value, jev_gate, logf,
    )
    .await
}

async fn run_flowsint(
    state: &Arc<AppState>,
    cfg: &crate::config::ConnectorCfg,
    rec: &Arc<TaskRec>,
    target: Option<&nexus_core::Entity>,
) -> Result<(String, crate::connectors::IngestCounts), String> {
    if cfg.flowsint_url.is_none() {
        return Ok(("flowsint unavailable (FLOWSINT_URL unset)".into(), Default::default()));
    }
    let t = target.ok_or("flowsint_enrich: target entity required")?;
    let enrichers: Vec<String> = rec
        .params
        .get("enrichers")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_else(|| crate::connectors::flowsint::default_enrichers(t.etype.as_str()));
    if enrichers.is_empty() {
        return Err(format!("flowsint_enrich: no enrichers for etype {}", t.etype));
    }
    let case_name = state
        .store
        .get_case(&rec.case_id)
        .ok()
        .flatten()
        .map(|c| c.name)
        .unwrap_or_else(|| rec.case_id.clone());
    let mut total = crate::connectors::IngestCounts::default();
    let mut summaries = Vec::new();
    for enr in &enrichers {
        log_line(rec, "info", &format!("running enricher {enr}"));
        let logf = |m: &str| log_line(rec, "info", m);
        match crate::connectors::flowsint::run_enricher(
            &state.http, cfg, &state.store, &rec.case_id, &case_name, target, t.etype.as_str(),
            &t.label, enr, logf,
        )
        .await
        {
            Ok((s, c)) => {
                total.merge(&c);
                summaries.push(s);
            }
            Err(e) => summaries.push(format!("{enr}: {e}")),
        }
    }
    Ok((summaries.join("; "), total))
}

async fn run_openplanter(
    state: &Arc<AppState>,
    cfg: &crate::config::ConnectorCfg,
    rec: &Arc<TaskRec>,
    target: Option<&nexus_core::Entity>,
    case_slug: &str,
) -> Result<(String, crate::connectors::IngestCounts), String> {
    let question = param_str(rec, "question")
        .or_else(|| target.map(|t| format!("Investigate {} ({}): gather background, connections and sourcing.", t.label, t.etype)))
        .ok_or("openplanter_research: question or target required")?;
    let use_ollama = std::env::var("OPENPLANTER_PROVIDER")
        .map(|v| v == "ollama")
        .unwrap_or(false);
    let logf = |m: &str| log_line(rec, "info", m);
    crate::connectors::openplanter::research(
        cfg, &state.store, &rec.case_id, case_slug, &rec.id, target, &question, use_ollama, logf,
    )
    .await
}

async fn run_archon(
    state: &Arc<AppState>,
    cfg: &crate::config::ConnectorCfg,
    rec: &Arc<TaskRec>,
    target: Option<&nexus_core::Entity>,
) -> Result<(String, crate::connectors::IngestCounts), String> {
    let question = param_str(rec, "question")
        .or_else(|| target.map(|t| format!("What is known about {} ({}) in canon?", t.label, t.etype)))
        .ok_or("archon_ground: question or target required")?;
    let sandbox = param_str(rec, "sandbox");
    let top_k = rec.params.get("top_k").and_then(|v| v.as_u64()).unwrap_or(6) as u32;

    let ans = crate::connectors::archon::ask(&state.http, cfg, &question, sandbox.as_deref(), top_k).await;
    let mut counts = crate::connectors::IngestCounts::default();
    if !ans.ok {
        return Ok((format!("archon: {}", ans.detail), counts));
    }
    // attach as evidence (to target entity or a case note entity)
    let subject = match target {
        Some(t) => t.id.clone(),
        None => {
            let o = state
                .store
                .add_entity(
                    &rec.case_id,
                    NewEntity::new(nexus_core::EType::Note, format!("Archon: {}", question.chars().take(50).collect::<String>()))
                        .with_confidence(0.9)
                        .with_data(json!({"source": "archon"})),
                )
                .map_err(|e| e.to_string())?;
            if !o.deduplicated {
                counts.entities += 1;
            }
            o.entity.id
        }
    };
    if state
        .store
        .add_evidence(
            nexus_core::NewEvidence::new(&rec.case_id, &subject, "archon_answer")
                .with_title(format!("archon: {}", question.chars().take(70).collect::<String>()))
                .with_snippet(ans.answer.chars().take(800).collect::<String>())
                .with_raw(json!({"citations": ans.citations, "backend": ans.backend, "found": ans.found}))
                .with_confidence(0.75),
        )
        .is_ok()
    {
        counts.evidence += 1;
    }
    let _ = state.store.add_timeline(
        nexus_core::NewEvent::new(&rec.case_id, "grounding", format!("archon grounding: {}", question.chars().take(60).collect::<String>()))
            .with_entity(subject)
            .with_source("archon"),
    );
    Ok((
        format!("archon answered ({} citations, backend {})", ans.citations.len(), ans.backend.as_deref().unwrap_or("?")),
        counts,
    ))
}

fn run_web_pivots(
    state: &Arc<AppState>,
    rec: &Arc<TaskRec>,
    target: Option<&nexus_core::Entity>,
) -> Result<(String, crate::connectors::IngestCounts), String> {
    let (etype, label, data) = match target {
        Some(t) => (t.etype.as_str().to_string(), t.label.clone(), t.data.clone()),
        None => (
            param_str(rec, "etype").unwrap_or_else(|| "topic".into()),
            param_str(rec, "value").ok_or("web_pivots: value or target required")?,
            json!({}),
        ),
    };
    let pivots = crate::connectors::websearch::pivots_json(&etype, &label, &data);
    let o = state
        .store
        .add_entity(
            &rec.case_id,
            NewEntity::new(nexus_core::EType::Note, format!("Search pivots — {label}"))
                .with_confidence(1.0)
                .with_data(json!({"pivots": pivots, "of": label, "etype": etype})),
        )
        .map_err(|e| e.to_string())?;
    let mut counts = crate::connectors::IngestCounts::default();
    if !o.deduplicated {
        counts.entities += 1;
    }
    let n = pivots.as_array().map(|a| a.len()).unwrap_or(0);
    Ok((format!("{n} search pivots recorded"), counts))
}

/// Re-export for routes: run a triage with the engine chain + decision log.
pub async fn triage_case(state: &Arc<AppState>, case_id: &str) -> Result<(String, Vec<crate::connectors::jev::TriageItem>), String> {
    let cfg = state.connectors();
    let g = nexus_core::load_graph(&state.store, case_id).map_err(|e| e.to_string())?;
    let entities: Vec<nexus_core::Entity> = g
        .nodes
        .iter()
        .filter(|e| !e.pinned)
        .cloned()
        .collect();
    let outcome = crate::connectors::laya::triage(&state.http, &cfg, &g, &entities).await;
    let _ = Store::list_cases(&state.store); // keep store warm (cheap)
    crate::connectors::laya::log_decision(
        cfg.laya_log.as_deref(),
        &json!({
            "case_id": case_id,
            "engine": outcome.engine,
            "top": outcome.ranked.first().map(|r| json!({"entity_id": r.entity_id, "score": r.score})),
            "n": outcome.ranked.len(),
        }),
    );
    Ok((outcome.engine.to_string(), outcome.ranked))
}
