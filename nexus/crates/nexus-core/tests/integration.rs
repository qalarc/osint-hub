//! Integration tests — exercise nexus-core strictly through the public API.
//! DBs live in `tempfile::tempdir`s.

use nexus_core::{
    brief, communities, components, degree_centrality, dossier, extract_candidates, load_graph,
    neighbors, normalize, pagerank, shortest_path, suggest_pivots, AddOutcome, Case, Entity,
    EntityPatch, EType, Evidence, NewEntity, NewEvent, NewEvidence, NewRelation, PivotSuggestion,
    Relation, Store, TimelineEvent,
};

fn temp_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("nexus.db")).expect("open store");
    (dir, store)
}

/// a — b — c — d chain
fn chain(store: &Store) -> (String, Vec<String>) {
    let case = store.create_case("Chain Case", "", &[]).unwrap();
    let mut ids = Vec::new();
    for label in ["a", "b", "c", "d"] {
        let e = store
            .add_entity(&case.id, NewEntity::new(EType::Person, label))
            .unwrap()
            .entity;
        ids.push(e.id);
    }
    for w in ids.windows(2) {
        store
            .add_relation(NewRelation::new(&case.id, &w[0], &w[1], "related_to"))
            .unwrap();
    }
    (case.id, ids)
}

#[test]
fn store_case_crud_via_public_api() {
    let (_dir, store) = temp_store();
    let case: Case = store.create_case("CRUD Case", "n0", &["t1".to_string()]).unwrap();
    assert_eq!(store.list_cases().unwrap().len(), 1);
    assert_eq!(store.get_case(&case.id).unwrap().unwrap().tags, vec!["t1"]);

    store.update_case(&case.id, Some("archived"), Some("n1")).unwrap();
    let got = store.get_case(&case.id).unwrap().unwrap();
    assert_eq!((got.status.as_str(), got.notes.as_str()), ("archived", "n1"));

    store.delete_case(&case.id).unwrap();
    assert!(store.list_cases().unwrap().is_empty());
}

#[test]
fn entity_add_normalizes_and_dedups() {
    let (_dir, store) = temp_store();
    let case = store.create_case("Dedup", "", &[]).unwrap();

    let AddOutcome { entity: first, deduplicated } = store
        .add_entity(&case.id, NewEntity::new(EType::Email, "Ann@Example.ORG"))
        .unwrap();
    assert!(!deduplicated);
    let AddOutcome { entity: again, deduplicated } = store
        .add_entity(&case.id, NewEntity::new(EType::Email, "ann@example.org"))
        .unwrap();
    assert!(deduplicated);
    assert_eq!(first.id, again.id);

    let AddOutcome { entity: d1, .. } = store
        .add_entity(&case.id, NewEntity::new(EType::Domain, "WWW.Site.io"))
        .unwrap();
    let AddOutcome { entity: d2, deduplicated } = store
        .add_entity(&case.id, NewEntity::new(EType::Domain, "site.io"))
        .unwrap();
    assert!(deduplicated);
    assert_eq!(d1.id, d2.id);

    // patch + find + list
    let _ = store
        .update_entity(&again.id, EntityPatch { pinned: Some(true), ..Default::default() })
        .unwrap();
    assert!(store.get_entity(&again.id).unwrap().unwrap().pinned);
    assert!(store.find_entity(&case.id, "email", "ANN@example.ORG").unwrap().is_some());
    assert_eq!(store.list_entities(&case.id).unwrap().len(), 2);
}

#[test]
fn merge_entities_repoints_and_deletes_other() {
    let (_dir, store) = temp_store();
    let case = store.create_case("Merge", "", &[]).unwrap();
    let primary = store
        .add_entity(&case.id, NewEntity::new(EType::Person, "alice"))
        .unwrap()
        .entity;
    let other = store
        .add_entity(&case.id, NewEntity::new(EType::Person, "bob"))
        .unwrap()
        .entity;
    let acct = store
        .add_entity(&case.id, NewEntity::new(EType::Username, "bobby"))
        .unwrap()
        .entity;

    store.add_relation(NewRelation::new(&case.id, &other.id, &acct.id, "uses")).unwrap();
    store.add_evidence(NewEvidence::new(&case.id, &other.id, "web_fetch")).unwrap();

    let merged: Entity = store.merge_entities(&primary.id, &other.id).unwrap();
    assert!(merged.data["merged_from"].as_array().unwrap().iter().any(|v| v["label"] == "bob"));
    assert!(store.get_entity(&other.id).unwrap().is_none(), "other is gone");

    let rels = store.list_relations(&case.id).unwrap();
    assert_eq!(rels.len(), 1);
    assert_eq!(rels[0].src, primary.id, "relation re-pointed to primary");
    assert_eq!(store.list_evidence(&primary.id).unwrap().len(), 1, "evidence re-subjected");
}

