//! Graph loading + algorithms — CONTRACT §3 (`graph.rs`).
//!
//! All algorithms treat edges as an **undirected** view and are deterministic
//! (sorted ids / stable tie-breaks).

use crate::model::{Entity, Relation};
use crate::store::Store;
use crate::Result;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// In-memory case graph.
#[derive(Debug, Clone, Default)]
pub struct Graph {
    pub nodes: Vec<Entity>,
    pub edges: Vec<Relation>,
}

impl Graph {
    pub fn node(&self, id: &str) -> Option<&Entity> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Label of a node, falling back to the raw id (for dangling refs).
    pub fn label<'a>(&'a self, id: &'a str) -> &'a str {
        self.node(id).map(|n| n.label.as_str()).unwrap_or(id)
    }

    pub fn etype<'a>(&'a self, id: &'a str) -> &'a str {
        self.node(id).map(|n| n.etype.as_str()).unwrap_or("unknown")
    }
}

/// Loads the full case graph (nodes = entities, edges = relations).
pub fn load_graph(store: &Store, case_id: &str) -> Result<Graph> {
    let nodes = store.list_entities(case_id)?;
    let edges = store.list_relations(case_id)?;
    Ok(Graph { nodes, edges })
}

/// Undirected adjacency as index lists (self-loops dropped).
fn adjacency(g: &Graph) -> Vec<Vec<usize>> {
    let idx = index(g);
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); g.nodes.len()];
    for e in &g.edges {
        if e.src == e.dst {
            continue;
        }
        if let (Some(&i), Some(&j)) = (idx.get(e.src.as_str()), idx.get(e.dst.as_str())) {
            adj[i].push(j);
            adj[j].push(i);
        }
    }
    adj
}

fn index(g: &Graph) -> HashMap<&str, usize> {
    g.nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect()
}

