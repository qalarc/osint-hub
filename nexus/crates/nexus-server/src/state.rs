//! AppState + router assembly + serve.

use crate::bus::Bus;
use crate::config::{Config, ConnectorCfg};
use crate::tasks::TaskEngine;
use nexus_core::Store;
use std::sync::{Arc, RwLock};

pub struct AppState {
    pub store: Store,
    pub cfg: Config,
    pub overlay: Arc<RwLock<ConnectorCfg>>,
    pub bus: Bus,
    pub http: reqwest::Client,
    pub tasks: Arc<TaskEngine>,
}

impl AppState {
    /// Effective connector config (env base + persisted overlay).
    pub fn connectors(&self) -> ConnectorCfg {
        self.overlay.read().unwrap().clone()
    }

    /// Apply + persist a partial runtime overlay (admin/config).
    pub fn apply_overlay(&self, overlay: &serde_json::Value) {
        {
            let mut guard = self.overlay.write().unwrap();
            apply_overlay_fields(&mut guard, overlay);
        }
        self.cfg.save_overlay(overlay);
    }
}

fn apply_overlay_fields(cfg: &mut ConnectorCfg, overlay: &serde_json::Value) {
    let Some(map) = overlay.as_object() else { return };
    let s = |k: &str| map.get(k).and_then(|v| v.as_str()).map(String::from);
    let o = |k: &str| match map.get(k) {
        Some(serde_json::Value::Null) => Some(None),
        Some(v) => v.as_str().map(|x| Some(x.to_string())),
        None => None,
    };
    if let Some(v) = s("hub_url") {
        cfg.hub_url = v;
    }
    if let Some(v) = o("hub_token") {
        cfg.hub_token = v;
    }
    if let Some(v) = o("flowsint_url") {
        cfg.flowsint_url = v;
    }
    if let Some(v) = o("openplanter_bin") {
        cfg.openplanter_bin = v;
    }
    if let Some(v) = s("archon_url") {
        cfg.archon_url = v;
    }
    if let Some(v) = o("laya_url") {
        cfg.laya_url = v;
    }
    if let Some(v) = s("jev_url") {
        cfg.jev_url = v;
    }
    if let Some(v) = o("jev_api_key") {
        cfg.jev_api_key = v;
    }
}

fn ensure_parent_dir(p: &std::path::Path) {
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
}

/// Build the full application state (store, bus, http client, task workers).
pub async fn init_state(cfg: Config) -> Arc<AppState> {
    ensure_parent_dir(&cfg.db_path);
    ensure_parent_dir(&cfg.connectors.workspaces);
    let store = if cfg.db_path == std::path::Path::new(":memory:") {
        Store::open_memory()
    } else {
        Store::open(&cfg.db_path)
    }
    .expect("nexus: failed to open store");
    let overlay = Arc::new(RwLock::new(cfg.connectors.clone()));
    let bus = Bus::new();
    let http = reqwest::Client::builder()
        .user_agent("nexus/0.1 (qalarc investigation workbench)")
        .build()
        .expect("nexus: http client");
    let (engine, rx) = TaskEngine::new();
    let state = Arc::new(AppState {
        store,
        cfg,
        overlay,
        bus,
        http,
        tasks: engine,
    });

    // task dispatcher: up to 2 connector tasks run concurrently
    let sem = Arc::new(tokio::sync::Semaphore::new(2));
    let st = state.clone();
    tokio::spawn(async move {
        let mut rx = rx;
        while let Some(rec) = rx.recv().await {
            let permit = sem.clone().acquire_owned().await.expect("semaphore closed");
            let st2 = st.clone();
            tokio::spawn(async move {
                crate::tasks::run_task_public(st2, rec).await;
                drop(permit);
            });
        }
    });
    state
}

pub fn build_router(state: Arc<AppState>) -> axum::Router {
    crate::routes::router(state)
}

pub async fn serve(cfg: Config) -> anyhow::Result<()> {
    let port = cfg.port;
    let state = init_state(cfg).await;
    let router = build_router(state);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    tracing::info!("nexus-server listening on http://127.0.0.1:{port}");
    axum::serve(listener, router).await?;
    Ok(())
}
