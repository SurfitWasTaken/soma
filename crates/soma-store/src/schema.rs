//! Schema (PRD §7.3) and forward-only migrations keyed on `PRAGMA user_version`.

use crate::{Result, StoreError};
use rusqlite::Connection;

const V1: &str = r#"
CREATE TABLE document (
  id            TEXT PRIMARY KEY,          -- blake3 of file bytes
  path          TEXT NOT NULL,
  title         TEXT,
  page_count    INTEGER NOT NULL,
  copied_local  INTEGER NOT NULL DEFAULT 0,
  added_at      INTEGER NOT NULL
);

-- Every addressable thing in the graph.
CREATE TABLE entity (
  id         TEXT PRIMARY KEY,
  kind       TEXT NOT NULL CHECK (kind IN ('node','relation')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE node (
  entity_id         TEXT PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  title             TEXT NOT NULL,
  body              TEXT NOT NULL DEFAULT '',
  abstraction_level INTEGER,
  color_override    INTEGER
);

CREATE TABLE relation_kind (
  id              TEXT PRIMARY KEY,
  name            TEXT NOT NULL,
  directed        INTEGER NOT NULL DEFAULT 1,
  acyclic         INTEGER NOT NULL DEFAULT 0,
  stroke          TEXT NOT NULL,
  color           INTEGER NOT NULL,
  joins_confusion INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE relation (
  entity_id TEXT PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  kind_id   TEXT NOT NULL REFERENCES relation_kind(id),
  friction  REAL,
  title     TEXT,                          -- non-null ⇒ annotated
  body      TEXT
);

-- Endpoints reference ENTITY, so a relation can point at a relation.
-- Table (not two columns) so n-ary relations need no migration.
CREATE TABLE relation_endpoint (
  relation_id TEXT NOT NULL REFERENCES relation(entity_id) ON DELETE CASCADE,
  entity_id   TEXT NOT NULL REFERENCES entity(id)          ON DELETE CASCADE,
  role        TEXT NOT NULL CHECK (role IN ('source','target','member')),
  ordinal     INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (relation_id, entity_id, role, ordinal)
);
CREATE INDEX idx_endpoint_entity ON relation_endpoint(entity_id);

CREATE TABLE system (
  id                     TEXT PRIMARY KEY,
  name                   TEXT NOT NULL,
  description            TEXT,
  color                  INTEGER NOT NULL,
  hotkey                 INTEGER,
  default_layout         TEXT NOT NULL DEFAULT 'force',
  default_relation_kinds TEXT NOT NULL DEFAULT '[]',
  confusion              INTEGER NOT NULL DEFAULT 0,
  inbox                  INTEGER NOT NULL DEFAULT 0
);

-- Membership is over ENTITY: a relation joins a system exactly like a node does.
CREATE TABLE membership (
  system_id TEXT NOT NULL REFERENCES system(id) ON DELETE CASCADE,
  entity_id TEXT NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  added_at  INTEGER NOT NULL,
  PRIMARY KEY (system_id, entity_id)
);
CREATE INDEX idx_membership_entity ON membership(entity_id);

-- Anchors are over ENTITY: relations can cite a passage too.
CREATE TABLE anchor (
  id          TEXT PRIMARY KEY,
  entity_id   TEXT NOT NULL REFERENCES entity(id)   ON DELETE CASCADE,
  document_id TEXT NOT NULL REFERENCES document(id) ON DELETE CASCADE,
  page_index  INTEGER NOT NULL,
  kind        TEXT NOT NULL CHECK (kind IN ('text','region','object')),
  quads       BLOB NOT NULL,
  exact       TEXT NOT NULL DEFAULT '',
  prefix      TEXT NOT NULL DEFAULT '',
  suffix      TEXT NOT NULL DEFAULT '',
  char_start  INTEGER, char_end INTEGER,
  confidence  REAL NOT NULL DEFAULT 1.0,
  color       INTEGER
);
CREATE INDEX idx_anchor_doc_page ON anchor(document_id, page_index);
CREATE INDEX idx_anchor_entity ON anchor(entity_id);

CREATE TABLE view (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, hotkey INTEGER,
  state JSON NOT NULL                      -- overlays, mode, camera, pins, lens
);

CREATE TABLE undo_log (
  seq     INTEGER PRIMARY KEY AUTOINCREMENT,
  at      INTEGER NOT NULL,
  label   TEXT NOT NULL,
  forward JSON NOT NULL,
  inverse JSON NOT NULL,
  undone  INTEGER NOT NULL DEFAULT 0
);

CREATE VIRTUAL TABLE entity_fts USING fts5(entity_id UNINDEXED, title, body);
"#;

const MIGRATIONS: &[&str] = &[V1];

/// Bring the database to the latest schema. Returns `true` if it was empty.
pub fn migrate(conn: &Connection) -> Result<bool> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let latest = MIGRATIONS.len() as i64;
    if version > latest {
        return Err(StoreError::TooNew { found: version, supported: latest });
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        conn.execute_batch(&format!("BEGIN; {sql}; PRAGMA user_version = {}; COMMIT;", i + 1))?;
    }
    Ok(version == 0)
}
