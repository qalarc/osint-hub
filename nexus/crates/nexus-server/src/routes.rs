//! All HTTP routes (CONTRACT §4).

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::sse::Sse;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, get_service};
use axum::middleware::{self, Next};
use axum::extract::Request;
use axum::http::header::AUTHORIZATION;
use tower_http::cors::CorsLayer;
use axum::{Json, Router};
use nexus_core::{
    AddOutcome, EntityPatch, EType, Graph, NewEntity, NewEvent, NewEvidence, NewRelation,
};
use serde_json::{json, Value};
use futures::StreamExt;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

// ─────────────────────────────────────────────────────────────── helpers ──

fn jstr(body: &Value, key: &str) -> Option<String> {
    body.get(key).and_then(|v| v.as_str()).map(String::from)
}

fn jf64(body: &Value, key: &str) -> Option<f64> {
    body.get(key).and_then(|v| v.as_f64())
}

fn jobj(body: &Value, key: &str) -> Option<Value> {
    body.get(key).cloned().filter(|v| v.is_object())
}

fn require_case(state: &AppState, id: &str) -> ApiResult<nexus_core::Case> {
    state
        .store
        .get_case(id)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("case {id}")))
}

fn require_entity(state: &AppState, id: &str) -> ApiResult<nexus_core::Entity> {
    state
        .store
        .get_entity(id)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("entity {id}")))
}

fn md_response(markdown: String) -> Response {
    (
        [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
        markdown,
    )
        .into_response()
}

// ──────────────────────────────────────────────────────────────── health ──

async fn health(State(state): State<Arc<AppState>>) -> Json<Value> {
    let cfg = state.connectors();
    // probe all connectors concurrently — sequential probes make /api/health
    // take 10-20s when several connectors are down
    let (hub, flowsint, openplanter, archon, laya, jev) = tokio::join!(
        crate::connectors::hub::health(&state.http, &cfg),
        crate::connectors::flowsint::health(&state.http, &cfg),
        crate::connectors::openplanter::health(&state.http, &cfg),
        crate::connectors::archon::health(&state.http, &cfg),
        crate::connectors::laya::health(&state.http, &cfg),
        crate::connectors::jev::health(&state.http, &cfg),
    );
    Json(json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "connectors": {
            "hub": hub, "flowsint": flowsint, "openplanter": openplanter,
            "archon": archon, "laya": laya, "jev": jev,
        }
    }))
}

async fn events(State(state): State<Arc<AppState>>) -> Sse<impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>> {
    let rx = state.bus.subscribe();
    crate::bus::sse_stream(rx)
}

// ────────────────────────────────────────────────────────────────── cases ──

async fn list_cases(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
    let cases = state.store.list_cases()?;
    Ok(Json(json!({"cases": cases})))
}

