//! # nexus-core
//!
//! Entity-graph investigation engine for NEXUS (CONTRACT §2/§3):
//! SQLite-backed case store with normalization-based dedup, graph
//! algorithms (path/pagerank/communities), pivot suggestions and
//! markdown dossier/brief builders.
//!
//! Authored-use OSINT only.

pub mod graph;
pub mod model;
pub mod pivot;
pub mod report;
pub mod resolve;
pub mod store;

pub use graph::{
    communities, components, degree_centrality, load_graph, neighbors, pagerank, shortest_path,
    Graph,
};
pub use model::{
    AddOutcome, Case, Entity, EntityPatch, EType, Evidence, NewEntity, NewEvent, NewEvidence,
    NewRelation, Relation, TimelineEvent, CANONICAL_RELS,
};
pub use pivot::{suggest_pivots, PivotSuggestion};
pub use report::{brief, dossier, rel_sentence};
pub use resolve::{extract_candidates, normalize, similarity};
pub use store::Store;

use chrono::SecondsFormat;

/// Crate result alias.
pub type Result<T> = std::result::Result<T, NexusError>;

/// Crate error type.
#[derive(Debug, thiserror::Error)]
pub enum NexusError {
    /// SQLite failure.
    #[error("store error: {0}")]
    Store(#[from] rusqlite::Error),
    /// Filesystem failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON (de)serialization failure.
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    /// A referenced object does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// Semantically invalid input (cross-case refs, self-relations, …).
    #[error("invalid: {0}")]
    Invalid(String),
}

/// Current UTC time as an RFC3339 seconds-precision string
/// (`2026-09-22T12:34:56Z`).
pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Generates a `<prefix>_<16 hex>` id per the contract's id convention.
pub fn new_id(prefix: &str) -> String {
    format!(
        "{}_{}",
        prefix,
        &uuid::Uuid::new_v4().simple().to_string()[..16]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_id_shape() {
        let id = new_id("e");
        assert!(id.starts_with("e_"));
        let hex = &id[2..];
        assert_eq!(hex.len(), 16);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(new_id("e"), new_id("e"));
    }

    #[test]
    fn now_iso_shape() {
        let ts = now_iso();
        assert!(ts.ends_with('Z'), "{ts}");
        assert_eq!(ts.len(), 20, "{ts}");
    }

    #[test]
    fn error_display() {
        let e = NexusError::NotFound("e_x".into());
        assert_eq!(e.to_string(), "not found: e_x");
        let e = NexusError::Invalid("bad".into());
        assert_eq!(e.to_string(), "invalid: bad");
    }
}
