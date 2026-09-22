//! Markdown dossier + journalism brief — CONTRACT §3 (`report.rs`).
//!
//! Both builders are infallible (`-> String`) per the contract; store reads
//! inside are best-effort (a failing read yields an empty section, never a
//! panic).

use crate::graph::{communities, pagerank, Graph};
use crate::model::{Case, Evidence, Relation};
use crate::store::Store;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Confidence floor for the brief's claim ledger.
pub const CLAIM_MIN_CONFIDENCE: f64 = 0.6;
/// Below this, a claim is always `Unverified` regardless of evidence.
pub const UNVERIFIED_BELOW: f64 = 0.5;
/// "Key entities" table size.
pub const KEY_ENTITIES: usize = 20;
/// "Key connections" list size.
pub const KEY_CONNECTIONS: usize = 20;
/// Executive summary size.
pub const TOP_ENTITIES: usize = 5;

/// Full markdown dossier: executive summary, key entities/connections,
/// communities, timeline, evidence appendix, sources.
pub fn dossier(g: &Graph, case: &Case, store: &Store) -> String {
    let pr = pagerank(g, 0.85, 100);
    let evidence = store.list_case_evidence(&case.id).unwrap_or_default();
    let timeline = store.list_timeline(&case.id).unwrap_or_default();

    // subject → evidence count (entity subjects only for the table)
    let mut ev_by_entity: HashMap<&str, usize> = HashMap::new();
    let mut ev_by_subject: HashMap<&str, usize> = HashMap::new();
    for ev in &evidence {
        *ev_by_subject.entry(ev.subject.as_str()).or_default() += 1;
        *ev_by_entity.entry(ev.subject.as_str()).or_default() += 1;
    }

    let mut ranked: Vec<(&crate::model::Entity, f64)> = g
        .nodes
        .iter()
        .map(|n| (n, *pr.get(&n.id).unwrap_or(&0.0)))
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.id.cmp(&b.0.id)));

    let mut md = String::new();
    md.push_str(&format!("# {}\n\n", case.name));
    md.push_str(&format!(
        "_Generated {} — {} entities, {} relations, {} evidence, {} timeline events._\n\n",
        crate::now_iso(),
        g.nodes.len(),
        g.edges.len(),
        evidence.len(),
        timeline.len()
    ));

    // executive summary
    md.push_str("## Executive summary\n\n");
    if ranked.is_empty() {
        md.push_str("_No entities in this case yet._\n");
    } else {
        for (i, (e, score)) in ranked.iter().take(TOP_ENTITIES).enumerate() {
            md.push_str(&format!(
                "{}. **«{}»** ({}) — pagerank {:.4}{}\n",
                i + 1,
                e.label,
                e.etype,
                score,
                ev_note(ev_by_entity.get(e.id.as_str()).copied())
            ));
        }
    }
    md.push('\n');

    // key entities table
    md.push_str("## Key entities\n\n");
    if ranked.is_empty() {
        md.push_str("_none._\n\n");
    } else {
        md.push_str("| etype | label | confidence | pinned | evidence |\n");
        md.push_str("|---|---|---|---|---|\n");
        for (e, _) in ranked.iter().take(KEY_ENTITIES) {
            md.push_str(&format!(
                "| {} | {} | {:.2} | {} | {} |\n",
                e.etype,
                md_cell(&e.label),
                e.confidence,
                if e.pinned { "yes" } else { "no" },
                ev_by_entity.get(e.id.as_str()).copied().unwrap_or(0)
            ));
        }
        md.push('\n');
    }

    // key connections
    md.push_str("## Key connections\n\n");
    let mut edges: Vec<&Relation> = g.edges.iter().collect();
    edges.sort_by(|a, b| {
        (b.weight * b.confidence)
            .total_cmp(&(a.weight * a.confidence))
            .then(a.id.cmp(&b.id))
    });
    if edges.is_empty() {
        md.push_str("_none._\n\n");
    } else {
        for r in edges.iter().take(KEY_CONNECTIONS) {
            let a = g.label(&r.src);
            let b = g.label(&r.dst);
            let mut line = format!(
                "- {} — conf {:.2}, weight {:.2}",
                rel_sentence(&r.rel, a, b),
                r.confidence,
                r.weight
            );
            if let Some(src) = &r.source {
                line.push_str(&format!(", source {src}"));
            }
            md.push_str(&line);
            md.push('\n');
        }
        md.push('\n');
    }

    // communities
    md.push_str("## Communities\n\n");
    let com = communities(g);
    if com.is_empty() {
        md.push_str("_none._\n\n");
    } else {
        let mut grouped: BTreeMap<usize, Vec<String>> = BTreeMap::new();
        for (id, c) in &com {
            grouped.entry(*c).or_default().push(id.clone());
        }
        for (c, ids) in grouped {
            let mut labels: Vec<String> = ids.iter().map(|id| g.label(id).to_string()).collect();
            labels.sort();
            md.push_str(&format!("- **Community {}**: {}\n", c + 1, labels.join(", ")));
        }
        md.push('\n');
    }

    // timeline (asc — list_timeline is already asc)
    md.push_str("## Timeline\n\n");
    if timeline.is_empty() {
        md.push_str("_none._\n\n");
    } else {
        for e in &timeline {
            let who = e
                .entity_id
                .as_deref()
                .map(|id| format!(" ({})", g.label(id)))
                .unwrap_or_default();
            md.push_str(&format!("- {} — {}: {}{}\n", e.ts, e.kind, e.title, who));
        }
        md.push('\n');
    }

    // evidence appendix
    md.push_str("## Evidence appendix\n\n");
    if evidence.is_empty() {
        md.push_str("_none._\n\n");
    } else {
        for ev in &evidence {
            md.push_str(&format!("- `{}` **{}**", ev.id, ev.kind));
            if let Some(title) = &ev.title {
                md.push_str(&format!(" — {title}"));
            }
            if let Some(url) = &ev.url {
                md.push_str(&format!(" — {url}"));
            }
            if let Some(snippet) = &ev.snippet {
                md.push_str(&format!(" — “{snippet}”"));
            }
            md.push_str(&format!(" — subject: {}\n", subject_label(g, &ev.subject)));
        }
        md.push('\n');
    }

    // sources
    md.push_str("## Sources\n\n");
    let sources = dedup_sources(&evidence);
    if sources.is_empty() {
        md.push_str("_none._\n");
    } else {
        for (title, url) in sources {
            md.push_str(&format!("- [{title}]({url})\n"));
        }
    }
    md
}