async fn create_case(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let name = jstr(&body, "name").filter(|n| !n.trim().is_empty());
    let Some(name) = name else {
        return Err(ApiError::Invalid("name is required".into()));
    };
    let notes = jstr(&body, "notes").unwrap_or_default();
    let tags: Vec<String> = body
        .get("tags")
        .and_then(|t| t.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let case = state.store.create_case(&name, &notes, &tags)?;
    state.bus.emit("case_updated", &case.id);
    Ok((StatusCode::CREATED, Json(json!({"case": case}))).into_response())
}

async fn get_case(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let case = require_case(&state, &id)?;
    let entities = state.store.list_entities(&id)?.len();
    let relations = state.store.list_relations(&id)?.len();
    let evidence = state.store.list_case_evidence(&id)?.len();
    let tasks = state.tasks.list(Some(&id), 100).len();
    Ok(Json(json!({
        "case": case,
        "stats": {"entities": entities, "relations": relations, "evidence": evidence, "tasks": tasks},
    })))
}

async fn patch_case(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    require_case(&state, &id)?;
    state.store.update_case(&id, jstr(&body, "status").as_deref(), jstr(&body, "notes").as_deref())?;
    let case = require_case(&state, &id)?;
    state.bus.emit("case_updated", &id);
    Ok(Json(json!({"case": case})))
}

async fn delete_case(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    require_case(&state, &id)?;
    state.store.delete_case(&id)?;
    state.bus.emit("case_updated", &id);
    Ok(Json(json!({"ok": true})))
}

async fn case_graph(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    require_case(&state, &id)?;
    let g: Graph = nexus_core::load_graph(&state.store, &id)?;
    let degree = nexus_core::degree_centrality(&g);
    let pagerank = nexus_core::pagerank(&g, 0.85, 100);
    let communities = nexus_core::communities(&g);
    let components: Vec<Vec<String>> = nexus_core::components(&g);
    Ok(Json(json!({
        "nodes": g.nodes,
        "edges": g.edges,
        "analysis": {
            "degree": degree,
            "pagerank": pagerank,
            "communities": communities,
            "components": components,
        },
    })))
}

async fn case_path(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    require_case(&state, &id)?;
    let (Some(a), Some(b)) = (q.get("a"), q.get("b")) else {
        return Err(ApiError::Invalid("query params a and b required".into()));
    };
    let g = nexus_core::load_graph(&state.store, &id)?;
    let path = nexus_core::shortest_path(&g, a, b);
    Ok(Json(json!({"path": path})))
}

async fn case_pivots(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    require_case(&state, &id)?;
    let g = nexus_core::load_graph(&state.store, &id)?;
    let cfg = state.connectors();
    let mut connectors: Vec<&str> = vec!["hub"];
    if cfg.flowsint_url.is_some() {
        connectors.push("flowsint");
    }
    if crate::connectors::openplanter::resolve_bin(&cfg).is_some() {
        connectors.push("openplanter");
    }
    if crate::connectors::archon::health(&state.http, &cfg).await == "up" {
        connectors.push("archon");
    }
    let pivots = match q.get("entity") {
        Some(e) => nexus_core::suggest_pivots(&g, e, &connectors),
        None => {
            // case-wide: best suggestion per entity, merged + resorted
            let mut all: Vec<nexus_core::PivotSuggestion> = Vec::new();
            for n in &g.nodes {
                all.extend(nexus_core::suggest_pivots(&g, &n.id, &connectors));
            }
            all.sort_by(|a, b| b.priority.partial_cmp(&a.priority).unwrap_or(std::cmp::Ordering::Equal));
            all.truncate(8);
            all
        }
    };
    Ok(Json(json!({"pivots": pivots})))
}

// ─────────────────────────────────────────────────────────────── entities ──

async fn add_entity(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    require_case(&state, &id)?;
    let etype_s = jstr(&body, "etype").ok_or_else(|| ApiError::Invalid("etype required".into()))?;
    let etype = EType::from_str(&etype_s)
        .ok_or_else(|| ApiError::Invalid(format!("unknown etype {etype_s}")))?;
    let label = jstr(&body, "label").filter(|l| !l.trim().is_empty());
    let Some(label) = label else {
        return Err(ApiError::Invalid("label required".into()));
    };
    let mut new = NewEntity::new(etype, &label);
    if let Some(d) = jobj(&body, "data") {
        new = new.with_data(d);
    }
    if let Some(c) = jf64(&body, "confidence") {
        new = new.with_confidence(c);
    }
    let AddOutcome { entity, deduplicated } = state.store.add_entity(&id, new)?;
    if !deduplicated {
        let _ = state.store.render_doc(&entity.id);
    }
    state.bus.emit("entity_added", &id);
    Ok((
        StatusCode::CREATED,
        Json(json!({"entity": entity, "deduplicated": deduplicated})),
    )
        .into_response())
}

fn entity_full(state: &AppState, id: &str) -> ApiResult<Value> {
    let entity = require_entity(state, id)?;
    let relations = state.store.list_entity_relations(id)?;
    let evidence = state.store.list_evidence(id)?;
    let doc = state.store.get_doc(id)?;
    Ok(json!({
        "entity": entity,
        "relations": relations,
        "evidence": evidence,
        "doc": doc,
    }))
}

async fn get_entity(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(entity_full(&state, &id)?))
}

