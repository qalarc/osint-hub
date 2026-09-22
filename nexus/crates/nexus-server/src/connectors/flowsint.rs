//! flowsint connector — full bridge to a running flowsint instance
//! (VERIFIED against live openapi.json, 2026-09-22).
//!
//! Flow: service-account auth (register once + OAuth2 password token) →
//! per-case investigation + sketch → node sync for the target entity →
//! `POST /api/enrichers/{name}/launch {sketch_id, node_ids}` → wait for the
//! sketch graph to stabilise → ingest nodes/edges into the nexus case.
//!
//! State (creds, token, case/node mappings) lives in
//! `<nexus>/data/flowsint-bridge.json`. Everything degrades gracefully.

use crate::connectors::IngestCounts;
use crate::config::ConnectorCfg;
use nexus_core::{EType, NewEntity, NewRelation, Store};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

// ───────────────────────────────────────────────────── bridge state file ──

fn state_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("data").join("flowsint-bridge.json"))
        .unwrap_or_else(|| PathBuf::from("flowsint-bridge.json"))
}

#[derive(Default)]
struct BridgeState {
    email: Option<String>,
    password: Option<String>,
    token: Option<String>,
    token_ts: u64,
    cases: HashMap<String, CaseMapping>,
}

#[derive(Default, Clone)]
struct CaseMapping {
    investigation_id: String,
    sketch_id: String,
    nodes: HashMap<String, String>, // normalized label key -> flowsint node id
}