#[test]
fn relation_add_is_idempotent_on_triple() {
    let (_dir, store) = temp_store();
    let case = store.create_case("RelDedup", "", &[]).unwrap();
    let a = store.add_entity(&case.id, NewEntity::new(EType::Person, "a")).unwrap().entity;
    let b = store.add_entity(&case.id, NewEntity::new(EType::Email, "a@b.c")).unwrap().entity;

    let r1: Relation = store.add_relation(NewRelation::new(&case.id, &a.id, &b.id, "owns")).unwrap();
    let r2 = store.add_relation(NewRelation::new(&case.id, &a.id, &b.id, "owns")).unwrap();
    assert_eq!(r1.id, r2.id);
    assert_eq!(store.list_relations(&case.id).unwrap().len(), 1);
}

#[test]
fn evidence_and_timeline_round_trip() {
    let (_dir, store) = temp_store();
    let case = store.create_case("EV", "", &[]).unwrap();
    let e = store.add_entity(&case.id, NewEntity::new(EType::Phone, "+14155550100")).unwrap().entity;

    let ev: Evidence = store
        .add_evidence(NewEvidence::new(&case.id, &e.id, "tool_result").with_url("https://example.test"))
        .unwrap();
    assert!(ev.id.starts_with("v_"));
    assert_eq!(store.list_evidence(&e.id).unwrap().len(), 1);

    let k: TimelineEvent = store.add_timeline(NewEvent::new(&case.id, "scan", "scanned").with_entity(&e.id)).unwrap();
    assert!(k.id.starts_with("k_"));
    assert_eq!(store.list_timeline(&case.id).unwrap().len(), 1);
}

#[test]
fn doc_render_preserves_prose_and_includes_connections() {
    let (_dir, store) = temp_store();
    let case = store.create_case("Docs", "", &[]).unwrap();
    let a = store.add_entity(&case.id, NewEntity::new(EType::Person, "ada")).unwrap().entity;
    let b = store
        .add_entity(&case.id, NewEntity::new(EType::Username, "ada_l"))
        .unwrap()
        .entity;
    store
        .add_relation(NewRelation::new(&case.id, &a.id, &b.id, "uses").with_confidence(0.9))
        .unwrap();
    store
        .set_doc(&a.id, "# ada\n\n## Summary\n\nAda keeps a low profile.\n\n## Open questions\n\nWhere does ada_l point?\n")
        .unwrap();

    let doc = store.render_doc(&a.id).unwrap();
    assert!(doc.contains("Ada keeps a low profile."), "summary preserved: {doc}");
    assert!(doc.contains("Where does ada_l point?"), "open questions preserved: {doc}");
    assert!(doc.contains("## Connections"), "{doc}");
    assert!(doc.contains("uses → ada_l (username) [conf 0.90]"), "{doc}");
}

#[test]
fn shortest_path_across_four_node_chain() {
    let (_dir, store) = temp_store();
    let (_cid, ids) = chain(&store);
    let g = load_graph(&store, &_cid).unwrap();
    let path = shortest_path(&g, &ids[0], &ids[3]).unwrap();
    assert_eq!(path, ids);
    assert!(shortest_path(&g, &ids[0], "e_missing").is_none());
}

