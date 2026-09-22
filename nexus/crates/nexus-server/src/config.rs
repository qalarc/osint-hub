//! Configuration: env defaults + runtime overlay (admin/config), persisted
//! to `NEXUS_CONFIG` (0600). Secrets are never logged and redacted in
//! responses.

use std::path::{Path, PathBuf};

/// Connector settings — the subset that `POST /api/admin/config` may
/// override at runtime.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConnectorCfg {
    pub hub_url: String,
    #[serde(default)]
    pub hub_token: Option<String>,
    pub flowsint_url: Option<String>,
    pub openplanter_bin: Option<String>,
    pub workspaces: PathBuf,
    pub archon_url: String,
    pub laya_url: Option<String>,
    pub jev_url: String,
    #[serde(default)]
    pub jev_api_key: Option<String>,
    #[serde(default)]
    pub laya_log: Option<PathBuf>,
}

impl ConnectorCfg {
    fn from_env() -> Self {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let data_dir = manifest
            .parent() // crates/
            .and_then(|p| p.parent()) // nexus/
            .map(|p| p.join("data"))
            .unwrap_or_else(|| PathBuf::from("data"));
        Self {
            hub_url: env_or("NEXUS_HUB_URL", "http://127.0.0.1:8799"),
            hub_token: env_opt("OSINT_HUB_TOKEN").or_else(|| env_opt("NEXUS_HUB_TOKEN")),
            flowsint_url: env_opt("FLOWSINT_URL"),
            openplanter_bin: env_opt("OPENPLANTER_BIN")
                .map(PathBuf::from)
                .or_else(|| which("openplanter-agent"))
                .map(|p| p.to_string_lossy().into_owned()),
            workspaces: env_path("OPENPLANTER_WORKSPACES")
                .unwrap_or_else(|| data_dir.join("workspaces")),
            archon_url: env_or("ARCHON_URL", "http://127.0.0.1:7843"),
            laya_url: env_opt("LAYA_URL"),
            jev_url: env_or("JEV_URL", "https://api.typesafe.ai/v1/systemone"),
            jev_api_key: env_opt("JEV_API_KEY")
                .or_else(|| env_opt("TYPESAFE_API_KEY"))
                .or_else(read_typesafe_env_key),
            laya_log: env_path("NEXUS_LAYA_LOG")
                .or_else(|| Some(data_dir.join("laya-decisions.jsonl"))),
        }
    }

    /// Redacted view for /api/admin/config responses: secrets → bool presence.
    pub fn redacted(&self) -> serde_json::Value {
        serde_json::json!({
            "hub_url": self.hub_url,
            "hub_token": self.hub_token.is_some(),
            "flowsint_url": self.flowsint_url,
            "openplanter_bin": self.openplanter_bin,
            "workspaces": self.workspaces,
            "archon_url": self.archon_url,
            "laya_url": self.laya_url,
            "jev_url": self.jev_url,
            "jev_api_key": self.jev_api_key.is_some(),
            "laya_log": self.laya_log,
        })
    }
}

/// Fields the admin overlay may set (CONTRACT §4).
const OVERLAY_FIELDS: &[&str] = &[
    "hub_url",
    "hub_token",
    "flowsint_url",
    "openplanter_bin",
    "archon_url",
    "laya_url",
    "jev_url",
    "jev_api_key",
];

#[derive(Debug, Clone)]
pub struct Config {
    pub db_path: PathBuf,
    pub web_dist: Option<PathBuf>,
    pub port: u16,
    pub config_path: Option<PathBuf>,
    pub connectors: ConnectorCfg,
}

impl Config {
    pub fn from_env() -> Self {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let nexus_root = manifest
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let data_dir = nexus_root.join("data");
        let db_path = env_path("NEXUS_DB").unwrap_or_else(|| data_dir.join("nexus.db"));
        let port = env_opt("NEXUS_PORT")
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(8801);
        let web_dist = env_path("NEXUS_WEB_DIST").or_else(|| {
            let d = nexus_root.join("webui/dist");
            d.is_dir().then_some(d)
        });
        let config_path = env_path("NEXUS_CONFIG").unwrap_or_else(|| data_dir.join("config.json"));

        let mut connectors = ConnectorCfg::from_env();
        // apply persisted overlay at boot
        if let Ok(text) = std::fs::read_to_string(&config_path) {
            if let Ok(overlay) = serde_json::from_str::<serde_json::Value>(&text) {
                apply_overlay(&mut connectors, &overlay);
            }
        }
        Self {
            db_path,
            web_dist,
            port,
            config_path: Some(config_path),
            connectors,
        }
    }

    /// Persist the given partial overlay (already applied to `self.connectors`
    /// by the caller) to disk with 0600. Never stores db_path/web_dist.
    pub fn save_overlay(&self, overlay: &serde_json::Value) {
        let path = &self.config_path;
        let Some(path) = path else { return };
        let mut merged = serde_json::Map::new();
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(serde_json::Value::Object(prev)) =
                serde_json::from_str::<serde_json::Value>(&text)
            {
                merged = prev;
            }
        }
        if let serde_json::Value::Object(map) = overlay {
            for (k, v) in map {
                if OVERLAY_FIELDS.contains(&k.as_str()) {
                    merged.insert(k.clone(), v.clone());
                }
            }
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(&serde_json::Value::Object(merged)) {
            let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
            let _ = std::fs::write(path, bytes);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }
}

fn apply_overlay(cfg: &mut ConnectorCfg, overlay: &serde_json::Value) {
    let Some(map) = overlay.as_object() else { return };
    if let Some(v) = str_field(map, "hub_url") {
        cfg.hub_url = v;
    }
    if let Some(v) = opt_str_field(map, "hub_token") {
        cfg.hub_token = v;
    }
    if let Some(v) = opt_str_field(map, "flowsint_url") {
        cfg.flowsint_url = v;
    }
    if let Some(v) = opt_str_field(map, "openplanter_bin") {
        cfg.openplanter_bin = v;
    }
    if let Some(v) = str_field(map, "archon_url") {
        cfg.archon_url = v;
    }
    if let Some(v) = opt_str_field(map, "laya_url") {
        cfg.laya_url = v;
    }
    if let Some(v) = str_field(map, "jev_url") {
        cfg.jev_url = v;
    }
    if let Some(v) = opt_str_field(map, "jev_api_key") {
        cfg.jev_api_key = v;
    }
}

fn str_field(map: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    map.get(key).and_then(|v| v.as_str()).map(String::from)
}

fn opt_str_field(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<Option<String>> {
    match map.get(key) {
        None => None,
        Some(serde_json::Value::Null) => Some(None),
        Some(v) => v.as_str().map(|s| Some(s.to_string())),
    }
}

fn env_opt(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.trim().is_empty())
}

fn env_or(name: &str, default: &str) -> String {
    env_opt(name).unwrap_or_else(|| default.to_string())
}

fn env_path(name: &str) -> Option<PathBuf> {
    env_opt(name).map(PathBuf::from)
}

/// Minimal PATH lookup (no external crate).
fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(bin))
        .find(|p| p.is_file())
}

/// Parse TYPESAFE_API_KEY from ~/.secrets/typesafe.env (values never leave
/// this function; only the key handle is stored).
fn read_typesafe_env_key() -> Option<String> {
    let f = home_dir()?.join(".secrets/typesafe.env");
    let text = std::fs::read_to_string(f).ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix("TYPESAFE_API_KEY="))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}