/// Entities reachable within `depth` hops (undirected), start node excluded,
/// sorted by id for determinism.
pub fn neighbors<'a>(g: &'a Graph, id: &str, depth: usize) -> Vec<&'a Entity> {
    let idx = index(g);
    let Some(&start) = idx.get(id) else {
        return Vec::new();
    };
    let adj = adjacency(g);
    let mut seen = vec![false; g.nodes.len()];
    seen[start] = true;
    let mut found: Vec<usize> = Vec::new();
    let mut frontier = vec![start];
    for _ in 0..depth {
        let mut next: Vec<usize> = Vec::new();
        for &u in &frontier {
            for &v in &adj[u] {
                if !seen[v] {
                    seen[v] = true;
                    next.push(v);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        found.extend_from_slice(&next);
        frontier = next;
    }
    let mut ents: Vec<&Entity> = found.into_iter().map(|i| &g.nodes[i]).collect();
    ents.sort_by(|a, b| a.id.cmp(&b.id));
    ents
}

/// Shortest path (BFS, undirected) between two entity ids, inclusive of both
/// endpoints. `None` when disconnected or unknown.
pub fn shortest_path(g: &Graph, a: &str, b: &str) -> Option<Vec<String>> {
    if a == b {
        return if g.node(a).is_some() { Some(vec![a.to_string()]) } else { None };
    }
    let idx = index(g);
    let start = *idx.get(a)?;
    let target = *idx.get(b)?;
    let adj = adjacency(g);
    let mut parent: HashMap<usize, usize> = HashMap::new();
    let mut seen = vec![false; g.nodes.len()];
    seen[start] = true;
    let mut queue = VecDeque::from([start]);
    while let Some(u) = queue.pop_front() {
        if u == target {
            break;
        }
        for &v in &adj[u] {
            if !seen[v] {
                seen[v] = true;
                parent.insert(v, u);
                queue.push_back(v);
            }
        }
    }
    if !seen[target] {
        return None;
    }
    let mut path = vec![target];
    let mut cur = target;
    while let Some(&p) = parent.get(&cur) {
        path.push(p);
        cur = p;
    }
    path.reverse();
    Some(path.into_iter().map(|i| g.nodes[i].id.clone()).collect())
}

/// Distinct-neighbor count per node (every node present, 0 default).
pub fn degree_centrality(g: &Graph) -> BTreeMap<String, usize> {
    let mut map: BTreeMap<String, HashSet<String>> = g
        .nodes
        .iter()
        .map(|n| (n.id.clone(), HashSet::new()))
        .collect();
    for e in &g.edges {
        if e.src == e.dst {
            continue;
        }
        if let Some(s) = map.get_mut(&e.src) {
            s.insert(e.dst.clone());
        }
        if let Some(s) = map.get_mut(&e.dst) {
            s.insert(e.src.clone());
        }
    }
    map.into_iter().map(|(k, v)| (k, v.len())).collect()
}

/// PageRank, power iteration, undirected view. Stops at `iters` or when the
/// L1 delta drops below 1e-9. Dangling mass is spread uniformly.
pub fn pagerank(g: &Graph, damping: f64, iters: usize) -> BTreeMap<String, f64> {
    let n = g.nodes.len();
    if n == 0 {
        return BTreeMap::new();
    }
    let adj = adjacency(g);
    let nf = n as f64;
    let mut rank = vec![1.0 / nf; n];
    for _ in 0..iters {
        let mut next = vec![(1.0 - damping) / nf; n];
        let dangling: f64 = (0..n).filter(|&i| adj[i].is_empty()).map(|i| rank[i]).sum();
        let dangling_add = damping * dangling / nf;
        for v in next.iter_mut() {
            *v += dangling_add;
        }
        for (i, nbrs) in adj.iter().enumerate() {
            if nbrs.is_empty() {
                continue;
            }
            let share = damping * rank[i] / nbrs.len() as f64;
            for &j in nbrs {
                next[j] += share;
            }
        }
        let delta: f64 = (0..n).map(|i| (next[i] - rank[i]).abs()).sum();
        rank = next;
        if delta < 1e-9 {
            break;
        }
    }
    g.nodes
        .iter()
        .zip(rank)
        .map(|(n, r)| (n.id.clone(), r))
        .collect()
}

/// Connected components (union-find), members sorted, components ordered by
/// first member id.
pub fn components(g: &Graph) -> Vec<Vec<String>> {
    let n = g.nodes.len();
    let idx = index(g);
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    for e in &g.edges {
        if let (Some(&i), Some(&j)) = (idx.get(e.src.as_str()), idx.get(e.dst.as_str())) {
            let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
            if ri != rj {
                parent[ri] = rj;
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (i, node) in g.nodes.iter().enumerate() {
        groups
            .entry(find(&mut parent, i))
            .or_default()
            .push(node.id.clone());
    }
    let mut out: Vec<Vec<String>> = groups.into_values().collect();
    for members in &mut out {
        members.sort();
    }
    out.sort_by(|a, b| a[0].cmp(&b[0]));
    out
}

/// Label propagation communities (20 rounds, undirected, ties broken by the
/// smaller label). Returns `node_id → community index` with indices assigned
/// in ascending order of final community labels for determinism.
pub fn communities(g: &Graph) -> BTreeMap<String, usize> {
    let n = g.nodes.len();
    if n == 0 {
        return BTreeMap::new();
    }
    let adj = adjacency(g);
    let mut labels: Vec<usize> = (0..n).collect();
    for _ in 0..20 {
        let prev = labels.clone();
        let mut changed = false;
        for i in 0..n {
            if adj[i].is_empty() {
                continue;
            }
            let mut counts: BTreeMap<usize, usize> = BTreeMap::new();
            for &j in &adj[i] {
                *counts.entry(prev[j]).or_default() += 1;
            }
            // most frequent neighbor label; ties → smallest label value
            let best = counts
                .iter()
                .max_by_key(|(label, cnt)| (**cnt, std::cmp::Reverse(**label)))
                .map(|(label, _)| *label)
                .expect("non-empty counts");
            if labels[i] != best {
                labels[i] = best;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // compress labels to dense community indices in ascending label order
    let mut distinct: Vec<usize> = labels.clone();
    distinct.sort_unstable();
    distinct.dedup();
    let rank_of: HashMap<usize, usize> = distinct
        .iter()
        .enumerate()
        .map(|(rank, label)| (*label, rank))
        .collect();
    g.nodes
        .iter()
        .zip(labels)
        .map(|(node, label)| (node.id.clone(), rank_of[&label]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NewEntity, NewRelation, EType};
    use crate::store::Store;

    fn seeded() -> (Store, String) {
        let store = Store::open_memory().unwrap();
        let case = store.create_case("g", "", &[]).unwrap();
        (store, case.id)
    }

    fn star(store: &Store, case_id: &str, hub_label: &str, leaves: usize) -> (String, Vec<String>) {
        let hub = store.add_entity(case_id, NewEntity::new(EType::Person, hub_label)).unwrap().entity;
        let mut leaf_ids = Vec::new();
        for i in 0..leaves {
            let leaf = store
                .add_entity(case_id, NewEntity::new(EType::Note, format!("leaf{i}")))
                .unwrap()
                .entity;
            store
                .add_relation(NewRelation::new(case_id, &hub.id, &leaf.id, "related_to"))
                .unwrap();
            leaf_ids.push(leaf.id);
        }
        (hub.id, leaf_ids)
    }

    #[test]
    fn shortest_path_across_chain() {
        let (store, cid) = seeded();
        let mut ids = Vec::new();
        for label in ["a", "b", "c", "d"] {
            let e = store.add_entity(&cid, NewEntity::new(EType::Person, label)).unwrap().entity;
            ids.push(e.id);
        }
        store.add_relation(NewRelation::new(&cid, &ids[0], &ids[1], "related_to")).unwrap();
        store.add_relation(NewRelation::new(&cid, &ids[1], &ids[2], "related_to")).unwrap();
        store.add_relation(NewRelation::new(&cid, &ids[2], &ids[3], "related_to")).unwrap();
        let g = load_graph(&store, &cid).unwrap();
        assert_eq!(
            shortest_path(&g, &ids[0], &ids[3]).unwrap(),
            ids
        );
        let mut rev = ids.clone();
        rev.reverse();
        assert_eq!(shortest_path(&g, &ids[3], &ids[0]).unwrap(), rev, "undirected");
        assert_eq!(shortest_path(&g, &ids[0], &ids[0]).unwrap(), vec![ids[0].clone()]);
    }

    #[test]
    fn pagerank_ranks_star_hub_highest() {
        let (store, cid) = seeded();
        let (hub, leaves) = star(&store, &cid, "hub", 5);
        let g = load_graph(&store, &cid).unwrap();
        let pr = pagerank(&g, 0.85, 100);
        assert!(pr[&hub] > *leaves.iter().map(|l| &pr[l]).max_by(|a, b| a.total_cmp(b)).unwrap());
        let sum: f64 = pr.values().sum();
        assert!((sum - 1.0).abs() < 1e-6, "ranks sum to 1, got {sum}");
        assert!(degree_centrality(&g)[&hub] == 5);
    }

    #[test]
    fn components_separate_two_islands() {
        let (store, cid) = seeded();
        let mut ids = Vec::new();
        for label in ["a", "b", "c", "d"] {
            let e = store.add_entity(&cid, NewEntity::new(EType::Person, label)).unwrap().entity;
            ids.push(e.id);
        }
        store.add_relation(NewRelation::new(&cid, &ids[0], &ids[1], "related_to")).unwrap();
        store.add_relation(NewRelation::new(&cid, &ids[2], &ids[3], "related_to")).unwrap();
        let g = load_graph(&store, &cid).unwrap();
        let comps = components(&g);
        assert_eq!(comps.len(), 2);
        let a: Vec<&Vec<String>> = comps.iter().filter(|c| c.contains(&ids[0])).collect();
        assert!(a[0].contains(&ids[1]) && !a[0].contains(&ids[2]));
    }

    #[test]
    fn communities_split_two_cliques() {
        let (store, cid) = seeded();
        let mut ids = Vec::new();
        for label in ["a", "b", "c", "d", "e", "f"] {
            let e = store.add_entity(&cid, NewEntity::new(EType::Person, label)).unwrap().entity;
            ids.push(e.id.clone());
        }
        // clique 1: a-b-c, clique 2: d-e-f, no bridge
        for (i, j) in [(0, 1), (0, 2), (1, 2), (3, 4), (3, 5), (4, 5)] {
            store
                .add_relation(NewRelation::new(&cid, &ids[i], &ids[j], "related_to"))
                .unwrap();
        }
        let g = load_graph(&store, &cid).unwrap();
        let com = communities(&g);
        assert_eq!(com[&ids[0]], com[&ids[1]]);
        assert_eq!(com[&ids[0]], com[&ids[2]]);
        assert_eq!(com[&ids[3]], com[&ids[4]]);
        assert_ne!(com[&ids[0]], com[&ids[3]]);
    }

    #[test]
    fn neighbors_depth_semantics() {
        let (store, cid) = seeded();
        let (hub, leaves) = star(&store, &cid, "hub", 3);
        let g = load_graph(&store, &cid).unwrap();
        assert_eq!(neighbors(&g, &hub, 1).len(), 3);
        assert_eq!(neighbors(&g, &hub, 3).len(), 3);
        assert!(neighbors(&g, "e_ghost", 2).is_empty());
        let leaf0 = &leaves[0];
        assert_eq!(neighbors(&g, leaf0, 1).len(), 1);
    }
}