async fn patch_entity(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    require_entity(&state, &id)?;
    let patch = EntityPatch {
        label: jstr(&body, "label"),
        data: jobj(&body, "data"),
        pinned: body.get("pinned").and_then(|p| p.as_bool()),
        confidence: jf64(&body, "confidence"),
    };
    let entity = state.store.update_entity(&id, patch)?;
    state
        .bus
        .emit("case_updated", &state.store.get_entity(&id)?.map(|e| e.case_id).unwrap_or_default());
    Ok(Json(json!({"entity": entity})))
}

async fn delete_entity(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let entity = require_entity(&state, &id)?;
    state.store.delete_entity(&id)?;
    state.bus.emit("case_updated", &entity.case_id);
    Ok(Json(json!({"ok": true})))
}

async fn merge_entity(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let other = jstr(&body, "other").ok_or_else(|| ApiError::Invalid("other required".into()))?;
    let entity = require_entity(&state, &id)?;
    require_entity(&state, &other)?;
    let merged = state.store.merge_entities(&id, &other)?;
    state.bus.emit("case_updated", &entity.case_id);
    Ok(Json(json!({"entity": merged})))
}

async fn merge_preview(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let other = jstr(&body, "other").ok_or_else(|| ApiError::Invalid("other required".into()))?;
    let a = require_entity(&state, &id)?;
    let b = require_entity(&state, &other)?;
    let similarity = nexus_core::similarity(&a.label, &b.label);
    let cfg = state.connectors();
    let (verdict, confidence, engine) = if (0.82..=0.97).contains(&similarity) && crate::connectors::jev::key_present(&cfg) {
        let state_str = format!(
            "Record A: type={} label={:?} data={}. Record B: type={} label={:?} data={}.",
            a.etype, a.label, a.data, b.etype, b.label, b.data
        );
        match crate::connectors::jev::noul(
            &state.http,
            &cfg,
            &state_str,
            "These two records refer to the same real-world entity.",
        )
        .await
        {
            Ok((yes, conf)) => {
                let v = if yes && conf >= 0.8 {
                    "same"
                } else if yes || conf >= 0.5 {
                    "related"
                } else {
                    "distinct"
                };
                (v, conf, "jev")
            }
            Err(e) => {
                tracing::debug!("jev merge gate unavailable: {e}");
                (heuristic_verdict(similarity), similarity, "heuristic")
            }
        }
    } else {
        (heuristic_verdict(similarity), similarity, "heuristic")
    };
    Ok(Json(json!({
        "similarity": similarity,
        "verdict": verdict,
        "confidence": confidence,
        "engine": engine,
    })))
}

fn heuristic_verdict(similarity: f64) -> &'static str {
    if similarity >= 0.97 {
        "same"
    } else if similarity >= 0.82 {
        "related"
    } else {
        "distinct"
    }
}

async fn entity_evidence(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    require_entity(&state, &id)?;
    Ok(Json(json!({"evidence": state.store.list_evidence(&id)?})))
}

async fn get_doc(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    require_entity(&state, &id)?;
    Ok(Json(json!({"doc": state.store.get_doc(&id)?})))
}

async fn set_doc(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    require_entity(&state, &id)?;
    let md = jstr(&body, "markdown").ok_or_else(|| ApiError::Invalid("markdown required".into()))?;
    state.store.set_doc(&id, &md)?;
    Ok(Json(json!({"doc": md})))
}

async fn render_doc(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    require_entity(&state, &id)?;
    let doc = state.store.render_doc(&id)?;
    Ok(Json(json!({"doc": doc})))
}

