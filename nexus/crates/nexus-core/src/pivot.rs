//! Pivot suggestions — CONTRACT §3 (`pivot.rs`).
//!
//! Suggests connector actions per entity. Only actions whose connector is
//! present in `connectors` are produced. Priorities are the contract values
//! (username+hub 0.9 … archon 0.6) with two boosts:
//! - **+0.10** for the best suggestion when the entity has no evidence yet.
//!   NOTE: `Graph` carries nodes/edges only, so "zero evidence" is proxied by
//!   *zero relations in the graph* (an isolated node).
//! - **+0.15** when the entity is pinned.
//!
//! Output sorted by priority desc (ties by action name), capped at 8.

use crate::graph::Graph;
use crate::model::EType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PivotSuggestion {
    pub entity_id: String,
    /// e.g. `hub_scan:username`, `flowsint_enrich:subdomain_discovery`
    pub action: String,
    pub reason: String,
    pub priority: f64,
}

/// Max suggestions returned.
pub const MAX_PIVOTS: usize = 8;

/// Builds pivot suggestions for `entity_id` given available connectors
/// (subset of `["hub", "flowsint", "openplanter", "archon"]`).
pub fn suggest_pivots(g: &Graph, entity_id: &str, connectors: &[&str]) -> Vec<PivotSuggestion> {
    let Some(entity) = g.node(entity_id) else {
        return Vec::new();
    };
    let has = |c: &str| connectors.contains(&c);
    let mut out: Vec<PivotSuggestion> = Vec::new();
    let mut push = |action: &str, priority: f64, reason: String| {
        out.push(PivotSuggestion {
            entity_id: entity.id.clone(),
            action: action.to_string(),
            reason,
            priority,
        });
    };

    // hub scans by etype
    match entity.etype {
        EType::Username if has("hub") => push(
            "hub_scan:username",
            0.9,
            "username entity — hub multi-scan reveals account registrations across linked sites".into(),
        ),
        EType::Email if has("hub") => push(
            "hub_scan:email",
            0.85,
            "email entity — hub scan finds registration + breach footprint".into(),
        ),
        EType::Phone if has("hub") => push(
            "hub_scan:phone",
            0.8,
            "phone entity — hub scan resolves messaging/social registrations".into(),
        ),
        EType::Image if has("hub") => push(
            "hub_scan:image",
            0.75,
            "image entity — hub reverse-image search finds pages hosting this image".into(),
        ),
        _ => {}
    }

    // flowsint enrichers
    match entity.etype {
        EType::Domain if has("flowsint") => push(
            "flowsint_enrich:subdomain_discovery",
            0.8,
            "domain entity — enumerate subdomains and exposed hosts".into(),
        ),
        EType::Website if has("flowsint") => push(
            "flowsint_enrich:website_crawler",
            0.7,
            "website entity — crawl for contacts, trackers and linked assets".into(),
        ),
        EType::Ip if has("flowsint") => push(
            "flowsint_enrich:ip_information",
            0.7,
            "ip entity — enrich hosting/ASN/geolocation intelligence".into(),
        ),
        _ => {}
    }

    // openplanter deep research
    if matches!(entity.etype, EType::Person | EType::Topic) && has("openplanter") {
        push(
            "openplanter_research",
            0.85,
            format!(
                "{} entity — deep agentic research produces cited artifacts",
                entity.etype
            ),
        );
    }

    // archon canon grounding — any entity
    if has("archon") {
        push(
            "archon_ground",
            0.6,
            format!("check internal canon/RAG for prior grounding on «{}»", entity.label),
        );
    }

    // boost: entity with no evidence (proxied by no relations in the graph)
    let isolated = g.edges.iter().all(|r| r.src != entity.id && r.dst != entity.id);
    if isolated && !out.is_empty() {
        let mut best = 0;
        for i in 1..out.len() {
            if out[i].priority > out[best].priority {
                best = i;
            }
        }
        out[best].priority = (out[best].priority + 0.10).min(1.0);
        out[best].reason.push_str("; +0.1 boost (no evidence recorded yet)");
    }

    if entity.pinned {
        for s in &mut out {
            s.priority = (s.priority + 0.15).min(1.0);
        }
    }

    out.sort_by(|a, b| b.priority.total_cmp(&a.priority).then(a.action.cmp(&b.action)));
    out.truncate(MAX_PIVOTS);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Entity, NewEntity};
    use crate::store::Store;

    #[test]
    fn respects_connector_availability_and_priority() {
        let store = Store::open_memory().unwrap();
        let case = store.create_case("t", "", &[]).unwrap();
        let u = store
            .add_entity(&case.id, NewEntity::new(EType::Username, "johndoe"))
            .unwrap()
            .entity;
        let d = store
            .add_entity(&case.id, NewEntity::new(EType::Domain, "evilcorp.io"))
            .unwrap()
            .entity;
        let g = Graph { nodes: vec![u.clone(), d.clone()], edges: vec![] };

        // hub + flowsint available
        let pivots = suggest_pivots(&g, &u.id, &["hub", "flowsint", "archon"]);
        assert_eq!(pivots[0].action, "hub_scan:username");
        assert!((pivots[0].priority - 1.0).abs() < 1e-9, "0.9 + 0.1 zero-evidence boost, got {}", pivots[0].priority);
        assert_eq!(pivots[1].action, "archon_ground");
        assert!((pivots[1].priority - 0.6).abs() < 1e-9, "archon stays at base 0.6 (boost goes to best only), got {}", pivots[1].priority);
        // username gets no flowsint action
        assert!(pivots.iter().all(|p| !p.action.starts_with("flowsint")));

        // without hub, no hub action for the username
        let pivots = suggest_pivots(&g, &u.id, &["flowsint"]);
        assert!(pivots.is_empty());

        // domain + flowsint → subdomain_discovery at 0.8 (+0.1 isolated → 0.9)
        let pivots = suggest_pivots(&g, &d.id, &["hub", "flowsint"]);
        assert_eq!(pivots[0].action, "flowsint_enrich:subdomain_discovery");
        assert!((pivots[0].priority - 0.9).abs() < 1e-9);

        // pinned adds +0.15
        let pinned = store
            .add_entity(&case.id, NewEntity::new(EType::Email, "x@y.io").with_confidence(0.5))
            .unwrap()
            .entity;
        let pinned = store
            .update_entity(&pinned.id, crate::model::EntityPatch { pinned: Some(true), ..Default::default() })
            .unwrap();
        let g2 = Graph { nodes: vec![pinned.clone()], edges: vec![] };
        let pivots = suggest_pivots(&g2, &pinned.id, &["hub"]);
        assert_eq!(pivots.len(), 1);
        assert!((pivots[0].priority - 1.0).abs() < 1e-9, "0.85 + 0.1 + 0.15 clamped to 1.0, got {}", pivots[0].priority);

        // relations present → no isolation boost; connected email stays 0.85
        let r = crate::model::Relation {
            id: "r_1".into(),
            case_id: case.id.clone(),
            src: pinned.id.clone(),
            dst: u.id.clone(),
            rel: "uses".into(),
            weight: 1.0,
            confidence: 1.0,
            source: None,
            evidence: None,
            created_at: String::new(),
        };
        let g3 = Graph { nodes: vec![pinned.clone(), u.clone()], edges: vec![r] };
        // pinned+connected: 0.85 + 0.15 pinned, NO isolation boost
        let pivots = suggest_pivots(&g3, &pinned.id, &["hub"]);
        assert!((pivots[0].priority - 1.0).abs() < 1e-9, "pinned connected → 0.85+0.15, got {}", pivots[0].priority);
        // unpinned but connected username: exactly 0.9 (no isolation boost)
        let pivots = suggest_pivots(&g3, &u.id, &["hub"]);
        assert!((pivots[0].priority - 0.9).abs() < 1e-9, "connected → no boost, got {}", pivots[0].priority);

        // unknown entity → no suggestions
        assert!(suggest_pivots(&g3, "e_ghost", &["hub"]).is_empty());
    }

    #[test]
    fn cap_and_person_openplanter() {
        let g = Graph {
            nodes: vec![Entity {
                id: "e_p".into(),
                case_id: "c_1".into(),
                etype: EType::Person,
                label: "alice".into(),
                data: serde_json::json!({}),
                confidence: 1.0,
                pinned: false,
                first_seen: String::new(),
                last_seen: String::new(),
            }],
            edges: vec![],
        };
        let pivots = suggest_pivots(&g, "e_p", &["openplanter", "archon"]);
        assert_eq!(pivots.len(), 2);
        assert_eq!(pivots[0].action, "openplanter_research");
        assert!((pivots[0].priority - 0.95).abs() < 1e-9, "0.85 + 0.1 boost, got {}", pivots[0].priority);
        assert!(pivots.len() <= MAX_PIVOTS);
    }
}
