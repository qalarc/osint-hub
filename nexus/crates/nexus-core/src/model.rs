//! Data model — CONTRACT §2.
//!
//! All types are `serde` snake_case, `Clone` + `Debug`. Timestamps are
//! RFC3339 UTC strings; ids are `<prefix>_<16 hex>`.

use serde::{Deserialize, Serialize};

/// The 22 fixed entity types (CONTRACT §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EType {
    Person,
    Org,
    Username,
    Alias,
    Email,
    Phone,
    Domain,
    Subdomain,
    Ip,
    Asn,
    Cidr,
    Website,
    SocialProfile,
    Wallet,
    Transaction,
    Image,
    Document,
    Article,
    Event,
    Location,
    Topic,
    Note,
}

/// Canonical relation types (CONTRACT §2). `rel` stays a free-form string;
/// these are the well-known ones (used for sentence templates in reports).
pub const CANONICAL_RELS: &[&str] = &[
    "uses",
    "owns",
    "resolves_to",
    "subdomain_of",
    "registered_to",
    "mentions",
    "contacted",
    "same_as",
    "paid_to",
    "located_in",
    "member_of",
    "works_at",
    "sourced_from",
    "derived_from",
    "related_to",
    "posted_on",
];

impl EType {
    /// All 22 variants, fixed order.
    pub const ALL: [EType; 22] = [
        Self::Person,
        Self::Org,
        Self::Username,
        Self::Alias,
        Self::Email,
        Self::Phone,
        Self::Domain,
        Self::Subdomain,
        Self::Ip,
        Self::Asn,
        Self::Cidr,
        Self::Website,
        Self::SocialProfile,
        Self::Wallet,
        Self::Transaction,
        Self::Image,
        Self::Document,
        Self::Article,
        Self::Event,
        Self::Location,
        Self::Topic,
        Self::Note,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Org => "org",
            Self::Username => "username",
            Self::Alias => "alias",
            Self::Email => "email",
            Self::Phone => "phone",
            Self::Domain => "domain",
            Self::Subdomain => "subdomain",
            Self::Ip => "ip",
            Self::Asn => "asn",
            Self::Cidr => "cidr",
            Self::Website => "website",
            Self::SocialProfile => "social_profile",
            Self::Wallet => "wallet",
            Self::Transaction => "transaction",
            Self::Image => "image",
            Self::Document => "document",
            Self::Article => "article",
            Self::Event => "event",
            Self::Location => "location",
            Self::Topic => "topic",
            Self::Note => "note",
        }
    }

    /// Parses an etype string; see also the [`std::str::FromStr`] impl
    /// (which maps failures to [`crate::NexusError::Invalid`]).
    #[allow(clippy::should_implement_trait)] // inherent Option-returning form per CONTRACT §3
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "person" => Self::Person,
            "org" => Self::Org,
            "username" => Self::Username,
            "alias" => Self::Alias,
            "email" => Self::Email,
            "phone" => Self::Phone,
            "domain" => Self::Domain,
            "subdomain" => Self::Subdomain,
            "ip" => Self::Ip,
            "asn" => Self::Asn,
            "cidr" => Self::Cidr,
            "website" => Self::Website,
            "social_profile" => Self::SocialProfile,
            "wallet" => Self::Wallet,
            "transaction" => Self::Transaction,
            "image" => Self::Image,
            "document" => Self::Document,
            "article" => Self::Article,
            "event" => Self::Event,
            "location" => Self::Location,
            "topic" => Self::Topic,
            "note" => Self::Note,
            _ => return None,
        })
    }
}

impl std::fmt::Display for EType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for EType {
    type Err = crate::NexusError;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Self::from_str(s).ok_or_else(|| crate::NexusError::Invalid(format!("unknown etype: {s}")))
    }
}

/// Investigation case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Case {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub status: String,
    pub notes: String,
    pub tags: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A node in the case graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    pub id: String,
    pub case_id: String,
    pub etype: EType,
    pub label: String,
    /// Free JSON (e.g. `{"platform":"github","url":"..."}`).
    pub data: serde_json::Value,
    pub confidence: f64,
    pub pinned: bool,
    pub first_seen: String,
    pub last_seen: String,
}