// ────────────────────────────────────────────────────────────── relations ──

async fn add_relation(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let case_id = jstr(&body, "case_id").ok_or_else(|| ApiError::Invalid("case_id required".into()))?;
    require_case(&state, &case_id)?;
    let src = jstr(&body, "src").ok_or_else(|| ApiError::Invalid("src required".into()))?;
    let dst = jstr(&body, "dst").ok_or_else(|| ApiError::Invalid("dst required".into()))?;
    let rel = jstr(&body, "rel").ok_or_else(|| ApiError::Invalid("rel required".into()))?;
    let mut new = NewRelation::new(&case_id, &src, &dst, &rel);
    if let Some(w) = jf64(&body, "weight") {
        new = new.with_weight(w);
    }
    new = new.with_confidence(jf64(&body, "confidence").unwrap_or(0.9));
    if let Some(s) = jstr(&body, "source") {
        new = new.with_source(s);
    }
    if let Some(ev) = body.get("evidence").filter(|v| v.is_object()) {
        new = new.with_evidence(ev.clone());
    }
    let relation = state.store.add_relation(new)?;
    state.bus.emit("case_updated", &case_id);
    Ok((StatusCode::CREATED, Json(json!({"relation": relation}))).into_response())
}

async fn delete_relation(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    state.store.delete_relation(&id)?;
    Ok(Json(json!({"ok": true})))
}

// ─────────────────────────────────────────────────────────────── evidence ──

async fn add_evidence(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let case_id = jstr(&body, "case_id").ok_or_else(|| ApiError::Invalid("case_id required".into()))?;
    require_case(&state, &case_id)?;
    let subject = jstr(&body, "subject").ok_or_else(|| ApiError::Invalid("subject required".into()))?;
    let kind = jstr(&body, "kind").unwrap_or_else(|| "manual".into());
    let mut new = NewEvidence::new(&case_id, &subject, &kind);
    if let Some(t) = jstr(&body, "title") {
        new = new.with_title(t);
    }
    if let Some(u) = jstr(&body, "url") {
        new = new.with_url(u);
    }
    if let Some(s) = jstr(&body, "snippet") {
        new = new.with_snippet(s);
    }
    if let Some(r) = jobj(&body, "raw") {
        new = new.with_raw(r);
    }
    if let Some(c) = jf64(&body, "confidence") {
        new = new.with_confidence(c);
    }
    let evidence = state.store.add_evidence(new)?;
    state.bus.emit("case_updated", &case_id);
    Ok((StatusCode::CREATED, Json(json!({"evidence": evidence}))).into_response())
}

// ─────────────────────────────────────────────────────────────── timeline ──

async fn get_timeline(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    require_case(&state, &id)?;
    Ok(Json(json!({"events": state.store.list_timeline(&id)?})))
}

async fn add_timeline(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    require_case(&state, &id)?;
    let kind = jstr(&body, "kind").unwrap_or_else(|| "note".into());
    let title = jstr(&body, "title").ok_or_else(|| ApiError::Invalid("title required".into()))?;
    let mut new = NewEvent::new(&id, &kind, &title);
    if let Some(ts) = jstr(&body, "ts") {
        new = new.with_ts(ts);
    }
    if let Some(e) = jstr(&body, "entity_id") {
        new = new.with_entity(e);
    }
    if let Some(d) = jobj(&body, "detail") {
        new = new.with_detail(d);
    }
    if let Some(s) = jstr(&body, "source") {
        new = new.with_source(s);
    }
    let event = state.store.add_timeline(new)?;
    state.bus.emit("case_updated", &id);
    Ok((StatusCode::CREATED, Json(json!({"event": event}))).into_response())
}

// ────────────────────────────────────────────────────────────────── tasks ──