impl BridgeState {
    #[allow(clippy::field_reassign_with_default)]
    fn load() -> Self {
        let Ok(text) = std::fs::read_to_string(state_path()) else {
            return Self::default();
        };
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let mut s = Self::default();
        s.email = v.get("email").and_then(|x| x.as_str()).map(String::from);
        s.password = v.get("password").and_then(|x| x.as_str()).map(String::from);
        s.token = v.get("token").and_then(|x| x.as_str()).map(String::from);
        s.token_ts = v.get("token_ts").and_then(|x| x.as_u64()).unwrap_or(0);
        if let Some(cases) = v.get("cases").and_then(|c| c.as_object()) {
            for (cid, cm) in cases {
                let mut m = CaseMapping {
                    investigation_id: cm
                        .get("investigation_id")
                        .and_then(|x| x.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    sketch_id: cm
                        .get("sketch_id")
                        .and_then(|x| x.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    nodes: HashMap::new(),
                };
                if let Some(nodes) = cm.get("nodes").and_then(|n| n.as_object()) {
                    for (k, nid) in nodes {
                        m.nodes.insert(k.clone(), nid.as_str().unwrap_or_default().to_string());
                    }
                }
                s.cases.insert(cid.clone(), m);
            }
        }
        s
    }

    fn save(&self) {
        let cases: serde_json::Map<String, Value> = self
            .cases
            .iter()
            .map(|(cid, m)| {
                let nodes: serde_json::Map<String, Value> =
                    m.nodes.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
                (
                    cid.clone(),
                    json!({"investigation_id": m.investigation_id, "sketch_id": m.sketch_id,
                           "nodes": nodes}),
                )
            })
            .collect();
        let v = json!({
            "email": self.email, "password": self.password,
            "token": self.token, "token_ts": self.token_ts,
            "cases": cases,
        });
        let p = state_path();
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(p, serde_json::to_vec_pretty(&v).unwrap_or_default());
    }
}

// ─────────────────────────────────────────────────────────────── helpers ──

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Typed node properties — flowsint validates every node into its
/// flowsint-types pydantic models (verified from their repo + live 500s),
/// so each type needs its canonical primary field.
fn typed_properties(etype: &str, label: &str) -> Value {
    match etype {
        "domain" | "subdomain" | "website" => json!({"domain": label}),
        "ip" => json!({"address": label}),
        "email" => json!({"email": label}),
        "phone" => json!({"number": label}),
        "username" | "alias" => json!({"value": label, "platform": "nexus"}),
        "wallet" => json!({"address": label}),
        "asn" => json!({"asn_str": label}),
        "cidr" => json!({"cidr": label}),
        "person" => json!({"full_name": label}),
        "org" => json!({"name": label}),
        _ => json!({"value": label}),
    }
}

/// nexus etype -> flowsint NodeType (their capitalized taxonomy).
fn etype_to_flowsint(etype: &str) -> &'static str {
    match etype {
        "domain" | "subdomain" => "Domain",
        "ip" => "IP",
        "asn" => "ASN",
        "cidr" => "CIDR",
        "person" => "Individual",
        "org" => "Organization",
        "email" => "Email",
        "phone" => "Phone",
        "website" => "Website",
        "social_profile" => "SocialProfile",
        "username" | "alias" => "Username",
        "wallet" => "Wallet",
        _ => "Note",
    }
}

/// flowsint NodeType -> nexus etype.
fn flowsint_to_etype(t: &str) -> EType {
    let t = t.to_lowercase();
    if t.contains("subdomain") {
        EType::Subdomain
    } else if t.contains("domain") {
        EType::Domain
    } else if t.contains("cidr") {
        EType::Cidr
    } else if t.contains("asn") {
        EType::Asn
    } else if t == "ip" || t.contains("ipaddress") {
        EType::Ip
    } else if t.contains("individual") || t.contains("person") {
        EType::Person
    } else if t.contains("org") {
        EType::Org
    } else if t.contains("email") {
        EType::Email
    } else if t.contains("phone") {
        EType::Phone
    } else if t.contains("website") {
        EType::Website
    } else if t.contains("social") || t.contains("profile") {
        EType::SocialProfile
    } else if t.contains("wallet") {
        EType::Wallet
    } else if t.contains("username") {
        EType::Username
    } else {
        EType::Note
    }
}

/// Default enrichers per etype — flowsint slugs (validated live at launch;
/// mismatches surface the available list in the task log).
pub fn default_enrichers(etype: &str) -> Vec<String> {
    match etype {
        "domain" | "subdomain" => vec![
            "domain_to_subdomains",
            "domain_to_dns",
            "domain_to_whois",
            "domain_to_asn",
        ],
        "ip" => vec!["ip_to_asn", "ip_to_domain", "ip_to_intelligence", "ip_to_ports"],
        "asn" => vec!["asn_to_cidrs"],
        "cidr" => vec!["cidr_to_ips"],
        "website" => vec!["website_to_crawler", "website_to_links", "website_to_webtrackers", "website_to_text"],
        "email" => vec!["email_to_gravatar", "email_to_breaches", "email_to_domains"],
        "phone" => vec!["phone_to_carrier", "phone_to_device_hudsonrock"],
        "username" | "alias" => vec!["username_to_socials_maigret", "username_to_socials_sherlock"],
        "org" => vec!["org_to_infos", "org_to_domains", "org_to_asn"],
        "person" => vec!["individual_to_organization", "individual_to_domains"],
        "wallet" => vec!["cryptowallet_to_transactions", "cryptowallet_to_nfts"],
        _ => vec![],
    }
    .into_iter()
    .map(String::from)
    .collect()
}

pub async fn health(http: &reqwest::Client, cfg: &ConnectorCfg) -> &'static str {
    match &cfg.flowsint_url {
        None => "unset",
        Some(base) => {
            let ok = http
                .get(format!("{}/health", base.trim_end_matches('/')))
                .timeout(std::time::Duration::from_secs(4))
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false);
            if ok { "up" } else { "down" }
        }
    }
}

// ────────────────────────────────────────────────────────────────── auth ──

struct Bridge<'a> {
    http: &'a reqwest::Client,
    base: String,
    state: BridgeState,
}