/// A typed edge in the case graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Relation {
    pub id: String,
    pub case_id: String,
    pub src: String,
    pub dst: String,
    pub rel: String,
    pub weight: f64,
    pub confidence: f64,
    pub source: Option<String>,
    /// Optional inline evidence blob `{title,url,...}` (CONTRACT §2).
    pub evidence: Option<serde_json::Value>,
    pub created_at: String,
}

/// Provenance record attached to an entity or relation id (`subject`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub id: String,
    pub case_id: String,
    pub subject: String,
    pub kind: String,
    pub title: Option<String>,
    pub url: Option<String>,
    pub snippet: Option<String>,
    pub raw: serde_json::Value,
    pub confidence: f64,
    pub ts: String,
}

/// Case timeline event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineEvent {
    pub id: String,
    pub case_id: String,
    pub ts: String,
    pub entity_id: Option<String>,
    pub kind: String,
    pub title: String,
    pub detail: serde_json::Value,
    pub source: Option<String>,
}

fn d_one() -> f64 {
    1.0
}

fn d_obj() -> serde_json::Value {
    serde_json::json!({})
}

/// Input for [`crate::Store::add_entity`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewEntity {
    pub etype: EType,
    pub label: String,
    #[serde(default = "d_obj")]
    pub data: serde_json::Value,
    #[serde(default = "d_one")]
    pub confidence: f64,
    #[serde(default)]
    pub pinned: bool,
}

impl NewEntity {
    pub fn new(etype: EType, label: impl Into<String>) -> Self {
        Self {
            etype,
            label: label.into(),
            data: d_obj(),
            confidence: 1.0,
            pinned: false,
        }
    }

    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = data;
        self
    }

    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.confidence = confidence;
        self
    }
}

/// Input for [`crate::Store::add_relation`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewRelation {
    pub case_id: String,
    pub src: String,
    pub dst: String,
    pub rel: String,
    #[serde(default = "d_one")]
    pub weight: f64,
    #[serde(default = "d_one")]
    pub confidence: f64,
    pub source: Option<String>,
    pub evidence: Option<serde_json::Value>,
}

impl NewRelation {
    pub fn new(case_id: impl Into<String>, src: impl Into<String>, dst: impl Into<String>, rel: impl Into<String>) -> Self {
        Self {
            case_id: case_id.into(),
            src: src.into(),
            dst: dst.into(),
            rel: rel.into(),
            weight: 1.0,
            confidence: 1.0,
            source: None,
            evidence: None,
        }
    }

    pub fn with_weight(mut self, weight: f64) -> Self {
        self.weight = weight;
        self
    }

    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.confidence = confidence;
        self
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    pub fn with_evidence(mut self, evidence: serde_json::Value) -> Self {
        self.evidence = Some(evidence);
        self
    }
}

/// Input for [`crate::Store::add_evidence`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewEvidence {
    pub case_id: String,
    pub subject: String,
    pub kind: String,
    pub title: Option<String>,
    pub url: Option<String>,
    pub snippet: Option<String>,
    #[serde(default = "d_obj")]
    pub raw: serde_json::Value,
    #[serde(default = "d_one")]
    pub confidence: f64,
}

impl NewEvidence {
    pub fn new(case_id: impl Into<String>, subject: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            case_id: case_id.into(),
            subject: subject.into(),
            kind: kind.into(),
            title: None,
            url: None,
            snippet: None,
            raw: d_obj(),
            confidence: 1.0,
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }

    pub fn with_snippet(mut self, snippet: impl Into<String>) -> Self {
        self.snippet = Some(snippet.into());
        self
    }

    pub fn with_raw(mut self, raw: serde_json::Value) -> Self {
        self.raw = raw;
        self
    }

    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.confidence = confidence;
        self
    }
}

