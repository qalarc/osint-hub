//! OpenPlanter connector — recursive investigation agent (subprocess bridge).
//! Runs the upstream CLI headlessly in a per-task workspace, then ingests
//! artifacts as evidence/documents and regex-sweeps candidate entities.

use crate::connectors::IngestCounts;
use crate::config::ConnectorCfg;
use nexus_core::{EType, NewEntity, NewEvidence, NewRelation, Store};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;

pub fn resolve_bin(cfg: &ConnectorCfg) -> Option<PathBuf> {
    if let Some(b) = &cfg.openplanter_bin {
        let p = PathBuf::from(b);
        if p.is_file() || !b.contains('/') {
            return Some(p); // absolute path or bare command for PATH resolution
        }
        return None;
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("openplanter-agent"))
        .find(|p| p.is_file())
}

pub async fn health(_http: &reqwest::Client, cfg: &ConnectorCfg) -> &'static str {
    match resolve_bin(cfg) {
        Some(_) => "up",
        None => "down",
    }
}

pub fn slugify(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join("-")
}

#[allow(clippy::too_many_arguments)]
pub async fn research(
    cfg: &ConnectorCfg,
    store: &Store,
    case_id: &str,
    case_slug: &str,
    task_id: &str,
    target: Option<&nexus_core::Entity>,
    question: &str,
    use_ollama: bool,
    mut log: impl FnMut(&str),
) -> Result<(String, IngestCounts), String> {
    let Some(bin) = resolve_bin(cfg) else {
        return Err(
            "openplanter-agent not found (set OPENPLANTER_BIN or install: \
             git clone https://github.com/ShinMegamiBoson/OpenPlanter && \
             pip install -e OpenPlanter/agent)"
                .into(),
        );
    };

    let ws = cfg.workspaces.join(slugify(case_slug)).join(task_id);
    std::fs::create_dir_all(&ws).map_err(|e| format!("workspace mkdir failed: {e}"))?;
    log(&format!("workspace {}", ws.display()));

    let start = std::time::SystemTime::now();
    let mut cmd = tokio::process::Command::new(&bin);
    cmd.args(["--task", question, "--workspace"])
        .arg(&ws)
        .args(["--headless", "--no-tui"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .current_dir(&ws);
    if use_ollama {
        cmd.arg("--provider").arg("ollama");
        // provider alone isn't enough — their default model is an Anthropic one
        // qalarc default: abliterated 4.7 flash (user preference) under a native
        // ollama alias — hf.co/ names make their resolver route to the wrong provider
        let model = std::env::var("OPENPLANTER_OLLAMA_MODEL").unwrap_or_else(|_| {
            "glm-4.7-flash-abl:base".to_string()
        });
        cmd.args(["--model", &model]);
        // thinking-capable models stall OpenPlanter's parser; ollama 400s on
        // thinking params for non-thinking models — none is the safe default
        cmd.args(["--reasoning-effort", "none"]);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("openplanter spawn failed: {e}"))?;

    // shared log buffers, streamed into task logs
    let buffer: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let readers: Vec<Option<tokio::process::ChildStdout>> = vec![]; // placeholder, replaced below
    drop(readers);

    async fn pump<S>(stream: Option<S>, buffer: Arc<Mutex<Vec<String>>>, tag: &'static str)
    where
        S: tokio::io::AsyncRead + Unpin,
    {
        let Some(stream) = stream else { return };
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let mut buf = buffer.lock().await;
            buf.push(format!("[{tag}] {line}"));
        }
    }

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let b1 = buffer.clone();
    let b2 = buffer.clone();
    let out_task = tokio::spawn(pump(stdout, b1, "op"));
    let err_task = tokio::spawn(pump(stderr, b2, "op!"));

    // 20-minute hard cap
    let waited = tokio::time::timeout(
        std::time::Duration::from_secs(20 * 60),
        child.wait(),
    )
    .await;
    let status = match waited {
        Ok(Ok(st)) => Some(st),
        Ok(Err(e)) => return Err(format!("openplanter wait failed: {e}")),
        Err(_) => {
            log("openplanter timed out (20min) — killing");
            let _ = child.kill().await;
            None
        }
    };
    let _ = out_task.await;
    let _ = err_task.await;
    for line in buffer.lock().await.iter().rev().take(12).rev() {
        log(line);
    }
    match status {
        Some(st) if st.success() => {}
        Some(st) => log(&format!("openplanter exited with {st}")),
        None => {}
    }

    // post-run ingestion
    let mut counts = IngestCounts::default();
    let mut hub_entity: Option<String> = None;
    let files = collect_artifacts(&ws, start);
    let artifacts = files.len();
    log(&format!("ingesting {artifacts} artifacts"));

    for path in files {
        let Ok(content) = std::fs::read_to_string(&path) else { continue };
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("artifact")
            .to_string();
        let is_md = path.extension().and_then(|e| e.to_str()) == Some("md");

        if hub_entity.is_none() {
            let etype = if is_md { EType::Document } else { EType::Note };
            match store.add_entity(
                case_id,
                NewEntity::new(etype, format!("Research: {}", truncate(question, 60)))
                    .with_confidence(0.85)
                    .with_data(json!({"source": "openplanter", "first_file": name})),
            ) {
                Ok(o) => {
                    if !o.deduplicated {
                        counts.entities += 1;
                    }
                    hub_entity = Some(o.entity.id.clone());
                }
                Err(_) => continue,
            }
        }
        let subject = hub_entity.clone().unwrap();

        if store
            .add_evidence(
                NewEvidence::new(case_id, &subject, "openplanter_finding")
                    .with_title(&name)
                    .with_snippet(truncate(&content, 500))
                    .with_raw(json!({"path": path.display().to_string()}))
                    .with_confidence(0.8),
            )
            .is_ok()
        {
            counts.evidence += 1;
        }

        for (etype, cand) in nexus_core::extract_candidates(&content).into_iter().take(100) {
            let Ok(o) = store.add_entity(
                case_id,
                NewEntity::new(etype, &cand).with_confidence(0.6),
            ) else {
                continue;
            };
            if !o.deduplicated {
                counts.entities += 1;
            }
            if store
                .add_relation(
                    NewRelation::new(case_id, &o.entity.id, &subject, "derived_from")
                        .with_confidence(0.6)
                        .with_source("openplanter"),
                )
                .is_ok()
            {
                counts.relations += 1;
            }
        }
    }

    let _ = target; // target used only for rel context in future work
    Ok((
        format!(
            "{artifacts} artifacts, {} entities, {} relations ingested",
            counts.entities, counts.relations
        ),
        counts,
    ))
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Files under `ws` (recursive, skip dot-dirs) modified after `start`,
/// text-ish extensions, <2MB, capped at 200.
fn collect_artifacts(ws: &Path, start: std::time::SystemTime) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![ws.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let p = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if p.is_dir() {
                if !name.starts_with('.') {
                    stack.push(p);
                }
                continue;
            }
            let ext_ok = matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("md") | Some("txt") | Some("json") | Some("csv")
            );
            if !ext_ok || name.starts_with('.') {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if meta.len() > 2_000_000 {
                continue;
            }
            if let Ok(modified) = meta.modified() {
                if modified >= start {
                    out.push(p);
                }
            }
            if out.len() >= 200 {
                return out;
            }
        }
    }
    out
}