#[test]
fn pagerank_ranks_hub_highest_in_star_graph() {
    let (_dir, store) = temp_store();
    let case = store.create_case("Star", "", &[]).unwrap();
    let hub = store.add_entity(&case.id, NewEntity::new(EType::Person, "hub")).unwrap().entity;
    for i in 0..6 {
        let leaf = store
            .add_entity(&case.id, NewEntity::new(EType::Note, format!("leaf{i}")))
            .unwrap()
            .entity;
        store
            .add_relation(NewRelation::new(&case.id, &hub.id, &leaf.id, "related_to"))
            .unwrap();
    }
    let g = load_graph(&store, &case.id).unwrap();
    let pr = pagerank(&g, 0.85, 100);
    for leaf in store.list_entities(&case.id).unwrap().iter().filter(|e| e.id != hub.id) {
        assert!(pr[&hub.id] > pr[&leaf.id], "hub must outrank {}", leaf.label);
    }
    assert_eq!(degree_centrality(&g)[&hub.id], 6);
    assert_eq!(neighbors(&g, &hub.id, 1).len(), 6);
}

#[test]
fn components_separate_two_islands() {
    let (_dir, store) = temp_store();
    let case = store.create_case("Islands", "", &[]).unwrap();
    let mut ids = Vec::new();
    for label in ["i1", "i2", "j1", "j2", "lonely"] {
        let e = store.add_entity(&case.id, NewEntity::new(EType::Person, label)).unwrap().entity;
        ids.push(e.id);
    }
    store.add_relation(NewRelation::new(&case.id, &ids[0], &ids[1], "related_to")).unwrap();
    store.add_relation(NewRelation::new(&case.id, &ids[2], &ids[3], "related_to")).unwrap();
    let g = load_graph(&store, &case.id).unwrap();
    let comps = components(&g);
    assert_eq!(comps.len(), 3, "two pairs + one isolated: {comps:?}");
}

#[test]
fn communities_split_two_cliques() {
    let (_dir, store) = temp_store();
    let case = store.create_case("Cliques", "", &[]).unwrap();
    let mut ids = Vec::new();
    for label in ["c1", "c2", "c3", "k1", "k2", "k3"] {
        let e = store.add_entity(&case.id, NewEntity::new(EType::Person, label)).unwrap().entity;
        ids.push(e.id);
    }
    for (i, j) in [(0, 1), (0, 2), (1, 2), (3, 4), (3, 5), (4, 5)] {
        store
            .add_relation(NewRelation::new(&case.id, &ids[i], &ids[j], "related_to"))
            .unwrap();
    }
    let g = load_graph(&store, &case.id).unwrap();
    let com = communities(&g);
    assert_eq!(com[&ids[0]], com[&ids[1]]);
    assert_eq!(com[&ids[0]], com[&ids[2]]);
    assert_eq!(com[&ids[3]], com[&ids[5]]);
    assert_ne!(com[&ids[0]], com[&ids[3]]);
}

#[test]
fn normalize_phone_to_e164() {
    assert_eq!(normalize("phone", "+1 (415) 555-0100"), "+14155550100");
    assert_eq!(normalize("phone", "+442071234567"), "+442071234567");
    assert_eq!(normalize("phone", "0-555-0100"), "05550100");
}

#[test]
fn extract_candidates_finds_all_types_in_paragraph() {
    let text = "Reached bob@corp.example about +1 (415) 555-0100. \
                Check https://darktrace.io and ns1.darktrace.io, also 8.8.8.8. \
                See github.com/octocat for the wallet 3FupZp77ySr7jwoLYEJ9mwzJpvoNBXsBnE.";
    let cands = extract_candidates(text);
    let find = |t: EType| cands.iter().find(|(e, _)| *e == t).map(|(_, v)| v.clone());
    assert_eq!(find(EType::Email).as_deref(), Some("bob@corp.example"));
    assert_eq!(find(EType::Phone).as_deref(), Some("+14155550100"));
    assert!(cands.iter().any(|(e, v)| *e == EType::Domain && v == "darktrace.io"));
    assert!(cands.iter().any(|(e, v)| *e == EType::Domain && v == "ns1.darktrace.io"));
    assert_eq!(find(EType::Ip).as_deref(), Some("8.8.8.8"));
    assert_eq!(find(EType::Username).as_deref(), Some("octocat"));
    assert!(find(EType::Wallet).is_some(), "{cands:?}");
}