/// Journalism brief: lede angles, nut graf, claim ledger, open questions,
/// deduped source list.
pub fn brief(g: &Graph, case: &Case, store: &Store) -> String {
    let pr = pagerank(g, 0.85, 100);
    let evidence = store.list_case_evidence(&case.id).unwrap_or_default();
    let mut ev_by_subject: HashMap<&str, usize> = HashMap::new();
    for ev in &evidence {
        *ev_by_subject.entry(ev.subject.as_str()).or_default() += 1;
    }

    let mut ranked: Vec<(&crate::model::Entity, f64)> = g
        .nodes
        .iter()
        .map(|n| (n, *pr.get(&n.id).unwrap_or(&0.0)))
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.id.cmp(&b.0.id)));

    let com = communities(g);
    let n_communities = com.values().collect::<HashSet<_>>().len();

    let mut md = String::new();
    md.push_str(&format!("# Brief — {}\n\n", case.name));

    // working lede angles
    md.push_str("## Working lede angles\n\n");
    if ranked.is_empty() {
        md.push_str("_No entities yet — nothing to write about._\n");
    } else {
        let top: Vec<&(&crate::model::Entity, f64)> = ranked.iter().take(3).collect();
        for (i, (e, _)) in top.iter().enumerate() {
            let ev_count = ev_by_subject.get(e.id.as_str()).copied().unwrap_or(0);
            let angle = match i {
                0 => format!(
                    "the anchor: what do the {ev_count} evidence item(s) on «{}» actually establish — and what is still inference?",
                    e.label
                ),
                1 => format!(
                    "the network: «{}» sits at the centre of the graph — who connects to it, and what do those ties buy them?",
                    e.label
                ),
                _ => format!(
                    "the trail: follow «{}» ({}) forward — what is the most recent verifiable activity on record?",
                    e.label, e.etype
                ),
            };
            md.push_str(&format!("{}. **«{}» ({})** — {}\n", i + 1, e.label, e.etype, angle));
        }
    }
    md.push('\n');

    // nut graf
    md.push_str("## Nut graf\n\n");
    let sourced = evidence.iter().filter(|e| e.url.is_some()).count();
    md.push_str(&format!(
        "The case “{}” currently spans {} entities and {} relations across {} communities, \
         resting on {} evidence items ({} with live sources). The strongest signals centre on {}. \
         Treat every unverified claim in the ledger below as reporting to be done, not fact.\n\n",
        case.name,
        g.nodes.len(),
        g.edges.len(),
        n_communities,
        evidence.len(),
        sourced,
        ranked
            .iter()
            .take(2)
            .map(|(e, _)| format!("«{}»", e.label))
            .collect::<Vec<_>>()
            .join(" and ")
    ));

    // claim ledger
    md.push_str("## Claim ledger\n\n");
    let mut claims: Vec<(&Relation, usize)> = g
        .edges
        .iter()
        .filter(|r| r.confidence >= CLAIM_MIN_CONFIDENCE)
        .map(|r| {
            // backing evidence: attached to the relation itself or to the source
            let n = ev_by_subject.get(r.id.as_str()).copied().unwrap_or(0)
                + ev_by_subject.get(r.src.as_str()).copied().unwrap_or(0);
            (r, n)
        })
        .collect();
    claims.sort_by(|a, b| {
        (b.0.weight * b.0.confidence)
            .total_cmp(&(a.0.weight * a.0.confidence))
            .then(a.0.id.cmp(&b.0.id))
    });
    if claims.is_empty() {
        md.push_str(&format!(
            "_No relations at ≥{CLAIM_MIN_CONFIDENCE:.2} confidence yet._\n\n"
        ));
    } else {
        md.push_str("| Claim | Status | Confidence | Evidence items |\n");
        md.push_str("|---|---|---|---|\n");
        for (r, n) in claims {
            let sentence = rel_sentence(&r.rel, g.label(&r.src), g.label(&r.dst));
            md.push_str(&format!(
                "| {} | {} | {:.2} | {} |\n",
                md_cell(&sentence),
                claim_status(r.confidence, n),
                r.confidence,
                n
            ));
        }
        md.push('\n');
    }

    // open questions harvested from wiki docs
    md.push_str("## Open questions\n\n");
    let mut questions: Vec<String> = Vec::new();
    for (entity_id, markdown) in store.list_docs_for_case(&case.id).unwrap_or_default() {
        if let Some(section) = crate::store::extract_section(&markdown, "Open questions") {
            let label = g.label(&entity_id).to_string();
            for line in section.lines() {
                let line = line.trim();
                if !line.is_empty() && !questions.contains(&line.to_string()) {
                    questions.push(format!("- **{label}**: {}", line.trim_start_matches("- ")));
                }
            }
        }
    }
    if questions.is_empty() {
        md.push_str("_none recorded._\n\n");
    } else {
        md.push_str(&questions.join("\n"));
        md.push_str("\n\n");
    }

    // source list
    md.push_str("## Source list\n\n");
    let sources = dedup_sources(&evidence);
    if sources.is_empty() {
        md.push_str("_none._\n");
    } else {
        for (title, url) in sources {
            md.push_str(&format!("- [{title}]({url})\n"));
        }
    }
    md
}