async fn create_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    require_case(&state, &id)?;
    let kind = jstr(&body, "kind").ok_or_else(|| ApiError::Invalid("kind required".into()))?;
    let valid = [
        "hub_scan",
        "flowsint_enrich",
        "openplanter_research",
        "archon_ground",
        "web_pivots",
    ];
    if !valid.contains(&kind.as_str()) {
        return Err(ApiError::Invalid(format!(
            "kind must be one of {valid:?}"
        )));
    }
    let rec = state.tasks.spawn_task(
        &id,
        &kind,
        jstr(&body, "target"),
        body.get("params").cloned().unwrap_or(json!({})),
    );
    state.bus.emit("task_update", &id);
    Ok((StatusCode::ACCEPTED, Json(json!({"task": rec.snapshot()}))).into_response())
}

async fn list_tasks(
    State(state): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<Value> {
    let tasks = state
        .tasks
        .list(q.get("case_id").map(|s| s.as_str()), 100)
        .iter()
        .map(|t| t.snapshot())
        .collect::<Vec<_>>();
    Json(json!({"tasks": tasks}))
}

async fn get_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let rec = state
        .tasks
        .get(&id)
        .ok_or_else(|| ApiError::NotFound(format!("task {id}")))?;
    Ok(Json(json!({"task": rec.to_json()})))
}

async fn task_stream(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Sse<impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>>> {
    let rec = state
        .tasks
        .get(&id)
        .ok_or_else(|| ApiError::NotFound(format!("task {id}")))?;
    // replay existing logs, then follow live
    let replay: Vec<String> = rec
        .logs
        .lock()
        .unwrap()
        .iter()
        .map(|l| {
            json!({"task_id": rec.id, "level": l.level, "msg": l.msg, "ts": l.ts}).to_string()
        })
        .collect();
    let rx = rec.log_tx.subscribe();
    let live = {
        use tokio_stream::StreamExt as TsExt;
        TsExt::filter_map(
            tokio_stream::wrappers::BroadcastStream::new(rx),
            |m| m.ok(),
        )
    };
    let stream = futures::stream::iter(replay)
        .chain(live)
        .map(|s| Ok(axum::response::sse::Event::default().data(s)));
    Ok(Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(10))
            .text("ping"),
    ))
}

// ────────────────────────────────────────────────────────────────── triage ──

async fn triage_endpoint(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let case_id = jstr(&body, "case_id").ok_or_else(|| ApiError::Invalid("case_id required".into()))?;
    require_case(&state, &case_id)?;
    let (engine, ranked) = crate::tasks::triage_case(&state, &case_id)
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(json!({
        "engine": engine,
        "ranked": ranked.iter().map(|r| json!({
            "entity_id": r.entity_id, "score": r.score,
            "verdict": r.verdict, "reason": r.reason,
        })).collect::<Vec<_>>(),
    })))
}

// ────────────────────────────────────────────────────────────────── archon ──

async fn archon_status(State(state): State<Arc<AppState>>) -> Json<Value> {
    let cfg = state.connectors();
    let up = crate::connectors::archon::health(&state.http, &cfg).await == "up";
    let sandboxes = if up {
        crate::connectors::archon::list_sandboxes(&state.http, &cfg)
            .await
            .unwrap_or_default()
    } else {
        vec![]
    };
    Json(json!({"up": up, "sandboxes": sandboxes}))
}