#[test]
fn pivots_respect_connectors_and_ranking() {
    let (_dir, store) = temp_store();
    let case = store.create_case("Pivots", "", &[]).unwrap();
    let user = store
        .add_entity(&case.id, NewEntity::new(EType::Username, "ghost10"))
        .unwrap()
        .entity;
    let g = load_graph(&store, &case.id).unwrap();

    // no hub → no hub_scan
    let empty: Vec<PivotSuggestion> = suggest_pivots(&g, &user.id, &["flowsint"]);
    assert!(empty.is_empty(), "{empty:?}");

    // hub available → hub_scan:username on top
    let pivots = suggest_pivots(&g, &user.id, &["hub", "archon"]);
    assert_eq!(pivots[0].action, "hub_scan:username");
    assert!((pivots[0].priority - 1.0).abs() < 1e-9); // 0.9 + 0.1 zero-evidence boost
    assert!(pivots.iter().any(|p| p.action == "archon_ground"));
    assert!(!pivots.iter().any(|p| p.action.starts_with("flowsint")));
}

#[test]
fn dossier_contains_sections_and_case_name() {
    let (_dir, store) = temp_store();
    let case = store.create_case("Deep Dive onSubject", "", &[]).unwrap();
    let a = store.add_entity(&case.id, NewEntity::new(EType::Person, "dana")).unwrap().entity;
    let b = store.add_entity(&case.id, NewEntity::new(EType::Email, "dana@x.io")).unwrap().entity;
    let rel = store
        .add_relation(NewRelation::new(&case.id, &a.id, &b.id, "uses").with_confidence(0.95))
        .unwrap();
    store
        .add_evidence(NewEvidence::new(&case.id, &rel.id, "manual").with_title("verified signup").with_url("https://x.io"))
        .unwrap();
    store.add_timeline(NewEvent::new(&case.id, "ingest", "imported seeds").with_entity(&a.id)).unwrap();

    let g = load_graph(&store, &case.id).unwrap();
    let case_full = store.get_case(&case.id).unwrap().unwrap();
    let md = dossier(&g, &case_full, &store);
    assert!(md.contains("# Deep Dive onSubject"), "{md}");
    assert!(md.contains("## Evidence appendix"), "{md}");
    assert!(md.contains("verified signup"), "{md}");
    assert!(md.contains("## Executive summary"), "{md}");
    assert!(md.contains("## Key entities"), "{md}");
    assert!(md.contains("imported seeds"), "{md}");
}

#[test]
fn brief_contains_claim_ledger() {
    let (_dir, store) = temp_store();
    let case = store.create_case("Story", "", &[]).unwrap();
    let a = store.add_entity(&case.id, NewEntity::new(EType::Person, "erin")).unwrap().entity;
    let b = store.add_entity(&case.id, NewEntity::new(EType::Org, "Acme LLC")).unwrap().entity;
    store
        .add_relation(NewRelation::new(&case.id, &a.id, &b.id, "works_at").with_confidence(0.9))
        .unwrap();
    store.add_evidence(NewEvidence::new(&case.id, &a.id, "web_fetch").with_url("https://linkedin.test/erin")).unwrap();
    store.add_evidence(NewEvidence::new(&case.id, &a.id, "note")).unwrap();

    let g = load_graph(&store, &case.id).unwrap();
    let case_full = store.get_case(&case.id).unwrap().unwrap();
    let md = brief(&g, &case_full, &store);
    assert!(md.contains("## Claim ledger"), "{md}");
    assert!(md.contains("## Working lede angles"), "{md}");
    assert!(md.contains("## Nut graf"), "{md}");
    assert!(md.contains("## Source list"), "{md}");
    assert!(md.contains("«erin» works at «Acme LLC»"), "{md}");
    assert!(md.contains("Supported"), "two evidence items on src → Supported: {md}");
}

#[test]
fn export_shape_round_trip() {
    // store types must survive serde round-trips (needed by export.json/import)
    let (_dir, store) = temp_store();
    let case = store.create_case("RT", "", &[]).unwrap();
    let e = store
        .add_entity(&case.id, NewEntity::new(EType::Website, "https://Example.io/a"))
        .unwrap()
        .entity;
    let json = serde_json::to_string(&e).unwrap();
    let back: Entity = serde_json::from_str(&json).unwrap();
    assert_eq!(back, e);
    assert!(json.contains("\"etype\":\"website\""));
}