/// Canonical relation → sentence template; unknown rels fall back to
/// `«A» —rel→ «B»`.
pub fn rel_sentence(rel: &str, a: &str, b: &str) -> String {
    let (x, y) = (format!("«{a}»"), format!("«{b}»"));
    match rel {
        "uses" => format!("{x} uses account {y}"),
        "owns" => format!("{x} owns {y}"),
        "resolves_to" => format!("{x} resolves to {y}"),
        "subdomain_of" => format!("{x} is a subdomain of {y}"),
        "registered_to" => format!("{x} is registered to {y}"),
        "mentions" => format!("{x} mentions {y}"),
        "contacted" => format!("{x} contacted {y}"),
        "same_as" => format!("{x} is the same as {y}"),
        "paid_to" => format!("{x} paid {y}"),
        "located_in" => format!("{x} is located in {y}"),
        "member_of" => format!("{x} is a member of {y}"),
        "works_at" => format!("{x} works at {y}"),
        "sourced_from" => format!("{x} is sourced from {y}"),
        "derived_from" => format!("{x} is derived from {y}"),
        "related_to" => format!("{x} is related to {y}"),
        "posted_on" => format!("{x} posted on {y}"),
        other => format!("{x} —{other}→ {y}"),
    }
}

fn claim_status(confidence: f64, evidence_count: usize) -> &'static str {
    if confidence < UNVERIFIED_BELOW || evidence_count == 0 {
        "Unverified"
    } else if evidence_count >= 2 {
        "Supported"
    } else {
        "Single-source"
    }
}

fn subject_label(g: &Graph, subject: &str) -> String {
    if let Some(_e) = g.node(subject) {
        g.label(subject).to_string()
    } else if let Some(r) = g.edges.iter().find(|r| r.id == subject) {
        format!("relation {}", rel_sentence(&r.rel, g.label(&r.src), g.label(&r.dst)))
    } else {
        subject.to_string()
    }
}

fn ev_note(count: Option<usize>) -> String {
    match count {
        Some(n) if n > 0 => format!(", {n} evidence item(s)"),
        _ => String::new(),
    }
}

fn dedup_sources(evidence: &[Evidence]) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for ev in evidence {
        if let Some(url) = &ev.url {
            if seen.insert(url.clone()) {
                let title = ev.title.clone().unwrap_or_else(|| ev.kind.clone());
                out.push((title, url.clone()));
            }
        }
    }
    out
}