/// Input for [`crate::Store::add_timeline`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewEvent {
    pub case_id: String,
    /// `None` → stamped with `now` at insert time.
    pub ts: Option<String>,
    pub entity_id: Option<String>,
    pub kind: String,
    pub title: String,
    #[serde(default = "d_obj")]
    pub detail: serde_json::Value,
    pub source: Option<String>,
}

impl NewEvent {
    pub fn new(case_id: impl Into<String>, kind: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            case_id: case_id.into(),
            ts: None,
            entity_id: None,
            kind: kind.into(),
            title: title.into(),
            detail: d_obj(),
            source: None,
        }
    }

    pub fn with_ts(mut self, ts: impl Into<String>) -> Self {
        self.ts = Some(ts.into());
        self
    }

    pub fn with_entity(mut self, entity_id: impl Into<String>) -> Self {
        self.entity_id = Some(entity_id.into());
        self
    }

    pub fn with_detail(mut self, detail: serde_json::Value) -> Self {
        self.detail = detail;
        self
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
}

/// Sparse patch for [`crate::Store::update_entity`].
/// `data` replaces the whole `data` object when present.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EntityPatch {
    pub label: Option<String>,
    pub data: Option<serde_json::Value>,
    pub pinned: Option<bool>,
    pub confidence: Option<f64>,
}

/// Result of [`crate::Store::add_entity`] — dedup-aware for the HTTP layer
/// (`POST /api/cases/{id}/entities` → `{entity, deduplicated}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AddOutcome {
    pub entity: Entity,
    pub deduplicated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etype_has_22_fixed_variants_and_round_trips() {
        assert_eq!(EType::ALL.len(), 22);
        for e in EType::ALL {
            assert_eq!(EType::from_str(e.as_str()), Some(e), "round-trip {}", e);
            let json = serde_json::to_string(&e).unwrap();
            assert_eq!(json, format!("\"{}\"", e.as_str()));
            assert_eq!(serde_json::to_string(&e).unwrap(), json);
            let back: EType = serde_json::from_str(&json).unwrap();
            assert_eq!(back, e);
        }
        assert!(EType::from_str("bogus").is_none());
        assert!("bogus".parse::<EType>().is_err());
    }

    #[test]
    fn canonical_rels_count() {
        assert_eq!(CANONICAL_RELS.len(), 16);
        assert!(CANONICAL_RELS.contains(&"resolves_to"));
        assert!(CANONICAL_RELS.contains(&"posted_on"));
    }

    #[test]
    fn new_types_have_contract_defaults() {
        let ne: NewEntity = serde_json::from_str(r#"{"etype":"username","label":"johndoe"}"#).unwrap();
        assert_eq!(ne.etype, EType::Username);
        assert_eq!(ne.confidence, 1.0);
        assert!(!ne.pinned);
        assert_eq!(ne.data, serde_json::json!({}));

        let nr: NewRelation = serde_json::from_str(
            r#"{"case_id":"c_1","src":"e_1","dst":"e_2","rel":"uses","source":"hub:sherlock"}"#,
        )
        .unwrap();
        assert_eq!(nr.weight, 1.0);
        assert_eq!(nr.confidence, 1.0);
        assert_eq!(nr.source.as_deref(), Some("hub:sherlock"));

        let nv: NewEvidence = serde_json::from_str(
            r#"{"case_id":"c_1","subject":"e_1","kind":"tool_result"}"#,
        )
        .unwrap();
        assert_eq!(nv.confidence, 1.0);
        assert_eq!(nv.raw, serde_json::json!({}));
    }

    #[test]
    fn entity_json_matches_contract_shape() {
        let json = r#"{"id":"e_ab12","case_id":"c_1","etype":"username","label":"johndoe","data":{},"confidence":1.0,"pinned":false,"first_seen":"2026-09-22T00:00:00Z","last_seen":"2026-09-22T00:00:00Z"}"#;
        let e: Entity = serde_json::from_str(json).unwrap();
        assert_eq!(e.etype, EType::Username);
        assert_eq!(e.id, "e_ab12");
    }
}