async fn archon_ask(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let cfg = state.connectors();
    let question = jstr(&body, "question").unwrap_or_default();
    let sandbox = jstr(&body, "sandbox");
    let top_k = body.get("top_k").and_then(|v| v.as_u64()).unwrap_or(6) as u32;
    if question.trim().is_empty() {
        return Json(json!({"ok": false, "detail": "question required"}));
    }
    let ans = crate::connectors::archon::ask(&state.http, &cfg, &question, sandbox.as_deref(), top_k).await;

    // attach evidence when case context is given
    let mut evidence_id = None;
    if let Some(case_id) = jstr(&body, "case_id") {
        if ans.ok && require_case(&state, &case_id).is_ok() {
            let subject = match jstr(&body, "entity_id").and_then(|id| state.store.get_entity(&id).ok().flatten()) {
                Some(e) => e.id,
                None => {
                    match state.store.add_entity(
                        &case_id,
                        NewEntity::new(EType::Note, format!("Archon: {}", question.chars().take(50).collect::<String>()))
                            .with_confidence(0.9)
                            .with_data(json!({"source": "archon"})),
                    ) {
                        Ok(o) => o.entity.id,
                        Err(_) => String::new(),
                    }
                }
            };
            if let Ok(ev) = state.store.add_evidence(
                NewEvidence::new(&case_id, &subject, "archon_answer")
                    .with_title(format!("archon: {}", question.chars().take(70).collect::<String>()))
                    .with_snippet(ans.answer.chars().take(800).collect::<String>())
                    .with_raw(json!({"citations": ans.citations, "backend": ans.backend, "found": ans.found}))
                    .with_confidence(0.75),
            ) {
                evidence_id = Some(ev.id);
                let _ = state.store.add_timeline(
                    NewEvent::new(&case_id, "grounding", format!("archon: {}", question.chars().take(60).collect::<String>()))
                        .with_entity(subject)
                        .with_source("archon"),
                );
                state.bus.emit("case_updated", &case_id);
            }
        }
    }
    Json(json!({
        "ok": ans.ok,
        "answer": ans.answer,
        "citations": ans.citations,
        "backend": ans.backend,
        "found": ans.found,
        "detail": ans.detail,
        "evidence_id": evidence_id,
    }))
}

async fn archon_push_case(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let Some(case_id) = jstr(&body, "case_id") else {
        return Json(json!({"ok": false, "detail": "case_id required"}));
    };
    let case = match require_case(&state, &case_id) {
        Ok(c) => c,
        Err(e) => return Json(json!({"ok": false, "detail": e.to_string()})),
    };
    let cfg = state.connectors();
    let name = jstr(&body, "sandbox").unwrap_or_else(|| format!("nexus-{}", case.slug));
    match crate::connectors::archon::ensure_sandbox(&state.http, &cfg, &name).await {
        Ok(sandbox) => {
            let g = nexus_core::load_graph(&state.store, &case_id).unwrap_or(Graph {
                nodes: vec![],
                edges: vec![],
            });
            let dossier = nexus_core::dossier(&g, &case, &state.store);
            Json(json!({
                "ok": true,
                "sandbox": sandbox,
                "detail": format!(
                    "sandbox “{name}” ready — dossier.md ({} chars) ready for archon library import",
                    dossier.len()
                ),
            }))
        }
        Err(detail) => Json(json!({"ok": false, "detail": detail})),
    }
}

// ─────────────────────────────────────────────────────── reports / export ──

async fn dossier_md(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let case = require_case(&state, &id)?;
    let g = nexus_core::load_graph(&state.store, &id)?;
    Ok(md_response(nexus_core::dossier(&g, &case, &state.store)))
}

async fn brief_md(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let case = require_case(&state, &id)?;
    let g = nexus_core::load_graph(&state.store, &id)?;
    Ok(md_response(nexus_core::brief(&g, &case, &state.store)))
}

async fn export_case(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let case = require_case(&state, &id)?;
    let nodes = state.store.list_entities(&id)?;
    let edges = state.store.list_relations(&id)?;
    let evidence = state.store.list_case_evidence(&id)?;
    let timeline = state.store.list_timeline(&id)?;
    let docs: BTreeMap<String, String> = state
        .store
        .list_docs_for_case(&id)?
        .into_iter()
        .collect();
    Ok(Json(json!({
        "nexus_export": {"version": 1, "exported_at": nexus_core::now_iso()},
        "case": case,
        "nodes": nodes,
        "edges": edges,
        "evidence": evidence,
        "timeline": timeline,
        "docs": docs,
    })))
}