fn md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EntityPatch, EType, NewEntity, NewEvidence, NewRelation};
    use crate::store::Store;

    fn seeded_case() -> (Store, String, Graph, Case) {
        let store = Store::open_memory().unwrap();
        let case = store.create_case("Dossier Trial", "notes", &[]).unwrap();
        let alice = store
            .add_entity(&case.id, NewEntity::new(EType::Person, "alice").with_confidence(0.9))
            .unwrap()
            .entity;
        let acct = store
            .add_entity(&case.id, NewEntity::new(EType::Username, "johndoe"))
            .unwrap()
            .entity;
        let rel = store
            .add_relation(
                NewRelation::new(&case.id, &alice.id, &acct.id, "uses")
                    .with_confidence(0.9)
                    .with_source("hub:sherlock"),
            )
            .unwrap();
        store
            .add_evidence(
                NewEvidence::new(&case.id, &rel.id, "tool_result")
                    .with_title("sherlock hit")
                    .with_url("https://github.com/johndoe")
                    .with_snippet("found account"),
            )
            .unwrap();
        store
            .add_evidence(NewEvidence::new(&case.id, &alice.id, "note").with_title("analyst note"))
            .unwrap();
        let g = crate::graph::load_graph(&store, &case.id).unwrap();
        let case_full = store.get_case(&case.id).unwrap().unwrap();
        (store, case.id, g, case_full)
    }

    #[test]
    fn dossier_has_required_sections() {
        let (store, _cid, g, case) = seeded_case();
        let md = dossier(&g, &case, &store);
        assert!(md.starts_with("# Dossier Trial"), "{md}");
        assert!(md.contains("## Executive summary"));
        assert!(md.contains("## Key entities"));
        assert!(md.contains("## Key connections"));
        assert!(md.contains("## Communities"));
        assert!(md.contains("## Timeline"));
        assert!(md.contains("## Evidence appendix"));
        assert!(md.contains("## Sources"));
        assert!(md.contains("sherlock hit"), "evidence appendix lists evidence: {md}");
        assert!(md.contains("«alice» uses account «johndoe»"), "{md}");
    }

    #[test]
    fn brief_has_claim_ledger_and_sections() {
        let (store, _cid, g, case) = seeded_case();
        let md = brief(&g, &case, &store);
        assert!(md.contains("## Working lede angles"));
        assert!(md.contains("## Nut graf"));
        assert!(md.contains("## Claim ledger"));
        assert!(md.contains("## Open questions"));
        assert!(md.contains("## Source list"));
        assert!(md.contains("«alice» uses account «johndoe»"), "{md}");
        // one evidence item on the relation itself + none on src? src has a note → Supported
        assert!(md.contains("Supported"), "{md}");
        assert!(md.contains("https://github.com/johndoe"), "{md}");
    }

    #[test]
    fn claim_status_rules() {
        assert_eq!(claim_status(0.9, 3), "Supported");
        assert_eq!(claim_status(0.7, 1), "Single-source");
        assert_eq!(claim_status(0.7, 0), "Unverified");
        assert_eq!(claim_status(0.2, 5), "Unverified");
    }

    #[test]
    fn rel_sentence_templates() {
        assert_eq!(rel_sentence("uses", "a", "b"), "«a» uses account «b»");
        assert_eq!(rel_sentence("works_at", "a", "b"), "«a» works at «b»");
        assert_eq!(rel_sentence("weird_rel", "a", "b"), "«a» —weird_rel→ «b»");
    }

    #[test]
    fn brief_surfaces_open_questions_from_docs() {
        let (store, cid, g, case) = seeded_case();
        let alice = store
            .find_entity(&cid, "person", "alice")
            .unwrap()
            .expect("alice exists");
        store
            .set_doc(&alice.id, "# alice\n\n## Summary\nx\n\n## Open questions\n\nWho is johndoe?\n")
            .unwrap();
        let md = brief(&g, &case, &store);
        assert!(md.contains("**alice**: Who is johndoe?"), "{md}");
    }

    #[test]
    fn dossier_and_brief_tolerate_empty_graph() {
        let store = Store::open_memory().unwrap();
        let case = store.create_case("Empty", "", &[]).unwrap();
        let g = Graph::default();
        let d = dossier(&g, &case, &store);
        let b = brief(&g, &case, &store);
        assert!(d.contains("# Empty") && d.contains("## Evidence appendix"));
        assert!(b.contains("## Claim ledger"));
    }

    #[test]
    fn pinned_surfaces_in_dossier_table() {
        let (store, cid, _g, case) = seeded_case();
        let e = store
            .find_entity(&cid, "person", "alice")
            .unwrap()
            .expect("alice exists");
        let _ = store.update_entity(&e.id, EntityPatch { pinned: Some(true), ..Default::default() });
        let g = crate::graph::load_graph(&store, &cid).unwrap();
        let md = dossier(&g, &case, &store);
        assert!(md.contains("| yes |"), "{md}");
    }
}
