//! Connector implementations + shared ingestion helpers.

pub mod archon;
pub mod flowsint;
pub mod hub;
pub mod jev;
pub mod laya;
pub mod openplanter;
pub mod websearch;

/// Running totals for a task's ingestion work.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct IngestCounts {
    pub entities: usize,
    pub relations: usize,
    pub evidence: usize,
}

impl IngestCounts {
    pub fn merge(&mut self, other: &IngestCounts) {
        self.entities += other.entities;
        self.relations += other.relations;
        self.evidence += other.evidence;
    }
}

/// Connector health for /api/health: up | down | unset.
pub type HealthMap = std::collections::BTreeMap<String, String>;