async fn import_case(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let export = body.get("case").is_some().then_some(&body).ok_or_else(|| {
        ApiError::Invalid("expected a nexus export.json payload with a case field".into())
    })?;
    let case_body = export.get("case").cloned().unwrap_or(json!({}));
    let name = case_body
        .get("name")
        .and_then(|n| n.as_str())
        .map(|s| format!("{s} (imported)"))
        .unwrap_or_else(|| "Imported case".to_string());
    let notes = case_body.get("notes").and_then(|n| n.as_str()).unwrap_or("");
    let tags: Vec<String> = case_body
        .get("tags")
        .and_then(|t| t.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let case = state.store.create_case(&name, notes, &tags)?;

    let mut id_map: HashMap<String, String> = HashMap::new();
    if let Some(nodes) = body.get("nodes").and_then(|n| n.as_array()) {
        for n in nodes {
            let Some(label) = n.get("label").and_then(|l| l.as_str()) else { continue };
            let old_id = n.get("id").and_then(|i| i.as_str()).unwrap_or("");
            let etype = n
                .get("etype")
                .and_then(|e| e.as_str())
                .and_then(EType::from_str)
                .unwrap_or(EType::Note);
            let new_entity = NewEntity::new(etype, label)
                .with_data(n.get("data").cloned().unwrap_or(json!({})))
                .with_confidence(n.get("confidence").and_then(|c| c.as_f64()).unwrap_or(1.0));
            if let Ok(out) = state.store.add_entity(&case.id, new_entity) {
                if !old_id.is_empty() {
                    id_map.insert(old_id.to_string(), out.entity.id.clone());
                }
            }
        }
    }
    if let Some(edges) = body.get("edges").and_then(|e| e.as_array()) {
        for e in edges {
            let (Some(src), Some(dst), Some(rel)) = (
                e.get("src").and_then(|s| s.as_str()).and_then(|s| id_map.get(s)),
                e.get("dst").and_then(|s| s.as_str()).and_then(|s| id_map.get(s)),
                e.get("rel").and_then(|r| r.as_str()),
            ) else {
                continue;
            };
            let _ = state.store.add_relation(
                NewRelation::new(&case.id, src, dst, rel)
                    .with_confidence(e.get("confidence").and_then(|c| c.as_f64()).unwrap_or(0.8))
                    .with_source(e.get("source").and_then(|s| s.as_str()).unwrap_or("import")),
            );
        }
    }
    if let Some(evidence) = body.get("evidence").and_then(|e| e.as_array()) {
        for v in evidence {
            let Some(subject) = v.get("subject").and_then(|s| s.as_str()).and_then(|s| id_map.get(s)) else { continue };
            let _ = state.store.add_evidence(
                NewEvidence::new(&case.id, subject, v.get("kind").and_then(|k| k.as_str()).unwrap_or("manual"))
                    .with_title(v.get("title").and_then(|t| t.as_str()).unwrap_or("imported"))
                    .with_url(v.get("url").and_then(|u| u.as_str()).unwrap_or(""))
                    .with_snippet(v.get("snippet").and_then(|s| s.as_str()).unwrap_or("")),
            );
        }
    }
    state.bus.emit("case_updated", &case.id);
    Ok((
        StatusCode::CREATED,
        Json(json!({"case": case, "mapped": id_map.len()})),
    )
        .into_response())
}

// ──────────────────────────────────────────────────────────── admin/config ──

async fn privacy_md() -> Response {
    // serve the repo's PRIVACY.md (guide links here)
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .map(|p| p.join("PRIVACY.md"));
    if let Some(p) = path {
        if let Ok(md) = std::fs::read_to_string(&p) {
            return md_response(md);
        }
    }
    md_response(
        "# Privacy\n\nPRIVACY.md not found next to the nexus workspace.          See the repository root: qalarc_osint/PRIVACY.md\n".into(),
    )
}

async fn admin_config(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    state.apply_overlay(&body);
    let effective = state.connectors();
    Json(json!({"ok": true, "config": effective.redacted()}))
}

// ────────────────────────────────────────────────────────────────── router ──

pub fn router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/events", get(events))
        .route("/cases", get(list_cases).post(create_case))
        .route("/cases/import", post(import_case))
        .route(
            "/cases/:id",
            get(get_case).patch(patch_case).delete(delete_case),
        )
        .route("/cases/:id/graph", get(case_graph))
        .route("/cases/:id/path", get(case_path))
        .route("/cases/:id/pivots", get(case_pivots))
        .route("/cases/:id/entities", post(add_entity))
        .route("/cases/:id/timeline", get(get_timeline).post(add_timeline))
        .route("/cases/:id/tasks", post(create_task))
        .route("/cases/:id/dossier.md", get(dossier_md))
        .route("/cases/:id/brief.md", get(brief_md))
        .route("/cases/:id/export.json", get(export_case))
        .route(
            "/entities/:id",
            get(get_entity).patch(patch_entity).delete(delete_entity),
        )
        .route("/entities/:id/merge", post(merge_entity))
        .route("/entities/:id/merge/preview", post(merge_preview))
        .route("/entities/:id/evidence", get(entity_evidence))
        .route("/entities/:id/doc", get(get_doc).patch(set_doc))
        .route("/entities/:id/doc/render", post(render_doc))
        .route("/relations", post(add_relation))
        .route("/relations/:id", delete(delete_relation))
        .route("/evidence", post(add_evidence))
        .route("/tasks", get(list_tasks))
        .route("/tasks/:id", get(get_task))
        .route("/tasks/:id/stream", get(task_stream))
        .route("/triage", post(triage_endpoint))
        .route("/laya/triage", post(triage_endpoint))
        .route("/archon/status", get(archon_status))
        .route("/archon/ask", post(archon_ask))
        .route("/archon/push_case", post(archon_push_case))
        .route("/admin/config", post(admin_config))
        .route("/privacy.md", get(privacy_md))
        .with_state(state);

    // optional bearer-token gate (NEXUS_TOKEN set → required on /api except health)
    let with_api = match std::env::var("NEXUS_TOKEN") {
        Ok(tok) if !tok.trim().is_empty() => Router::new()
            .nest("/api", api.layer(middleware::from_fn(move |req: Request, next: Next| {
                let tok = tok.clone();
                async move { check_token(req, next, tok).await }
            }))),
        _ => Router::new().nest("/api", api),
    }
    .layer(CorsLayer::permissive());
    match std::env::var("NEXUS_WEB_DIST")
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            // default: <nexus>/webui/dist relative to the crate
            let d = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .and_then(|p| p.parent())
                .map(|p| p.join("webui/dist"));
            d.filter(|d| d.is_dir())
        }) {
        Some(dist) if dist.is_dir() => {
            let index = dist.join("index.html");
            let svc = tower_http::services::ServeDir::new(&dist)
                .append_index_html_on_directories(true)
                .not_found_service(tower_http::services::ServeFile::new(index));
            with_api.fallback_service(get_service(svc))
        }
        _ => with_api,
    }
}

/// Bearer-token gate: `Authorization: Bearer $NEXUS_TOKEN`. `/api/health`
/// stays open so the UI can show server status before a token is entered.
async fn check_token(req: Request, next: Next, token: String) -> Response {
    // nest() strips the /api prefix before middleware runs — accept both forms
    let path = req.uri().path();
    if path == "/api/health" || path == "/health" {
        return next.run(req).await;
    }
    let ok = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|given| given == token)
        .unwrap_or(false);
    if ok {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "missing or invalid bearer token (set it in Settings)"})),
        )
            .into_response()
    }
}