impl<'a> Bridge<'a> {
    fn new(http: &'a reqwest::Client, cfg: &ConnectorCfg) -> Result<Self, String> {
        let Some(base) = cfg.flowsint_url.as_deref() else {
            return Err("flowsint not configured".into());
        };
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            state: BridgeState::load(),
        })
    }

    async fn api_opt(
        &self,
        token: &str,
        path: &str,
    ) -> Result<Option<Value>, String> {
        let resp = self
            .http
            .get(format!("{}{path}", self.base))
            .timeout(std::time::Duration::from_secs(60))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| format!("{path}: {e}"))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None); // flowsint 404s empty collections
        }
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("{path} -> HTTP {status}: {}", truncate(&text, 160)));
        }
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| format!("{path}: bad JSON ({e})"))
    }

    async fn api(
        &self,
        token: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, String> {
        let url = format!("{}{path}", self.base);
        let mut req = self
            .http
            .request(
                reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::POST),
                &url,
            )
            .timeout(std::time::Duration::from_secs(60))
            .bearer_auth(token);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await.map_err(|e| format!("{path}: {e}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("{path} -> HTTP {status}: {}", truncate(&text, 160)));
        }
        serde_json::from_str(&text).map_err(|e| format!("{path}: bad JSON ({e})"))
    }

    /// Valid token (bootstrap register + OAuth2 password grant; re-auth ~50min).
    async fn token(&mut self) -> Result<String, String> {
        let fresh = self
            .state
            .token
            .as_deref()
            .map(|t| !t.is_empty() && now_secs().saturating_sub(self.state.token_ts) < 50 * 60)
            .unwrap_or(false);
        if fresh {
            return Ok(self.state.token.clone().unwrap_or_default());
        }
        if self.state.email.is_none() {
            let email = format!(
                "nexus-bridge-{}@example.com",
                &nexus_core::new_id("x")[2..10]
            );
            let password = format!(
                "Nx{}!{}",
                &nexus_core::new_id("p")[2..8],
                &nexus_core::new_id("s")[2..8]
            );
            let resp = self
                .http
                .post(format!("{}/api/auth/register", self.base))
                .timeout(std::time::Duration::from_secs(20))
                .json(&json!({"email": email, "password": password}))
                .send()
                .await
                .map_err(|e| format!("register: {e}"))?;
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            // 201 = created. 4xx mentioning "already" = creds are ours, fine.
            // Anything else is a real failure — surface the detail.
            let already = status.is_client_error() && body.to_lowercase().contains("already");
            if !status.is_success() && !already {
                return Err(format!("register -> HTTP {status}: {}", truncate(&body, 200)));
            }
            self.state.email = Some(email);
            self.state.password = Some(password);
            self.state.save();
        }
        let form = [
            ("grant_type", "password"),
            ("username", self.state.email.as_deref().unwrap_or("")),
            ("password", self.state.password.as_deref().unwrap_or("")),
        ];
        let resp = self
            .http
            .post(format!("{}/api/auth/token", self.base))
            .timeout(std::time::Duration::from_secs(20))
            .form(&form)
            .send()
            .await
            .map_err(|e| format!("token: {e}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("token -> HTTP {status}: {}", truncate(&text, 160)));
        }
        let v: Value = serde_json::from_str(&text).map_err(|e| format!("token JSON: {e}"))?;
        let tok = v
            .get("access_token")
            .and_then(|t| t.as_str())
            .ok_or("token response missing access_token")?
            .to_string();
        self.state.token = Some(tok.clone());
        self.state.token_ts = now_secs();
        self.state.save();
        Ok(tok)
    }

    /// investigation + sketch ids for a nexus case (created on first use).
    async fn ensure_context(
        &mut self,
        token: &str,
        case_id: &str,
        case_name: &str,
    ) -> Result<CaseMapping, String> {
        if let Some(m) = self.state.cases.get(case_id) {
            if !m.sketch_id.is_empty() {
                return Ok(m.clone());
            }
        }
        let inv_name = format!("nexus — {case_name}");
        let list = self
            .api_opt(token, "/api/investigations")
            .await?
            .unwrap_or(Value::Null);
        let mut inv_id = None;
        if let Some(items) = list.as_array() {
            for it in items {
                if it.get("name").and_then(|n| n.as_str()) == Some(inv_name.as_str()) {
                    inv_id = it.get("id").and_then(|i| i.as_str()).map(String::from);
                }
            }
        }
        let inv_id = match inv_id {
            Some(id) => id,
            None => {
                let created = self
                    .api(
                        token,
                        "POST",
                        "/api/investigations/create",
                        Some(json!({
                            "name": inv_name,
                            "description": format!("Bridged from NEXUS case {case_id}")
                        })),
                    )
                    .await?;
                created
                    .get("id")
                    .and_then(|i| i.as_str())
                    .ok_or("investigation create returned no id")?
                    .to_string()
            }
        };
        let sketches = self
            .api_opt(token, &format!("/api/investigations/{inv_id}/sketches"))
            .await?
            .unwrap_or(Value::Null);
        let mut sketch_id = None;
        if let Some(items) = sketches.as_array() {
            for it in items {
                if it.get("title").and_then(|n| n.as_str()).is_some() {
                    sketch_id = it.get("id").and_then(|i| i.as_str()).map(String::from);
                }
            }
        }
        let sketch_id = match sketch_id {
            Some(id) => id,
            None => {
                let created = self
                    .api(
                        token,
                        "POST",
                        "/api/sketches/create",
                        Some(json!({
                            "title": "nexus-bridge",
                            "description": "auto-created by NEXUS",
                            "investigation_id": inv_id
                        })),
                    )
                    .await?;
                created
                    .get("id")
                    .and_then(|i| i.as_str())
                    .ok_or("sketch create returned no id")?
                    .to_string()
            }
        };
        let m = CaseMapping {
            investigation_id: inv_id,
            sketch_id,
            nodes: HashMap::new(),
        };
        self.state.cases.insert(case_id.to_string(), m.clone());
        self.state.save();
        Ok(m)
    }

    fn persist(&mut self, case_id: &str, m: &CaseMapping) {
        self.state.cases.insert(case_id.to_string(), m.clone());
        self.state.save();
    }

    /// Fetch the sketch graph (their keys: `nds` / `rls`, legacy: nodes/edges).
    async fn graph(&self, token: &str, sketch_id: &str) -> Result<Value, String> {
        let g = self
            .api(token, "GET", &format!("/api/sketches/{sketch_id}/graph"), None)
            .await?;
        let nodes = g
            .get("nds")
            .or_else(|| g.get("nodes"))
            .cloned()
            .unwrap_or_else(|| json!([]));
        let edges = g
            .get("rls")
            .or_else(|| g.get("edges"))
            .or_else(|| g.get("links"))
            .cloned()
            .unwrap_or_else(|| json!([]));
        Ok(json!({"nodes": nodes, "edges": edges}))
    }

    /// flowsint node id for an entity label (search graph, else create).
    async fn ensure_node(
        &mut self,
        token: &str,
        case_id: &str,
        m: &mut CaseMapping,
        etype: &str,
        label: &str,
    ) -> Result<String, String> {
        let key = format!("{}|{}", etype_to_flowsint(etype), label.to_lowercase());
        if let Some(id) = m.nodes.get(&key) {
            if !id.is_empty() {
                return Ok(id.clone());
            }
        }
        // search existing graph by label
        let graph = self.graph(token, &m.sketch_id).await?;
        if let Some(nodes) = graph.get("nodes").and_then(|n| n.as_array()) {
            for n in nodes {
                let nlbl = n
                    .get("nodeLabel")
                    .or_else(|| n.get("label"))
                    .and_then(|l| l.as_str())
                    .unwrap_or_default();
                if nlbl.eq_ignore_ascii_case(label) {
                    if let Some(id) = n.get("id").and_then(|i| i.as_str()) {
                        m.nodes.insert(key, id.to_string());
                        self.persist(case_id, m);
                        return Ok(id.to_string());
                    }
                }
            }
        }
        let temp_id = nexus_core::new_id("fn");
        self.api(
            token,
            "POST",
            &format!("/api/sketches/{}/nodes/add", m.sketch_id),
            Some(json!({
                "id": temp_id,
                "nodeLabel": label,
                "nodeType": etype_to_flowsint(etype),
                "nodeMetadata": {"created_at": nexus_core::now_iso()},
                "nodeProperties": typed_properties(etype, label),
            })),
        )
        .await?;
        // flowsint assigns its own neo4j elementId — resolve by label
        let graph = self.graph(token, &m.sketch_id).await?;
        let real_id = graph
            .get("nodes")
            .and_then(|n| n.as_array())
            .and_then(|arr| {
                arr.iter()
                    .find(|n| {
                        n.get("nodeLabel")
                            .or_else(|| n.get("label"))
                            .and_then(|l| l.as_str())
                            .map(|l| l.eq_ignore_ascii_case(label))
                            .unwrap_or(false)
                    })
                    .and_then(|n| n.get("id").and_then(|i| i.as_str()))
            })
            .map(String::from)
            .unwrap_or(temp_id);
        m.nodes.insert(key, real_id.clone());
        self.persist(case_id, m);
        Ok(real_id)
    }
}

// ───────────────────────────────────────────────────────── run + ingest ──

#[allow(clippy::too_many_arguments)]
pub async fn run_enricher(
    http: &reqwest::Client,
    cfg: &ConnectorCfg,
    store: &Store,
    case_id: &str,
    case_name: &str,
    target: Option<&nexus_core::Entity>,
    etype: &str,
    value: &str,
    enricher: &str,
    mut log: impl FnMut(&str),
) -> Result<(String, IngestCounts), String> {
    let mut bridge = Bridge::new(http, cfg)?;
    let token = bridge.token().await?;
    log("flowsint: authenticated");
    let mut m = bridge.ensure_context(&token, case_id, case_name).await?;
    log(&format!("flowsint: sketch {}", m.sketch_id));

    let node_id = bridge.ensure_node(&token, case_id, &mut m, etype, value).await?;
    log(&format!("flowsint: node {node_id} for “{value}”"));

    bridge
        .api(
            &token,
            "POST",
            &format!("/api/enrichers/{enricher}/launch"),
            Some(json!({"sketch_id": m.sketch_id, "node_ids": [node_id]})),
        )
        .await
        .map_err(|e| {
            format!("{e} — check the enricher slug; GET /api/enrichers lists the available set")
        })?;
    log(&format!("flowsint: launched {enricher}"));

    // wait for the graph to stabilise (10min cap, 3 stable 5s polls)
    let mut last_count = 0usize;
    let mut stable = 0u32;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10 * 60);
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let graph = bridge.graph(&token, &m.sketch_id).await?;
        let count = graph
            .get("nodes")
            .and_then(|n| n.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        if count == last_count && count > 0 {
            stable += 1;
            if stable >= 3 {
                break;
            }
        } else {
            stable = 0;
        }
        last_count = count;
    }
    log(&format!("flowsint: graph settled at {last_count} nodes"));

    // ingest
    let graph = bridge.graph(&token, &m.sketch_id).await?;
    let mut counts = IngestCounts::default();
    let mut id_map: HashMap<String, String> = HashMap::new();
    if let Some(nodes) = graph.get("nodes").and_then(|n| n.as_array()) {
        for n in nodes {
            let Some(fid) = n.get("id").and_then(|i| i.as_str()) else { continue };
            let label = n
                .get("nodeLabel")
                .or_else(|| n.get("label"))
                .and_then(|l| l.as_str())
                .unwrap_or("unnamed");
            let ftype = n
                .get("nodeType")
                .or_else(|| n.get("type"))
                .and_then(|t| t.as_str())
                .unwrap_or("Note");
            let Ok(outcome) = store.add_entity(
                case_id,
                NewEntity::new(flowsint_to_etype(ftype), label)
                    .with_confidence(0.8)
                    .with_data(json!({"flowsint_type": ftype})),
            ) else {
                continue;
            };
            if !outcome.deduplicated {
                counts.entities += 1;
            }
            id_map.insert(fid.to_string(), outcome.entity.id.clone());
            if let Some(t) = target.filter(|t| outcome.entity.id != t.id) {
                if store
                    .add_relation(
                        NewRelation::new(case_id, &t.id, &outcome.entity.id, "related_to")
                            .with_confidence(0.8)
                            .with_source(format!("flowsint:{enricher}")),
                    )
                    .is_ok()
                {
                    counts.relations += 1;
                }
            }
        }
    }
    if let Some(edges) = graph
        .get("edges")
        .or_else(|| graph.get("links"))
        .and_then(|e| e.as_array())
    {
        for e in edges {
            let src = ["source", "src", "from"]
                .iter()
                .find_map(|k| e.get(k).and_then(|s| s.as_str()))
                .unwrap_or("");
            let dst = ["target", "dst", "to"]
                .iter()
                .find_map(|k| e.get(k).and_then(|s| s.as_str()))
                .unwrap_or("");
            let rel = e
                .get("label")
                .or_else(|| e.get("rel"))
                .or_else(|| e.get("type"))
                .and_then(|r| r.as_str())
                .unwrap_or("related_to");
            if let (Some(a), Some(b)) = (id_map.get(src), id_map.get(dst)) {
                if store
                    .add_relation(
                        NewRelation::new(case_id, a, b, rel)
                            .with_confidence(0.75)
                            .with_source(format!("flowsint:{enricher}")),
                    )
                    .is_ok()
                {
                    counts.relations += 1;
                }
            }
        }
    }
    Ok((
        format!(
            "flowsint {enricher}: {} new entities, {} relations (sketch at {last_count} nodes)",
            counts.entities, counts.relations
        ),
        counts,
    ))
}
