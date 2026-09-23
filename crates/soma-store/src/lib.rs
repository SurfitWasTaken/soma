//! SQLite persistence for a Soma workspace (PRD §5.7, §7.3).
//!
//! One file per workspace, WAL mode, every [`Tx`] in one SQL transaction.
//! Undo/redo is a persisted log of forward/inverse transactions, so history
//! survives restart (F-DATA-3) and a `SIGKILL` loses at most the in-flight
//! transaction (F-DATA-4).

mod schema;
pub mod worker;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use soma_core::*;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Undo history retained on disk (F-DATA-3 requires ≥ 200).
pub const UNDO_DEPTH: i64 = 1000;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("workspace is inconsistent: {0}")]
    Core(#[from] CoreError),
    #[error("workspace schema {found} is newer than this build ({supported})")]
    TooNew { found: i64, supported: i64 },
}

pub type Result<T> = std::result::Result<T, StoreError>;

pub struct Store {
    conn: Connection,
    path: PathBuf,
}

/// One entry of the undo history, for display.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub seq: i64,
    pub at: Timestamp,
    pub label: String,
    pub undone: bool,
}

impl Store {
    /// Open (or create and seed) a workspace file.
    pub fn open(path: impl AsRef<Path>) -> Result<Store> {
        let path = path.as_ref().to_path_buf();
        let conn = Connection::open(&path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let mut store = Store { conn, path };
        let fresh = schema::migrate(&store.conn)?;
        if fresh {
            store.apply_unlogged(&defaults::seed())?;
        }
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Store> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let mut store = Store { conn, path: PathBuf::from(":memory:") };
        schema::migrate(&store.conn)?;
        store.apply_unlogged(&defaults::seed())?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the whole workspace into memory.
    pub fn load(&self) -> Result<Graph> {
        let c = &self.conn;
        let documents = c
            .prepare("SELECT id, path, title, page_count, copied_local, added_at FROM document ORDER BY id")?
            .query_map([], |r| {
                Ok(Document {
                    id: DocumentId(r.get(0)?),
                    path: r.get(1)?,
                    title: r.get(2)?,
                    page_count: r.get(3)?,
                    copied_local: r.get(4)?,
                    added_at: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let kinds = c
            .prepare(
                "SELECT id, name, directed, acyclic, stroke, color, joins_confusion FROM relation_kind ORDER BY id",
            )?
            .query_map([], |r| {
                Ok(RelationKind {
                    id: KindId(r.get(0)?),
                    name: r.get(1)?,
                    directed: r.get(2)?,
                    acyclic: r.get(3)?,
                    stroke: Stroke::parse(&r.get::<_, String>(4)?).unwrap_or(Stroke::Solid),
                    color: Color(r.get(5)?),
                    joins_confusion: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let systems = c
            .prepare(
                "SELECT id, name, description, color, default_relation_kinds, default_layout, hotkey, confusion, inbox
                 FROM system ORDER BY id",
            )?
            .query_map([], |r| {
                let kinds: String = r.get(4)?;
                Ok(System {
                    id: SystemId(r.get(0)?),
                    name: r.get(1)?,
                    description: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    color: Color(r.get(3)?),
                    default_relation_kinds: serde_json::from_str(&kinds).unwrap_or_default(),
                    default_layout: LayoutKind::parse(&r.get::<_, String>(5)?).unwrap_or(LayoutKind::Force),
                    hotkey: r.get(6)?,
                    confusion: r.get(7)?,
                    inbox: r.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let nodes = c
            .prepare(
                "SELECT n.entity_id, n.title, n.body, n.abstraction_level, n.color_override, e.created_at, e.updated_at
                 FROM node n JOIN entity e ON e.id = n.entity_id ORDER BY n.entity_id",
            )?
            .query_map([], |r| {
                Ok(Node {
                    id: EntityId(r.get(0)?),
                    title: r.get(1)?,
                    body: r.get(2)?,
                    abstraction_level: r.get(3)?,
                    color_override: r.get::<_, Option<u32>>(4)?.map(Color),
                    created_at: r.get(5)?,
                    updated_at: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut endpoints_stmt = c.prepare(
            "SELECT entity_id, role, ordinal FROM relation_endpoint WHERE relation_id = ?1 ORDER BY ordinal, role",
        )?;
        let mut relations = c
            .prepare(
                "SELECT r.entity_id, r.kind_id, r.friction, r.title, r.body, e.created_at, e.updated_at
                 FROM relation r JOIN entity e ON e.id = r.entity_id ORDER BY r.entity_id",
            )?
            .query_map([], |r| {
                Ok(Relation {
                    id: EntityId(r.get(0)?),
                    kind: KindId(r.get(1)?),
                    endpoints: vec![],
                    friction: r.get(2)?,
                    title: r.get(3)?,
                    body: r.get(4)?,
                    created_at: r.get(5)?,
                    updated_at: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for rel in &mut relations {
            rel.endpoints = endpoints_stmt
                .query_map([rel.id.as_str()], |r| {
                    Ok(Endpoint {
                        entity: EntityId(r.get(0)?),
                        role: Role::parse(&r.get::<_, String>(1)?).unwrap_or(Role::Member),
                        ordinal: r.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
        }
        let memberships = c
            .prepare("SELECT system_id, entity_id, added_at FROM membership ORDER BY system_id, entity_id")?
            .query_map([], |r| {
                Ok(Membership {
                    system: SystemId(r.get(0)?),
                    entity: EntityId(r.get(1)?),
                    added_at: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let anchors = c
            .prepare(
                "SELECT id, entity_id, document_id, page_index, kind, quads, exact, prefix, suffix,
                        char_start, char_end, confidence, color FROM anchor ORDER BY id",
            )?
            .query_map([], |r| {
                Ok(Anchor {
                    id: AnchorId(r.get(0)?),
                    entity: EntityId(r.get(1)?),
                    document: DocumentId(r.get(2)?),
                    page_index: r.get(3)?,
                    kind: AnchorKind::parse(&r.get::<_, String>(4)?).unwrap_or(AnchorKind::Region),
                    quads: Quad::decode(&r.get::<_, Vec<u8>>(5)?),
                    exact: r.get(6)?,
                    prefix: r.get(7)?,
                    suffix: r.get(8)?,
                    char_start: r.get(9)?,
                    char_end: r.get(10)?,
                    confidence: r.get(11)?,
                    color: r.get::<_, Option<u32>>(12)?.map(Color),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(endpoints_stmt);
        let snapshot = Snapshot {
            format: "soma".into(),
            schema_version: snapshot::SCHEMA_VERSION,
            documents,
            kinds,
            systems,
            nodes,
            relations,
            memberships,
            anchors,
        };
        Ok(Graph::from_snapshot(&snapshot)?)
    }

    /// Persist a user action and record it for undo. Clears the redo tail.
    pub fn commit(&mut self, tx: &Tx) -> Result<()> {
        if tx.is_empty() {
            return Ok(());
        }
        let t = self.conn.transaction()?;
        apply_ops(&t, tx)?;
        t.execute("DELETE FROM undo_log WHERE undone = 1", [])?;
        t.execute(
            "INSERT INTO undo_log (at, label, forward, inverse, undone) VALUES (?1, ?2, ?3, ?4, 0)",
            params![now_ms(), tx.label, serde_json::to_string(tx)?, serde_json::to_string(&tx.inverse())?],
        )?;
        t.execute("DELETE FROM undo_log WHERE seq <= (SELECT MAX(seq) FROM undo_log) - ?1", [UNDO_DEPTH])?;
        t.commit()?;
        Ok(())
    }

    /// Persist without touching the undo log (seeding, imports).
    pub fn apply_unlogged(&mut self, tx: &Tx) -> Result<()> {
        let t = self.conn.transaction()?;
        apply_ops(&t, tx)?;
        t.commit()?;
        Ok(())
    }

    /// Undo the latest action. Returns the transaction that was applied to
    /// the database so the caller can apply it to its in-memory graph.
    pub fn undo(&mut self) -> Result<Option<Tx>> {
        self.step("SELECT seq, inverse FROM undo_log WHERE undone = 0 ORDER BY seq DESC LIMIT 1", 1)
    }

    pub fn redo(&mut self) -> Result<Option<Tx>> {
        self.step("SELECT seq, forward FROM undo_log WHERE undone = 1 ORDER BY seq ASC LIMIT 1", 0)
    }

    fn step(&mut self, select: &str, mark: i64) -> Result<Option<Tx>> {
        let t = self.conn.transaction()?;
        let Some((seq, json)) =
            t.query_row(select, [], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))).optional()?
        else {
            return Ok(None);
        };
        let tx: Tx = serde_json::from_str(&json)?;
        apply_ops(&t, &tx)?;
        t.execute("UPDATE undo_log SET undone = ?1 WHERE seq = ?2", params![mark, seq])?;
        t.commit()?;
        Ok(Some(tx))
    }

    pub fn history(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
        Ok(self
            .conn
            .prepare("SELECT seq, at, label, undone FROM undo_log ORDER BY seq DESC LIMIT ?1")?
            .query_map([limit as i64], |r| {
                Ok(HistoryEntry { seq: r.get(0)?, at: r.get(1)?, label: r.get(2)?, undone: r.get(3)? })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Full-text search over entity titles and bodies (FTS5 query syntax;
    /// plain words are prefix-matched).
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<EntityId>> {
        let q: String = query
            .split_whitespace()
            .map(|w| format!("\"{}\"*", w.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" ");
        if q.is_empty() {
            return Ok(vec![]);
        }
        Ok(self
            .conn
            .prepare("SELECT entity_id FROM entity_fts WHERE entity_fts MATCH ?1 ORDER BY rank LIMIT ?2")?
            .query_map(params![q, limit as i64], |r| Ok(EntityId(r.get(0)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// `PRAGMA integrity_check` + foreign key check.
    pub fn integrity_check(&self) -> Result<Vec<String>> {
        let mut out: Vec<String> = self
            .conn
            .prepare("PRAGMA integrity_check")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        out.retain(|s| s != "ok");
        let fk: Vec<String> = self
            .conn
            .prepare("PRAGMA foreign_key_check")?
            .query_map([], |r| Ok(format!("fk violation in {}", r.get::<_, String>(0)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        out.extend(fk);
        Ok(out)
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}

fn fts_put(t: &Transaction, id: &EntityId, title: &str, body: &str) -> rusqlite::Result<()> {
    t.execute("DELETE FROM entity_fts WHERE entity_id = ?1", [id.as_str()])?;
    if !title.is_empty() || !body.is_empty() {
        t.execute(
            "INSERT INTO entity_fts (entity_id, title, body) VALUES (?1, ?2, ?3)",
            params![id.as_str(), title, body],
        )?;
    }
    Ok(())
}

fn fts_delete(t: &Transaction, id: &EntityId) -> rusqlite::Result<()> {
    t.execute("DELETE FROM entity_fts WHERE entity_id = ?1", [id.as_str()])?;
    Ok(())
}

fn put_endpoints(t: &Transaction, r: &Relation) -> rusqlite::Result<()> {
    t.execute("DELETE FROM relation_endpoint WHERE relation_id = ?1", [r.id.as_str()])?;
    for e in &r.endpoints {
        t.execute(
            "INSERT INTO relation_endpoint (relation_id, entity_id, role, ordinal) VALUES (?1, ?2, ?3, ?4)",
            params![r.id.as_str(), e.entity.as_str(), e.role.as_str(), e.ordinal],
        )?;
    }
    Ok(())
}

fn put_system(t: &Transaction, s: &System, insert: bool) -> Result<()> {
    let sql = if insert {
        "INSERT INTO system (id, name, description, color, hotkey, default_layout, default_relation_kinds, confusion, inbox)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
    } else {
        "UPDATE system SET name = ?2, description = ?3, color = ?4, hotkey = ?5, default_layout = ?6,
         default_relation_kinds = ?7, confusion = ?8, inbox = ?9 WHERE id = ?1"
    };
    t.execute(
        sql,
        params![
            s.id.as_str(),
            s.name,
            s.description,
            s.color.0,
            s.hotkey,
            s.default_layout.as_str(),
            serde_json::to_string(&s.default_relation_kinds)?,
            s.confusion,
            s.inbox
        ],
    )?;
    Ok(())
}

fn put_kind(t: &Transaction, k: &RelationKind, insert: bool) -> rusqlite::Result<()> {
    let sql = if insert {
        "INSERT INTO relation_kind (id, name, directed, acyclic, stroke, color, joins_confusion)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
    } else {
        "UPDATE relation_kind SET name = ?2, directed = ?3, acyclic = ?4, stroke = ?5, color = ?6,
         joins_confusion = ?7 WHERE id = ?1"
    };
    t.execute(
        sql,
        params![
            k.id.as_str(),
            k.name,
            k.directed,
            k.acyclic,
            k.stroke.as_str(),
            k.color.0,
            k.joins_confusion
        ],
    )?;
    Ok(())
}

fn put_anchor(t: &Transaction, a: &Anchor, insert: bool) -> rusqlite::Result<()> {
    let sql = if insert {
        "INSERT INTO anchor (id, entity_id, document_id, page_index, kind, quads, exact, prefix, suffix,
                             char_start, char_end, confidence, color)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"
    } else {
        "UPDATE anchor SET entity_id = ?2, document_id = ?3, page_index = ?4, kind = ?5, quads = ?6, exact = ?7,
         prefix = ?8, suffix = ?9, char_start = ?10, char_end = ?11, confidence = ?12, color = ?13 WHERE id = ?1"
    };
    t.execute(
        sql,
        params![
            a.id.as_str(),
            a.entity.as_str(),
            a.document.as_str(),
            a.page_index,
            a.kind.as_str(),
            Quad::encode(&a.quads),
            a.exact,
            a.prefix,
            a.suffix,
            a.char_start,
            a.char_end,
            a.confidence,
            a.color.map(|c| c.0)
        ],
    )?;
    Ok(())
}

fn put_document(t: &Transaction, d: &Document, insert: bool) -> rusqlite::Result<()> {
    let sql = if insert {
        "INSERT INTO document (id, path, title, page_count, copied_local, added_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
    } else {
        "UPDATE document SET path = ?2, title = ?3, page_count = ?4, copied_local = ?5, added_at = ?6 WHERE id = ?1"
    };
    t.execute(sql, params![d.id.as_str(), d.path, d.title, d.page_count, d.copied_local, d.added_at])?;
    Ok(())
}

/// Translate each op into SQL. Ops arrive already validated by
/// [`Graph::apply`]; foreign keys and CHECKs are a second line of defence.
fn apply_ops(t: &Transaction, tx: &Tx) -> Result<()> {
    for op in &tx.ops {
        match op {
            Op::AddDocument(d) => put_document(t, d, true)?,
            Op::UpdateDocument { after, .. } => put_document(t, after, false)?,
            Op::RemoveDocument(d) => {
                t.execute("DELETE FROM document WHERE id = ?1", [d.id.as_str()])?;
            }

            Op::AddKind(k) => put_kind(t, k, true)?,
            Op::UpdateKind { after, .. } => put_kind(t, after, false)?,
            Op::RemoveKind(k) => {
                t.execute("DELETE FROM relation_kind WHERE id = ?1", [k.id.as_str()])?;
            }

            Op::AddSystem(s) => put_system(t, s, true)?,
            Op::UpdateSystem { after, .. } => put_system(t, after, false)?,
            Op::RemoveSystem(s) => {
                t.execute("DELETE FROM system WHERE id = ?1", [s.id.as_str()])?;
            }

            Op::AddNode(n) => {
                t.execute(
                    "INSERT INTO entity (id, kind, created_at, updated_at) VALUES (?1, 'node', ?2, ?3)",
                    params![n.id.as_str(), n.created_at, n.updated_at],
                )?;
                t.execute(
                    "INSERT INTO node (entity_id, title, body, abstraction_level, color_override)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        n.id.as_str(),
                        n.title,
                        n.body,
                        n.abstraction_level,
                        n.color_override.map(|c| c.0)
                    ],
                )?;
                fts_put(t, &n.id, &n.title, &n.body)?;
            }
            Op::UpdateNode { after: n, .. } => {
                t.execute(
                    "UPDATE node SET title = ?2, body = ?3, abstraction_level = ?4, color_override = ?5
                     WHERE entity_id = ?1",
                    params![
                        n.id.as_str(),
                        n.title,
                        n.body,
                        n.abstraction_level,
                        n.color_override.map(|c| c.0)
                    ],
                )?;
                t.execute(
                    "UPDATE entity SET created_at = ?2, updated_at = ?3 WHERE id = ?1",
                    params![n.id.as_str(), n.created_at, n.updated_at],
                )?;
                fts_put(t, &n.id, &n.title, &n.body)?;
            }
            Op::RemoveNode(n) => {
                t.execute("DELETE FROM entity WHERE id = ?1", [n.id.as_str()])?;
                fts_delete(t, &n.id)?;
            }

            Op::AddRelation(r) => {
                t.execute(
                    "INSERT INTO entity (id, kind, created_at, updated_at) VALUES (?1, 'relation', ?2, ?3)",
                    params![r.id.as_str(), r.created_at, r.updated_at],
                )?;
                t.execute(
                    "INSERT INTO relation (entity_id, kind_id, friction, title, body) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![r.id.as_str(), r.kind.as_str(), r.friction, r.title, r.body],
                )?;
                put_endpoints(t, r)?;
                fts_put(t, &r.id, r.title.as_deref().unwrap_or(""), r.body.as_deref().unwrap_or(""))?;
            }
            Op::UpdateRelation { after: r, .. } => {
                t.execute(
                    "UPDATE relation SET kind_id = ?2, friction = ?3, title = ?4, body = ?5 WHERE entity_id = ?1",
                    params![r.id.as_str(), r.kind.as_str(), r.friction, r.title, r.body],
                )?;
                t.execute(
                    "UPDATE entity SET created_at = ?2, updated_at = ?3 WHERE id = ?1",
                    params![r.id.as_str(), r.created_at, r.updated_at],
                )?;
                put_endpoints(t, r)?;
                fts_put(t, &r.id, r.title.as_deref().unwrap_or(""), r.body.as_deref().unwrap_or(""))?;
            }
            Op::RemoveRelation(r) => {
                t.execute("DELETE FROM entity WHERE id = ?1", [r.id.as_str()])?;
                fts_delete(t, &r.id)?;
            }

            Op::AddMembership(m) => {
                t.execute(
                    "INSERT INTO membership (system_id, entity_id, added_at) VALUES (?1, ?2, ?3)",
                    params![m.system.as_str(), m.entity.as_str(), m.added_at],
                )?;
            }
            Op::RemoveMembership(m) => {
                t.execute(
                    "DELETE FROM membership WHERE system_id = ?1 AND entity_id = ?2",
                    params![m.system.as_str(), m.entity.as_str()],
                )?;
            }

            Op::AddAnchor(a) => put_anchor(t, a, true)?,
            Op::UpdateAnchor { after, .. } => put_anchor(t, after, false)?,
            Op::RemoveAnchor(a) => {
                t.execute("DELETE FROM anchor WHERE id = ?1", [a.id.as_str()])?;
            }
        }
    }
    Ok(())
}
