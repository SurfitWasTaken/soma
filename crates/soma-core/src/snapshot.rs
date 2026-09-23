//! Lossless, versioned JSON export (F-DATA-5, acceptance test A8).

use crate::graph::{Graph, Result};
use crate::model::*;
use crate::op::{Op, Tx};
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub format: String,
    pub schema_version: u32,
    pub documents: Vec<Document>,
    pub kinds: Vec<RelationKind>,
    pub systems: Vec<System>,
    pub nodes: Vec<Node>,
    pub relations: Vec<Relation>,
    pub memberships: Vec<Membership>,
    pub anchors: Vec<Anchor>,
}

impl Graph {
    pub fn to_snapshot(&self) -> Snapshot {
        Snapshot {
            format: "soma".into(),
            schema_version: SCHEMA_VERSION,
            documents: self.documents.values().cloned().collect(),
            kinds: self.kinds.values().cloned().collect(),
            systems: self.systems.values().cloned().collect(),
            nodes: self.nodes.values().cloned().collect(),
            relations: self.relations.values().cloned().collect(),
            memberships: self
                .memberships
                .iter()
                .map(|((s, e), at)| Membership { system: s.clone(), entity: e.clone(), added_at: *at })
                .collect(),
            anchors: self.anchors.values().cloned().collect(),
        }
    }

    /// A transaction that recreates the snapshot, in dependency order.
    pub fn snapshot_tx(s: &Snapshot) -> Tx {
        let mut tx = Tx::new("import");
        tx.ops.extend(s.documents.iter().cloned().map(Op::AddDocument));
        tx.ops.extend(s.kinds.iter().cloned().map(Op::AddKind));
        tx.ops.extend(s.systems.iter().cloned().map(Op::AddSystem));
        tx.ops.extend(s.nodes.iter().cloned().map(Op::AddNode));
        // Relations onto relations must come after their endpoints.
        let mut pending: Vec<&Relation> = s.relations.iter().collect();
        let mut placed = std::collections::HashSet::new();
        let node_ids: std::collections::HashSet<_> = s.nodes.iter().map(|n| &n.id).collect();
        while !pending.is_empty() {
            let before = pending.len();
            pending.retain(|r| {
                let ready = r.endpoint_ids().all(|e| node_ids.contains(e) || placed.contains(e));
                if ready {
                    placed.insert(&r.id);
                    tx.ops.push(Op::AddRelation((*r).clone()));
                }
                !ready
            });
            if pending.len() == before {
                // Dangling or cyclic — let apply report it.
                tx.ops.extend(pending.drain(..).cloned().map(Op::AddRelation));
            }
        }
        tx.ops.extend(s.memberships.iter().cloned().map(Op::AddMembership));
        tx.ops.extend(s.anchors.iter().cloned().map(Op::AddAnchor));
        tx
    }

    pub fn from_snapshot(s: &Snapshot) -> Result<Graph> {
        let mut g = Graph::new();
        g.apply(&Self::snapshot_tx(s))?;
        Ok(g)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.to_snapshot()).expect("snapshot serializes")
    }

    pub fn from_json(json: &str) -> std::result::Result<Graph, String> {
        let s: Snapshot = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if s.schema_version > SCHEMA_VERSION {
            return Err(format!(
                "export schema {} is newer than supported {SCHEMA_VERSION}",
                s.schema_version
            ));
        }
        Graph::from_snapshot(&s).map_err(|e| e.to_string())
    }
}
