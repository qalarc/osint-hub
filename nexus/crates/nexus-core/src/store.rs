//! SQLite-backed store — CONTRACT §3. WAL, auto-migration, Mutex-guarded.

use crate::model::{
    AddOutcome, Case, Entity, EntityPatch, EType, Evidence, NewEntity, NewEvent, NewEvidence,
    NewRelation, Relation, TimelineEvent,
};
use crate::{normalize, new_id, now_iso, NexusError, Result};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde_json::Value;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

const MIGRATION_V1: &str = r#"
CREATE TABLE IF NOT EXISTS cases (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    slug TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    notes TEXT NOT NULL DEFAULT '',
    tags TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS entities (
    id TEXT PRIMARY KEY,
    case_id TEXT NOT NULL,
    etype TEXT NOT NULL,
    label TEXT NOT NULL,
    data TEXT NOT NULL DEFAULT '{}',
    confidence REAL NOT NULL DEFAULT 1.0,
    pinned INTEGER NOT NULL DEFAULT 0,
    norm_key TEXT NOT NULL DEFAULT '',
    first_seen TEXT NOT NULL,
    last_seen TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS relations (
    id TEXT PRIMARY KEY,
    case_id TEXT NOT NULL,
    src TEXT NOT NULL,
    dst TEXT NOT NULL,
    rel TEXT NOT NULL,
    weight REAL NOT NULL DEFAULT 1.0,
    confidence REAL NOT NULL DEFAULT 1.0,
    source TEXT,
    evidence TEXT,
    created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS evidence (
    id TEXT PRIMARY KEY,
    case_id TEXT NOT NULL,
    subject TEXT NOT NULL,
    kind TEXT NOT NULL,
    title TEXT,
    url TEXT,
    snippet TEXT,
    raw TEXT NOT NULL DEFAULT '{}',
    confidence REAL NOT NULL DEFAULT 1.0,
    ts TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS timeline (
    id TEXT PRIMARY KEY,
    case_id TEXT NOT NULL,
    ts TEXT NOT NULL,
    entity_id TEXT,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    detail TEXT NOT NULL DEFAULT '{}',
    source TEXT
);
CREATE TABLE IF NOT EXISTS docs (
    entity_id TEXT PRIMARY KEY,
    markdown TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_entities_norm ON entities(case_id, etype, norm_key);
CREATE UNIQUE INDEX IF NOT EXISTS uq_relations_triple ON relations(src, dst, rel);
CREATE INDEX IF NOT EXISTS ix_evidence_subject ON evidence(subject);
CREATE INDEX IF NOT EXISTS ix_timeline_case ON timeline(case_id, ts);
"#;

/// Case store. All methods take `&self`; the connection sits behind a Mutex.
pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    /// In-memory DB (handy for tests/embedding).
    pub fn open_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        conn.execute_batch(MIGRATION_V1)?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            conn.pragma_update(None, "user_version", 1)?;
        }
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn conn(&self) -> Result<MutexGuard<'_, Connection>> {
        self.conn.lock().map_err(|p| {
            NexusError::Invalid(format!("store lock poisoned: {p}"))
        })
    }

    // ------------------------------------------------------------- cases

    pub fn create_case(&self, name: &str, notes: &str, tags: &[String]) -> Result<Case> {
        let conn = self.conn()?;
        let now = now_iso();
        let id = new_id("c");
        let base_slug = slugify(name);
        let mut slug = base_slug.clone();
        let mut n = 1;
        while case_by_slug(&conn, &slug)?.is_some() {
            n += 1;
            slug = format!("{base_slug}-{n}");
        }
        let case = Case {
            id: id.clone(),
            name: name.to_string(),
            slug,
            status: "active".to_string(),
            notes: notes.to_string(),
            tags: tags.to_vec(),
            created_at: now.clone(),
            updated_at: now,
        };
        conn.execute(
            "INSERT INTO cases (id, name, slug, status, notes, tags, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                case.id,
                case.name,
                case.slug,
                case.status,
                case.notes,
                serde_json::to_string(&case.tags)?,
                case.created_at,
                case.updated_at
            ],
        )?;
        Ok(case)
    }

    pub fn list_cases(&self) -> Result<Vec<Case>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare("SELECT * FROM cases ORDER BY created_at, id")?;
        let rows = stmt
            .query_map([], case_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_case(&self, id: &str) -> Result<Option<Case>> {
        let conn = self.conn()?;
        case_by_id(&conn, id)
    }

    pub fn update_case(&self, id: &str, status: Option<&str>, notes: Option<&str>) -> Result<()> {
        let conn = self.conn()?;
        let n = conn.execute(
            "UPDATE cases SET
                status = COALESCE(?2, status),
                notes = COALESCE(?3, notes),
                updated_at = ?4
             WHERE id = ?1",
            params![id, status, notes, now_iso()],
        )?;
        if n == 0 {
            return Err(NexusError::NotFound(format!("case {id}")));
        }
        Ok(())
    }

    pub fn delete_case(&self, id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM docs WHERE entity_id IN (SELECT id FROM entities WHERE case_id = ?1)",
            params![id],
        )?;
        conn.execute("DELETE FROM timeline WHERE case_id = ?1", params![id])?;
        conn.execute("DELETE FROM evidence WHERE case_id = ?1", params![id])?;
        conn.execute("DELETE FROM relations WHERE case_id = ?1", params![id])?;
        conn.execute("DELETE FROM entities WHERE case_id = ?1", params![id])?;
        let n = conn.execute("DELETE FROM cases WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(NexusError::NotFound(format!("case {id}")));
        }
        Ok(())
    }

    // ---------------------------------------------------------- entities

    /// Adds (or dedups) an entity. Normalization per CONTRACT §2; the unique
    /// key is `(case_id, etype, norm_key)`. On dedup: confidence is raised to
    /// the max, `last_seen` refreshed and `data` keys merged (new wins).
    pub fn add_entity(&self, case_id: &str, new: NewEntity) -> Result<AddOutcome> {
        let conn = self.conn()?;
        add_entity_conn(&conn, case_id, new)
    }

    pub fn get_entity(&self, id: &str) -> Result<Option<Entity>> {
        let conn = self.conn()?;
        entity_by_id(&conn, id)
    }

    pub fn update_entity(&self, id: &str, patch: EntityPatch) -> Result<Entity> {
        let conn = self.conn()?;
        let existing = entity_by_id(&conn, id)?.ok_or_else(|| NexusError::NotFound(format!("entity {id}")))?;
        let data = match patch.data {
            Some(d) => d,
            None => existing.data.clone(),
        };
        conn.execute(
            "UPDATE entities SET
                label = COALESCE(?2, label),
                data = ?3,
                confidence = COALESCE(?4, confidence),
                pinned = COALESCE(?5, pinned),
                last_seen = ?6
             WHERE id = ?1",
            params![
                id,
                patch.label,
                serde_json::to_string(&data)?,
                patch.confidence,
                patch.pinned.map(|b| b as i64),
                now_iso()
            ],
        )?;
        entity_by_id(&conn, id)?.ok_or_else(|| NexusError::NotFound(format!("entity {id}")))
    }

    /// Deletes an entity plus incident relations, its evidence, docs, and
    /// timeline events attached to it.
    pub fn delete_entity(&self, id: &str) -> Result<()> {
        let conn = self.conn()?;
        let n = conn.execute("DELETE FROM entities WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(NexusError::NotFound(format!("entity {id}")));
        }
        conn.execute(
            "DELETE FROM relations WHERE src = ?1 OR dst = ?1",
            params![id],
        )?;
        conn.execute("DELETE FROM evidence WHERE subject = ?1", params![id])?;
        conn.execute("DELETE FROM docs WHERE entity_id = ?1", params![id])?;
        conn.execute(
            "UPDATE timeline SET entity_id = NULL WHERE entity_id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// Merges `other` into `primary`: relations re-pointed (collapsing into
    /// duplicates when primary already has the same (src,dst,rel)), evidence
    /// re-subjected, timeline re-pointed, doc adopted if primary had none,
    /// audit entry appended under `data.merged_from`, then `other` deleted.
    pub fn merge_entities(&self, primary: &str, other: &str) -> Result<Entity> {
        let conn = self.conn()?;
        merge_entities_conn(&conn, primary, other)
    }

    pub fn find_entity(&self, case_id: &str, etype: &str, label: &str) -> Result<Option<Entity>> {
        let conn = self.conn()?;
        let norm = normalize(etype, label);
        let mut stmt = conn.prepare(
            "SELECT * FROM entities WHERE case_id = ?1 AND etype = ?2 AND norm_key = ?3",
        )?;
        let found = stmt
            .query_row(params![case_id, etype, norm], entity_from_row)
            .optional()?;
        Ok(found)
    }

    pub fn list_entities(&self, case_id: &str) -> Result<Vec<Entity>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT * FROM entities WHERE case_id = ?1 ORDER BY first_seen, id",
        )?;
        let rows = stmt
            .query_map(params![case_id], entity_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Relations incident to one entity (either direction).
    pub fn list_entity_relations(&self, entity_id: &str) -> Result<Vec<Relation>> {
        let conn = self.conn()?;
        entity_relations(&conn, entity_id)
    }

    // -------------------------------------------------- relations/evidence

    /// Dedups on `(src, dst, rel)` — returns the existing row (raising
    /// confidence to the max and back-filling source/evidence when empty).
    pub fn add_relation(&self, new: NewRelation) -> Result<Relation> {
        let conn = self.conn()?;
        add_relation_conn(&conn, new)
    }

    pub fn delete_relation(&self, id: &str) -> Result<()> {
        let conn = self.conn()?;
        let n = conn.execute("DELETE FROM relations WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(NexusError::NotFound(format!("relation {id}")));
        }
        conn.execute("DELETE FROM evidence WHERE subject = ?1", params![id])?;
        Ok(())
    }

    pub fn list_relations(&self, case_id: &str) -> Result<Vec<Relation>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT * FROM relations WHERE case_id = ?1 ORDER BY created_at, id",
        )?;
        let rows = stmt
            .query_map(params![case_id], relation_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn add_evidence(&self, new: NewEvidence) -> Result<Evidence> {
        let conn = self.conn()?;
        let id = new_id("v");
        let ts = now_iso();
        conn.execute(
            "INSERT INTO evidence (id, case_id, subject, kind, title, url, snippet, raw, confidence, ts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id,
                new.case_id,
                new.subject,
                new.kind,
                new.title,
                new.url,
                new.snippet,
                serde_json::to_string(&new.raw)?,
                new.confidence,
                ts
            ],
        )?;
        Ok(Evidence {
            id,
            case_id: new.case_id,
            subject: new.subject,
            kind: new.kind,
            title: new.title,
            url: new.url,
            snippet: new.snippet,
            raw: new.raw,
            confidence: new.confidence,
            ts,
        })
    }

    pub fn list_evidence(&self, subject: &str) -> Result<Vec<Evidence>> {
        let conn = self.conn()?;
        let mut stmt =
            conn.prepare("SELECT * FROM evidence WHERE subject = ?1 ORDER BY ts, id")?;
        let rows = stmt
            .query_map(params![subject], evidence_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// All evidence for a case (report appendix). Additive to CONTRACT §3.
    pub fn list_case_evidence(&self, case_id: &str) -> Result<Vec<Evidence>> {
        let conn = self.conn()?;
        let mut stmt =
            conn.prepare("SELECT * FROM evidence WHERE case_id = ?1 ORDER BY ts, id")?;
        let rows = stmt
            .query_map(params![case_id], evidence_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ------------------------------------------------------------ timeline

    pub fn add_timeline(&self, new: NewEvent) -> Result<TimelineEvent> {
        let conn = self.conn()?;
        let id = new_id("k");
        let ts = new.ts.unwrap_or_else(now_iso);
        conn.execute(
            "INSERT INTO timeline (id, case_id, ts, entity_id, kind, title, detail, source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                new.case_id,
                ts,
                new.entity_id,
                new.kind,
                new.title,
                serde_json::to_string(&new.detail)?,
                new.source
            ],
        )?;
        Ok(TimelineEvent {
            id,
            case_id: new.case_id,
            ts,
            entity_id: new.entity_id,
            kind: new.kind,
            title: new.title,
            detail: new.detail,
            source: new.source,
        })
    }

    pub fn list_timeline(&self, case_id: &str) -> Result<Vec<TimelineEvent>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT * FROM timeline WHERE case_id = ?1 ORDER BY ts, id",
        )?;
        let rows = stmt
            .query_map(params![case_id], event_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---------------------------------------------------------------- docs

    pub fn get_doc(&self, entity_id: &str) -> Result<Option<String>> {
        let conn = self.conn()?;
        let mut stmt =
            conn.prepare("SELECT markdown FROM docs WHERE entity_id = ?1")?;
        Ok(stmt.query_row(params![entity_id], |r| r.get(0)).optional()?)
    }

    pub fn set_doc(&self, entity_id: &str, markdown: &str) -> Result<()> {
        let conn = self.conn()?;
        set_doc(&conn, entity_id, markdown)
    }

    /// Regenerates the live sections of the entity wiki doc; human prose under
    /// `## Summary` and `## Open questions` is preserved verbatim.
    pub fn render_doc(&self, entity_id: &str) -> Result<String> {
        let conn = self.conn()?;
        render_doc_conn(&conn, entity_id)
    }

    /// `(entity_id, markdown)` pairs for every doc of a case's entities.
    pub fn list_docs_for_case(&self, case_id: &str) -> Result<Vec<(String, String)>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT d.entity_id, d.markdown FROM docs d
             JOIN entities e ON e.id = d.entity_id
             WHERE e.case_id = ?1 ORDER BY d.entity_id",
        )?;
        let rows = stmt
            .query_map(params![case_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

// ============================================================ conn helpers
// (private: safe to call while the store lock is held)

fn case_by_id(conn: &Connection, id: &str) -> Result<Option<Case>> {
    let mut stmt = conn.prepare("SELECT * FROM cases WHERE id = ?1")?;
    Ok(stmt.query_row(params![id], case_from_row).optional()?)
}

fn case_by_slug(conn: &Connection, slug: &str) -> Result<Option<Case>> {
    let mut stmt = conn.prepare("SELECT * FROM cases WHERE slug = ?1")?;
    Ok(stmt.query_row(params![slug], case_from_row).optional()?)
}

fn case_from_row(row: &Row<'_>) -> rusqlite::Result<Case> {
    let tags_raw: String = row.get("tags")?;
    Ok(Case {
        id: row.get("id")?,
        name: row.get("name")?,
        slug: row.get("slug")?,
        status: row.get("status")?,
        notes: row.get("notes")?,
        tags: serde_json::from_str(&tags_raw).map_err(json_col_err)?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn entity_by_id(conn: &Connection, id: &str) -> Result<Option<Entity>> {
    let mut stmt = conn.prepare("SELECT * FROM entities WHERE id = ?1")?;
    Ok(stmt.query_row(params![id], entity_from_row).optional()?)
}

fn entity_from_row(row: &Row<'_>) -> rusqlite::Result<Entity> {
    let etype_raw: String = row.get("etype")?;
    let data_raw: String = row.get("data")?;
    Ok(Entity {
        id: row.get("id")?,
        case_id: row.get("case_id")?,
        etype: EType::from_str(&etype_raw)
            .ok_or_else(|| rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unknown etype: {etype_raw}"),
                )),
            ))?,
        label: row.get("label")?,
        data: serde_json::from_str(&data_raw).map_err(json_col_err)?,
        confidence: row.get("confidence")?,
        pinned: row.get::<_, i64>("pinned")? != 0,
        first_seen: row.get("first_seen")?,
        last_seen: row.get("last_seen")?,
    })
}

fn relation_from_row(row: &Row<'_>) -> rusqlite::Result<Relation> {
    let evidence_raw: Option<String> = row.get("evidence")?;
    Ok(Relation {
        id: row.get("id")?,
        case_id: row.get("case_id")?,
        src: row.get("src")?,
        dst: row.get("dst")?,
        rel: row.get("rel")?,
        weight: row.get("weight")?,
        confidence: row.get("confidence")?,
        source: row.get("source")?,
        evidence: match evidence_raw {
            Some(s) => Some(serde_json::from_str(&s).map_err(json_col_err)?),
            None => None,
        },
        created_at: row.get("created_at")?,
    })
}

fn evidence_from_row(row: &Row<'_>) -> rusqlite::Result<Evidence> {
    let raw_raw: String = row.get("raw")?;
    Ok(Evidence {
        id: row.get("id")?,
        case_id: row.get("case_id")?,
        subject: row.get("subject")?,
        kind: row.get("kind")?,
        title: row.get("title")?,
        url: row.get("url")?,
        snippet: row.get("snippet")?,
        raw: serde_json::from_str(&raw_raw).map_err(json_col_err)?,
        confidence: row.get("confidence")?,
        ts: row.get("ts")?,
    })
}

fn event_from_row(row: &Row<'_>) -> rusqlite::Result<TimelineEvent> {
    let detail_raw: String = row.get("detail")?;
    Ok(TimelineEvent {
        id: row.get("id")?,
        case_id: row.get("case_id")?,
        ts: row.get("ts")?,
        entity_id: row.get("entity_id")?,
        kind: row.get("kind")?,
        title: row.get("title")?,
        detail: serde_json::from_str(&detail_raw).map_err(json_col_err)?,
        source: row.get("source")?,
    })
}

fn json_col_err(e: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(e),
    )
}

fn set_doc(conn: &Connection, entity_id: &str, markdown: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO docs (entity_id, markdown, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(entity_id) DO UPDATE SET markdown = ?2, updated_at = ?3",
        params![entity_id, markdown, now_iso()],
    )?;
    Ok(())
}

fn add_entity_conn(conn: &Connection, case_id: &str, new: NewEntity) -> Result<AddOutcome> {
    if case_by_id(conn, case_id)?.is_none() {
        return Err(NexusError::NotFound(format!("case {case_id}")));
    }
    let etype_s = new.etype.as_str();
    let norm = normalize(etype_s, &new.label);
    let now = now_iso();

    // dedup lookup
    let mut stmt = conn.prepare(
        "SELECT * FROM entities WHERE case_id = ?1 AND etype = ?2 AND norm_key = ?3",
    )?;
    let existing = stmt
        .query_row(params![case_id, etype_s, norm], entity_from_row)
        .optional()?;
    drop(stmt);

    if let Some(mut ent) = existing {
        // merge: max confidence, refreshed last_seen, data keys merged (new wins)
        let merged_data = merge_data_objects(&ent.data, &new.data);
        if new.confidence > ent.confidence {
            ent.confidence = new.confidence;
        }
        ent.data = merged_data.clone();
        ent.last_seen = now.clone();
        conn.execute(
            "UPDATE entities SET confidence = ?2, data = ?3, last_seen = ?4 WHERE id = ?1",
            params![ent.id, ent.confidence, serde_json::to_string(&merged_data)?, now],
        )?;
        return Ok(AddOutcome { entity: ent, deduplicated: true });
    }

    let id = new_id("e");
    conn.execute(
        "INSERT INTO entities (id, case_id, etype, label, data, confidence, pinned, norm_key, first_seen, last_seen)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            id,
            case_id,
            etype_s,
            new.label,
            serde_json::to_string(&new.data)?,
            new.confidence,
            new.pinned as i64,
            norm,
            now.clone(),
            now
        ],
    )?;
    let entity = entity_by_id(conn, &id)?.ok_or_else(|| NexusError::Invalid("insert vanished".into()))?;
    Ok(AddOutcome { entity, deduplicated: false })
}

/// object(obj, obj) → merged object, keys of `new` winning; non-objects → new.
fn merge_data_objects(old: &Value, new: &Value) -> Value {
    match (old.as_object(), new.as_object()) {
        (Some(o), Some(n)) => {
            let mut out = o.clone();
            for (k, v) in n {
                out.insert(k.clone(), v.clone());
            }
            Value::Object(out)
        }
        _ => {
            if new.as_object().is_some() {
                new.clone()
            } else {
                old.clone()
            }
        }
    }
}

fn entity_relations(conn: &Connection, entity_id: &str) -> Result<Vec<Relation>> {
    let mut stmt = conn.prepare(
        "SELECT * FROM relations WHERE src = ?1 OR dst = ?1 ORDER BY created_at, id",
    )?;
    let rows = stmt
        .query_map(params![entity_id], relation_from_row)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn add_relation_conn(conn: &Connection, new: NewRelation) -> Result<Relation> {
    if new.src == new.dst {
        return Err(NexusError::Invalid(format!(
            "self-relation on {} not allowed",
            new.src
        )));
    }
    let src = entity_by_id(conn, &new.src)?.ok_or_else(|| NexusError::NotFound(format!("entity {}", new.src)))?;
    let dst = entity_by_id(conn, &new.dst)?.ok_or_else(|| NexusError::NotFound(format!("entity {}", new.dst)))?;
    if src.case_id != new.case_id || dst.case_id != new.case_id {
        return Err(NexusError::Invalid(
            "relation endpoints must belong to the relation's case".into(),
        ));
    }

    let mut stmt = conn.prepare(
        "SELECT * FROM relations WHERE src = ?1 AND dst = ?2 AND rel = ?3",
    )?;
    let existing = stmt
        .query_row(params![new.src, new.dst, new.rel], relation_from_row)
        .optional()?;
    drop(stmt);

    if let Some(mut rel) = existing {
        rel.confidence = rel.confidence.max(new.confidence);
        rel.weight = rel.weight.max(new.weight);
        if rel.source.is_none() {
            rel.source = new.source.clone();
        }
        if rel.evidence.is_none() {
            rel.evidence = new.evidence.clone();
        }
        conn.execute(
            "UPDATE relations SET confidence = ?2, weight = ?3, source = ?4, evidence = ?5 WHERE id = ?1",
            params![
                rel.id,
                rel.confidence,
                rel.weight,
                rel.source,
                rel.evidence.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default())
            ],
        )?;
        return Ok(rel);
    }

    let id = new_id("r");
    conn.execute(
        "INSERT INTO relations (id, case_id, src, dst, rel, weight, confidence, source, evidence, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            id,
            new.case_id,
            new.src,
            new.dst,
            new.rel,
            new.weight,
            new.confidence,
            new.source,
            new.evidence.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default()),
            now_iso()
        ],
    )?;
    let mut stmt = conn.prepare("SELECT * FROM relations WHERE id = ?1")?;
    Ok(stmt.query_row(params![id], relation_from_row)?)
}

fn merge_entities_conn(conn: &Connection, primary_id: &str, other_id: &str) -> Result<Entity> {
    if primary_id == other_id {
        return Err(NexusError::Invalid("cannot merge an entity into itself".into()));
    }
    let primary = entity_by_id(conn, primary_id)?
        .ok_or_else(|| NexusError::NotFound(format!("entity {primary_id}")))?;
    let other = entity_by_id(conn, other_id)?
        .ok_or_else(|| NexusError::NotFound(format!("entity {other_id}")))?;
    if primary.case_id != other.case_id {
        return Err(NexusError::Invalid("cannot merge across cases".into()));
    }

    // 1. relations: re-point, collapse duplicates, drop merge-created self-loops
    let rels = entity_relations(conn, &other.id)?;
    for r in rels {
        let new_src = if r.src == other.id { primary.id.clone() } else { r.src.clone() };
        let new_dst = if r.dst == other.id { primary.id.clone() } else { r.dst.clone() };
        if new_src == new_dst {
            conn.execute("DELETE FROM relations WHERE id = ?1", params![r.id])?;
            continue;
        }
        let dup: Option<String> = conn
            .query_row(
                "SELECT id FROM relations WHERE src = ?1 AND dst = ?2 AND rel = ?3 AND id <> ?4",
                params![new_src, new_dst, r.rel, r.id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(dup_id) = dup {
            conn.execute(
                "UPDATE relations SET confidence = MAX(confidence, ?2), weight = MAX(weight, ?3),
                    source = COALESCE(source, ?4) WHERE id = ?1",
                params![dup_id, r.confidence, r.weight, r.source],
            )?;
            conn.execute("DELETE FROM relations WHERE id = ?1", params![r.id])?;
        } else {
            conn.execute(
                "UPDATE relations SET src = ?2, dst = ?3 WHERE id = ?1",
                params![r.id, new_src, new_dst],
            )?;
        }
    }

    // 2. evidence subjects, 3. timeline events
    conn.execute(
        "UPDATE evidence SET subject = ?2 WHERE subject = ?1",
        params![other.id, primary.id],
    )?;
    conn.execute(
        "UPDATE timeline SET entity_id = ?2 WHERE entity_id = ?1",
        params![other.id, primary.id],
    )?;

    // 4. docs: adopt other's doc when primary has none
    let has_doc: Option<String> = conn
        .query_row("SELECT markdown FROM docs WHERE entity_id = ?1", params![primary.id], |r| r.get(0))
        .optional()?;
    if has_doc.is_none() {
        let other_doc: Option<String> = conn
            .query_row("SELECT markdown FROM docs WHERE entity_id = ?1", params![other.id], |r| r.get(0))
            .optional()?;
        if let Some(md) = other_doc {
            set_doc(conn, &primary.id, &md)?;
        }
    }
    conn.execute("DELETE FROM docs WHERE entity_id = ?1", params![other.id])?;

    // 5. audit trail in primary.data.merged_from
    let mut data = primary.data.clone();
    if !data.is_object() {
        data = serde_json::json!({});
    }
    {
        let obj = data.as_object_mut().expect("just made it an object");
        let entry = obj
            .entry("merged_from".to_string())
            .or_insert_with(|| Value::Array(vec![]));
        if !entry.is_array() {
            *entry = Value::Array(vec![]);
        }
        entry.as_array_mut().expect("checked array").push(serde_json::json!({
            "id": other.id,
            "label": other.label,
            "etype": other.etype.as_str(),
            "merged_at": now_iso(),
        }));
    }

    // 6. delete other, 7. update primary
    conn.execute("DELETE FROM entities WHERE id = ?1", params![other.id])?;
    conn.execute(
        "UPDATE entities SET data = ?2, confidence = MAX(confidence, ?3), pinned = MAX(pinned, ?4), last_seen = ?5 WHERE id = ?1",
        params![
            primary.id,
            serde_json::to_string(&data)?,
            other.confidence,
            other.pinned as i64,
            now_iso()
        ],
    )?;

    entity_by_id(conn, &primary.id)?.ok_or_else(|| NexusError::NotFound(format!("entity {}", primary.id)))
}

fn render_doc_conn(conn: &Connection, entity_id: &str) -> Result<String> {
    let entity = entity_by_id(conn, entity_id)?
        .ok_or_else(|| NexusError::NotFound(format!("entity {entity_id}")))?;
    let prior = conn
        .query_row("SELECT markdown FROM docs WHERE entity_id = ?1", params![entity_id], |r| r.get::<_, String>(0))
        .optional()?;

    let summary_prose = prior
        .as_deref()
        .and_then(|md| crate::store::extract_section(md, "Summary"))
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            format!(
                "_Auto-generated skeleton — replace with a human summary of «{}» ({})._",
                entity.label, entity.etype
            )
        });
    let open_questions = prior
        .as_deref()
        .and_then(|md| crate::store::extract_section(md, "Open questions"))
        .unwrap_or_default();

    let mut doc = String::new();
    doc.push_str(&format!("# {}\n\n", entity.label));
    doc.push_str("## Summary\n\n");
    doc.push_str(&summary_prose);
    doc.push_str("\n\n## Known facts\n\n");

    // known facts from entity data (audit arrays skipped)
    let mut facts: Vec<String> = vec![
        format!("- type: {}", entity.etype),
        format!("- confidence: {:.2}", entity.confidence),
    ];
    if entity.pinned {
        facts.push("- pinned: true".to_string());
    }
    if let Some(obj) = entity.data.as_object() {
        let mut keys: Vec<&String> = obj.keys().collect();
        keys.sort();
        for k in keys {
            if k == "merged_from" {
                continue;
            }
            let v = &obj[k];
            let rendered = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
            facts.push(format!("- {k}: {rendered}"));
        }
    }
    doc.push_str(&facts.join("\n"));
    doc.push_str("\n\n## Connections\n\n");

    let rels = entity_relations(conn, entity_id)?;
    if rels.is_empty() {
        doc.push_str("_none yet_\n");
    } else {
        let mut lines: Vec<String> = Vec::new();
        for r in &rels {
            let (arrow, other_id) = if r.src == entity.id {
                ("→", &r.dst)
            } else {
                ("←", &r.src)
            };
            let other = entity_by_id(conn, other_id)?;
            let (label, etype) = other
                .map(|o| (o.label, o.etype.as_str().to_string()))
                .unwrap_or_else(|| (other_id.to_string(), "unknown".to_string()));
            let mut meta = format!("[conf {}", trim_f64(r.confidence));
            if let Some(src) = &r.source {
                meta.push_str(&format!(", source {src}"));
            }
            meta.push(']');
            lines.push(format!("- {} {} {} ({}) {}", r.rel, arrow, label, etype, meta));
        }
        lines.sort();
        doc.push_str(&lines.join("\n"));
        doc.push('\n');
    }

    doc.push_str("\n## Timeline\n\n");
    let mut stmt = conn.prepare(
        "SELECT * FROM timeline WHERE entity_id = ?1 ORDER BY ts, id",
    )?;
    let events = stmt
        .query_map(params![entity_id], event_from_row)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(stmt);
    if events.is_empty() {
        doc.push_str("_none yet_\n");
    } else {
        let lines: Vec<String> = events
            .iter()
            .map(|e| format!("- {} — {}: {}", e.ts, e.kind, e.title))
            .collect();
        doc.push_str(&lines.join("\n"));
        doc.push('\n');
    }

    doc.push_str("\n## Open questions\n\n");
    if !open_questions.trim().is_empty() {
        doc.push_str(open_questions.trim());
        doc.push('\n');
    }

    doc.push_str("\n## Sources\n\n");
    let mut stmt = conn.prepare("SELECT * FROM evidence WHERE subject = ?1 ORDER BY ts, id")?;
    let evidence = stmt
        .query_map(params![entity_id], evidence_from_row)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if evidence.is_empty() {
        doc.push_str("_none yet_\n");
    } else {
        let lines: Vec<String> = evidence
            .iter()
            .map(source_line)
            .collect();
        doc.push_str(&lines.join("\n"));
        doc.push('\n');
    }

    set_doc(conn, entity_id, &doc)?;
    Ok(doc)
}

/// One markdown bullet for a piece of evidence (used by docs + dossier).
pub(crate) fn source_line(ev: &Evidence) -> String {
    let title = ev.title.clone().unwrap_or_else(|| ev.kind.clone());
    match &ev.url {
        Some(url) => {
            let mut line = format!("- [{title}]({url})");
            if let Some(snippet) = &ev.snippet {
                line.push_str(&format!(" — {snippet}"));
            }
            line
        }
        None => {
            let mut line = format!("- {}: {}", ev.kind, title);
            if let Some(snippet) = &ev.snippet {
                line.push_str(&format!(" — {snippet}"));
            }
            line
        }
    }
}

fn trim_f64(v: f64) -> String {
    format!("{v:.2}")
}

pub(crate) fn slugify(name: &str) -> String {
    let mut s = String::new();
    let mut prev_dash = true;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            s.extend(c.to_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            s.push('-');
            prev_dash = true;
        }
    }
    while s.ends_with('-') {
        s.pop();
    }
    if s.is_empty() { "case".to_string() } else { s }
}

/// Extracts the prose of an H2 section (verbatim lines, outer blanks trimmed).
/// Returns `Some("")` when the heading exists but is empty, `None` when absent.
pub fn extract_section(markdown: &str, heading: &str) -> Option<String> {
    let heading_line = format!("## {heading}");
    let mut lines: Vec<&str> = Vec::new();
    let mut capture = false;
    for line in markdown.lines() {
        if line.trim_end() == heading_line {
            capture = true;
            continue;
        }
        if capture {
            if line.starts_with("## ") || line.starts_with("# ") {
                break;
            }
            lines.push(line);
        }
    }
    if !capture {
        return None;
    }
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    Some(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NewEntity;

    fn mem() -> Store {
        Store::open_memory().unwrap()
    }

    #[test]
    fn case_crud_round_trip() {
        let store = mem();
        let case = store.create_case("Case 41 — subject X", "notes here", &["osint".into()]).unwrap();
        assert!(case.id.starts_with("c_"));
        assert_eq!(case.slug, "case-41-subject-x");
        assert_eq!(case.status, "active");
        assert_eq!(store.list_cases().unwrap().len(), 1);
        assert_eq!(store.get_case(&case.id).unwrap().unwrap().notes, "notes here");

        store.update_case(&case.id, Some("closed"), None).unwrap();
        let c = store.get_case(&case.id).unwrap().unwrap();
        assert_eq!(c.status, "closed");
        assert_eq!(c.notes, "notes here");

        // duplicate slug gets suffix
        let c2 = store.create_case("Case 41 — subject X!", "", &[]).unwrap();
        assert_eq!(c2.slug, "case-41-subject-x-2");

        store.delete_case(&case.id).unwrap();
        assert!(store.get_case(&case.id).unwrap().is_none());
        assert!(store.delete_case(&case.id).is_err()); // NotFound
    }

    #[test]
    fn entity_add_dedups_by_normalized_key() {
        let store = mem();
        let case = store.create_case("t", "", &[]).unwrap();
        let cid = case.id.as_str();

        let a = store.add_entity(cid, NewEntity::new(EType::Email, "Bob@Example.COM")).unwrap();
        assert!(!a.deduplicated);
        let b = store.add_entity(cid, NewEntity::new(EType::Email, "bob@example.com")).unwrap();
        assert!(b.deduplicated);
        assert_eq!(a.entity.id, b.entity.id);
        assert_eq!(b.entity.label, "Bob@Example.COM"); // first label wins

        let d1 = store.add_entity(cid, NewEntity::new(EType::Domain, "www.EvilCorp.io")).unwrap();
        let d2 = store.add_entity(cid, NewEntity::new(EType::Domain, "evilcorp.io")).unwrap();
        assert!(d2.deduplicated);
        assert_eq!(d1.entity.id, d2.entity.id);

        // data merge on dedup (new keys win)
        let with_data = NewEntity::new(EType::Username, "johndoe").with_data(serde_json::json!({"platform":"github"}));
        let u1 = store.add_entity(cid, with_data).unwrap();
        let u2 = store.add_entity(
            cid,
            NewEntity::new(EType::Username, "JohnDoe").with_data(serde_json::json!({"platform":"twitter","url":"https://x.com/JohnDoe"})),
        )
        .unwrap();
        assert!(u2.deduplicated);
        let merged = store.get_entity(&u1.entity.id).unwrap().unwrap();
        assert_eq!(merged.data["platform"], "twitter");
        assert_eq!(merged.data["url"], "https://x.com/JohnDoe");

        // find_entity uses the same normalization
        assert!(store.find_entity(cid, "email", "BOB@EXAMPLE.com").unwrap().is_some());
        assert!(store.find_entity(cid, "domain", "www.evilcorp.io").unwrap().is_some());

        // distinct etype same label → distinct entity
        let same_label_other_type = store.add_entity(cid, NewEntity::new(EType::Person, "johndoe")).unwrap();
        assert!(!same_label_other_type.deduplicated);
    }

    #[test]
    fn entity_update_find_delete() {
        let store = mem();
        let case = store.create_case("t", "", &[]).unwrap();
        let e = store
            .add_entity(&case.id, NewEntity::new(EType::Person, "alice").with_confidence(0.5))
            .unwrap()
            .entity;

        let patched = store
            .update_entity(
                &e.id,
                EntityPatch {
                    pinned: Some(true),
                    confidence: Some(0.95),
                    data: Some(serde_json::json!({"role":"target"})),
                    label: Some("Alice A.".into()),
                },
            )
            .unwrap();
        assert!(patched.pinned);
        assert_eq!(patched.confidence, 0.95);
        assert_eq!(patched.label, "Alice A.");
        assert_eq!(patched.data["role"], "target");

        store.delete_entity(&e.id).unwrap();
        assert!(store.get_entity(&e.id).unwrap().is_none());
        assert!(store.delete_entity(&e.id).is_err());
    }

    #[test]
    fn relation_dedup_and_validation() {
        let store = mem();
        let case = store.create_case("t", "", &[]).unwrap();
        let a = store.add_entity(&case.id, NewEntity::new(EType::Person, "alice")).unwrap().entity;
        let b = store.add_entity(&case.id, NewEntity::new(EType::Username, "alicesmith")).unwrap().entity;

        let r1 = store
            .add_relation(NewRelation::new(&case.id, &a.id, &b.id, "uses").with_confidence(0.7))
            .unwrap();
        let r2 = store
            .add_relation(NewRelation::new(&case.id, &a.id, &b.id, "uses").with_confidence(0.9))
            .unwrap();
        assert_eq!(r1.id, r2.id, "same (src,dst,rel) returns the existing row");
        assert_eq!(r2.confidence, 0.9, "confidence raised to max");

        let r3 = store.add_relation(NewRelation::new(&case.id, &a.id, &b.id, "owns")).unwrap();
        assert_ne!(r1.id, r3.id);
        let rev = store.add_relation(NewRelation::new(&case.id, &b.id, &a.id, "uses")).unwrap();
        assert_ne!(r1.id, rev.id, "direction matters for dedup");

        assert_eq!(store.list_relations(&case.id).unwrap().len(), 3);
        // entity incident listing
        assert_eq!(store.list_entity_relations(&a.id).unwrap().len(), 3);

        // validation
        assert!(store.add_relation(NewRelation::new(&case.id, &a.id, &a.id, "uses")).is_err());
        assert!(store
            .add_relation(NewRelation::new(&case.id, "e_missing", &b.id, "uses"))
            .is_err());

        store.delete_relation(&rev.id).unwrap();
        assert_eq!(store.list_relations(&case.id).unwrap().len(), 2);
        assert!(store.delete_relation(&rev.id).is_err());
    }

    #[test]
    fn merge_repoints_relations_evidence_and_audits() {
        let store = mem();
        let case = store.create_case("t", "", &[]).unwrap();
        let a = store.add_entity(&case.id, NewEntity::new(EType::Person, "alice")).unwrap().entity;
        let b = store.add_entity(&case.id, NewEntity::new(EType::Person, "bob")).unwrap().entity;
        let c = store.add_entity(&case.id, NewEntity::new(EType::Username, "carol")).unwrap().entity;

        let r_uses = store.add_relation(NewRelation::new(&case.id, &a.id, &c.id, "uses")).unwrap();
        let r_mention = store
            .add_relation(NewRelation::new(&case.id, &b.id, &c.id, "mentions").with_confidence(0.6))
            .unwrap();
        // a mentions b → would become a self-loop after merge → dropped
        store.add_relation(NewRelation::new(&case.id, &a.id, &b.id, "mentions")).unwrap();
        // duplicate after re-point: b owns c while a owns c exists → collapsed
        store.add_relation(NewRelation::new(&case.id, &a.id, &c.id, "owns")).unwrap();
        store.add_relation(NewRelation::new(&case.id, &b.id, &c.id, "owns")).unwrap();

        store
            .add_evidence(NewEvidence::new(&case.id, &b.id, "web_fetch"))
            .unwrap();
        store
            .add_timeline(NewEvent::new(&case.id, "scan", "merged hit").with_entity(&b.id))
            .unwrap();

        let merged = store.merge_entities(&a.id, &b.id).unwrap();
        assert!(store.get_entity(&b.id).unwrap().is_none(), "other deleted");
        let audit = merged.data["merged_from"].as_array().unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0]["label"], "bob");
        assert_eq!(audit[0]["id"], b.id);

        let rels = store.list_relations(&case.id).unwrap();
        assert_eq!(rels.len(), 3, "a→c uses, b→c mentions re-pointed, dup owns collapsed (self-loop dropped): {rels:?}");
        assert!(rels.iter().any(|r| r.id == r_uses.id));
        assert!(rels.iter().all(|r| r.src != b.id && r.dst != b.id));

        // evidence + timeline re-pointed
        assert_eq!(store.list_evidence(&a.id).unwrap().len(), 1);
        let tl = store.list_timeline(&case.id).unwrap();
        assert_eq!(tl[0].entity_id.as_deref(), Some(a.id.as_str()));

        // merging into itself is invalid
        assert!(store.merge_entities(&a.id, &a.id).is_err());
        // r_mention was re-pointed onto a→c mentions
        assert!(store
            .list_relations(&case.id)
            .unwrap()
            .iter()
            .any(|r| r.id == r_mention.id && r.src == a.id && r.dst == c.id));
    }

    #[test]
    fn evidence_and_timeline_round_trip() {
        let store = mem();
        let case = store.create_case("t", "", &[]).unwrap();
        let e = store.add_entity(&case.id, NewEntity::new(EType::Email, "x@y.io")).unwrap().entity;

        let ev = store
            .add_evidence(
                NewEvidence::new(&case.id, &e.id, "tool_result")
                    .with_title("holehe hit")
                    .with_url("https://y.io")
                    .with_snippet("registered"),
            )
            .unwrap();
        assert!(ev.id.starts_with("v_"));
        assert_eq!(store.list_evidence(&e.id).unwrap().len(), 1);
        assert_eq!(store.list_case_evidence(&case.id).unwrap().len(), 1);

        let k1 = store.add_timeline(NewEvent::new(&case.id, "scan", "first")).unwrap();
        let k2 = store
            .add_timeline(NewEvent::new(&case.id, "scan", "second").with_ts("2020-01-01T00:00:00Z"))
            .unwrap();
        let tl = store.list_timeline(&case.id).unwrap();
        assert_eq!(tl.len(), 2);
        assert_eq!(tl[0].id, k2.id, "asc by ts");
        assert_eq!(tl[1].id, k1.id);
        assert!(k1.id.starts_with("k_"));
    }

    #[test]
    fn delete_case_cascades() {
        let store = mem();
        let case = store.create_case("t", "", &[]).unwrap();
        let e = store.add_entity(&case.id, NewEntity::new(EType::Ip, "1.2.3.4")).unwrap().entity;
        let e2 = store.add_entity(&case.id, NewEntity::new(EType::Domain, "x.io")).unwrap().entity;
        store.add_relation(NewRelation::new(&case.id, &e.id, &e2.id, "resolves_to")).unwrap();
        store.add_evidence(NewEvidence::new(&case.id, &e.id, "note")).unwrap();
        store.add_timeline(NewEvent::new(&case.id, "ingest", "boot")).unwrap();
        store.set_doc(&e.id, "# x\n\n## Summary\nhi").unwrap();

        store.delete_case(&case.id).unwrap();
        assert!(store.list_entities(&case.id).unwrap().is_empty());
        assert!(store.list_relations(&case.id).unwrap().is_empty());
        assert!(store.list_case_evidence(&case.id).unwrap().is_empty());
        assert!(store.list_timeline(&case.id).unwrap().is_empty());
        assert!(store.get_doc(&e.id).unwrap().is_none());
    }

    #[test]
    fn doc_render_preserves_prose_and_refreshes_sections() {
        let store = mem();
        let case = store.create_case("t", "", &[]).unwrap();
        let alice = store.add_entity(&case.id, NewEntity::new(EType::Person, "alice").with_confidence(0.9)).unwrap().entity;
        let acct = store
            .add_entity(&case.id, NewEntity::new(EType::Username, "johndoe").with_data(serde_json::json!({"platform":"github"})))
            .unwrap()
            .entity;
        store
            .add_relation(
                NewRelation::new(&case.id, &alice.id, &acct.id, "uses")
                    .with_confidence(0.9)
                    .with_source("hub:sherlock"),
            )
            .unwrap();
        store.add_timeline(NewEvent::new(&case.id, "scan", "sherlock hit").with_entity(&alice.id)).unwrap();
        store.add_evidence(NewEvidence::new(&case.id, &alice.id, "web_fetch").with_title("profile").with_url("https://github.com/johndoe")).unwrap();

        store
            .set_doc(
                &alice.id,
                "# alice\n\n## Summary\n\nAlice is a person of interest since 2019.\nLives in Berlin.\n\n## Known facts\n\n- stale: yes\n\n## Open questions\n\nWhere was she on the 4th?\nWho is johndoe really?\n",
            )
            .unwrap();

        let doc = store.render_doc(&alice.id).unwrap();
        assert!(doc.contains("Alice is a person of interest since 2019."), "{doc}");
        assert!(doc.contains("Lives in Berlin."), "{doc}");
        assert!(doc.contains("Where was she on the 4th?"), "{doc}");
        assert!(doc.contains("Who is johndoe really?"), "{doc}");
        assert!(doc.contains("## Connections"), "{doc}");
        assert!(doc.contains("- uses → johndoe (username) [conf 0.90, source hub:sherlock]"), "{doc}");
        assert!(doc.contains("## Timeline"), "{doc}");
        assert!(doc.contains("sherlock hit"), "{doc}");
        assert!(doc.contains("[profile](https://github.com/johndoe)"), "{doc}");
        assert!(!doc.contains("- stale: yes"), "known facts must regenerate: {doc}");
        assert!(doc.contains("- type: person"), "{doc}");
        // persisted by render
        assert_eq!(store.get_doc(&alice.id).unwrap().as_deref(), Some(doc.as_str()));

        // render with no prior doc seeds a placeholder summary
        let doc2 = store.render_doc(&acct.id).unwrap();
        assert!(doc2.contains("_Auto-generated skeleton"), "{doc2}");
        // incoming relation renders with ← arrow
        assert!(doc2.contains("← alice (person)"), "{doc2}");
    }

    #[test]
    fn extract_section_semantics() {
        let md = "# t\n\n## Summary\n\nline one\nline two\n\n## Sources\n\n- x\n";
        assert_eq!(extract_section(md, "Summary").unwrap(), "line one\nline two");
        assert_eq!(extract_section(md, "Sources").unwrap(), "- x");
        assert_eq!(extract_section(md, "Missing"), None);
        assert_eq!(extract_section("## Open questions\n\n", "Open questions").unwrap(), "");
    }
}
