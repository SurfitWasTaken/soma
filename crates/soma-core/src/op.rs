//! Primitive, invertible mutations. Every user action is a [`Tx`] of these;
//! undo is the reversed list of inverses (PRD §6.1 principle 5).
//!
//! `Remove*` ops carry the full record so they can be inverted, and they never
//! cascade implicitly: a cascade is spelled out as explicit ops by
//! [`crate::commands::delete_entity`], which is what makes a single undo
//! restore everything (acceptance test A4).

use crate::model::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    AddDocument(Document),
    RemoveDocument(Document),
    UpdateDocument { before: Document, after: Document },

    AddKind(RelationKind),
    RemoveKind(RelationKind),
    UpdateKind { before: RelationKind, after: RelationKind },

    AddSystem(System),
    RemoveSystem(System),
    UpdateSystem { before: System, after: System },

    AddNode(Node),
    RemoveNode(Node),
    UpdateNode { before: Node, after: Node },

    AddRelation(Relation),
    RemoveRelation(Relation),
    UpdateRelation { before: Relation, after: Relation },

    AddMembership(Membership),
    RemoveMembership(Membership),

    AddAnchor(Anchor),
    RemoveAnchor(Anchor),
    UpdateAnchor { before: Anchor, after: Anchor },
}

impl Op {
    pub fn inverse(&self) -> Op {
        use Op::*;
        match self.clone() {
            AddDocument(x) => RemoveDocument(x),
            RemoveDocument(x) => AddDocument(x),
            UpdateDocument { before, after } => UpdateDocument { before: after, after: before },
            AddKind(x) => RemoveKind(x),
            RemoveKind(x) => AddKind(x),
            UpdateKind { before, after } => UpdateKind { before: after, after: before },
            AddSystem(x) => RemoveSystem(x),
            RemoveSystem(x) => AddSystem(x),
            UpdateSystem { before, after } => UpdateSystem { before: after, after: before },
            AddNode(x) => RemoveNode(x),
            RemoveNode(x) => AddNode(x),
            UpdateNode { before, after } => UpdateNode { before: after, after: before },
            AddRelation(x) => RemoveRelation(x),
            RemoveRelation(x) => AddRelation(x),
            UpdateRelation { before, after } => UpdateRelation { before: after, after: before },
            AddMembership(x) => RemoveMembership(x),
            RemoveMembership(x) => AddMembership(x),
            AddAnchor(x) => RemoveAnchor(x),
            RemoveAnchor(x) => AddAnchor(x),
            UpdateAnchor { before, after } => UpdateAnchor { before: after, after: before },
        }
    }
}

/// An atomic, undoable unit of change.
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct Tx {
    pub label: String,
    pub ops: Vec<Op>,
}

impl Tx {
    pub fn new(label: impl Into<String>) -> Self {
        Self { label: label.into(), ops: Vec::new() }
    }

    pub fn push(&mut self, op: Op) -> &mut Self {
        self.ops.push(op);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn inverse(&self) -> Tx {
        Tx { label: format!("undo {}", self.label), ops: self.ops.iter().rev().map(Op::inverse).collect() }
    }

    pub fn extend(&mut self, other: Tx) {
        self.ops.extend(other.ops);
    }
}
