//! User-level actions compiled to [`Tx`]s. Builders read the current graph but
//! never mutate it; the caller applies the returned transaction (optimistically
//! in memory, then persisted by the store).

use crate::graph::{CoreError, Graph, Result};
use crate::ids::*;
use crate::model::*;
use crate::op::{Op, Tx};

fn node(g: &Graph, id: &EntityId) -> Result<Node> {
    g.nodes.get(id).cloned().ok_or_else(|| CoreError::NotFound(id.to_string()))
}

fn relation(g: &Graph, id: &EntityId) -> Result<Relation> {
    g.relations.get(id).cloned().ok_or_else(|| CoreError::NotFound(id.to_string()))
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| s.to_owned())
}

pub struct NewNode {
    pub title: String,
    pub body: String,
    /// Empty ⇒ filed in the Inbox.
    pub systems: Vec<SystemId>,
    /// `entity` and `id` are overwritten.
    pub anchors: Vec<Anchor>,
}

/// F-NODE-1 / F-CAP-2: create a node, file it, anchor it.
pub fn create_node(g: &Graph, new: NewNode, now: Timestamp) -> (Tx, EntityId) {
    let id = EntityId::generate();
    let mut tx = Tx::new(format!("create “{}”", new.title));
    tx.push(Op::AddNode(Node {
        id: id.clone(),
        title: new.title,
        body: new.body,
        abstraction_level: None,
        color_override: None,
        created_at: now,
        updated_at: now,
    }));
    let mut systems = new.systems;
    if systems.is_empty()
        && let Some(inbox) = g.inbox()
    {
        systems.push(inbox.id.clone());
    }
    for s in systems {
        tx.push(Op::AddMembership(Membership { system: s, entity: id.clone(), added_at: now }));
    }
    for mut a in new.anchors {
        a.id = AnchorId::generate();
        a.entity = id.clone();
        tx.push(Op::AddAnchor(a));
    }
    (tx, id)
}

/// F-CAP-4 / F-REL-1: a binary relation `source —kind→ target`.
pub fn link(
    g: &Graph,
    source: &EntityId,
    target: &EntityId,
    kind: &KindId,
    now: Timestamp,
) -> Result<(Tx, EntityId)> {
    let k = g.kinds.get(kind).ok_or_else(|| CoreError::NotFound(kind.to_string()))?;
    let id = EntityId::generate();
    let mut tx = Tx::new(format!("link {}", k.name));
    tx.push(Op::AddRelation(Relation {
        id: id.clone(),
        kind: kind.clone(),
        endpoints: vec![
            Endpoint { entity: source.clone(), role: Role::Source, ordinal: 0 },
            Endpoint { entity: target.clone(), role: Role::Target, ordinal: 1 },
        ],
        friction: None,
        title: None,
        body: None,
        created_at: now,
        updated_at: now,
    }));
    if k.joins_confusion
        && let Some(c) = g.confusion_system()
    {
        tx.push(Op::AddMembership(Membership { system: c.id.clone(), entity: id.clone(), added_at: now }));
    }
    Ok((tx, id))
}

/// F-NODE-2 / F-REL-3: set title and body. For a relation this annotates it;
/// topology is untouched (I1).
pub fn annotate(g: &Graph, id: &EntityId, title: &str, body: &str, now: Timestamp) -> Result<Tx> {
    let mut tx = Tx::new("edit");
    if let Some(n) = g.nodes.get(id) {
        let mut after = n.clone();
        after.title = title.to_owned();
        after.body = body.to_owned();
        after.updated_at = now;
        tx.push(Op::UpdateNode { before: n.clone(), after });
    } else {
        let r = relation(g, id)?;
        let mut after = r.clone();
        after.title = non_empty(title);
        after.body = non_empty(body);
        after.updated_at = now;
        tx.label = "annotate relation".into();
        tx.push(Op::UpdateRelation { before: r, after });
    }
    Ok(tx)
}

/// F-NODE-5 / F-REL-4: file into or out of a system. Nodes leave the Inbox
/// when filed elsewhere and return to it when they leave their last system.
pub fn set_membership(
    g: &Graph,
    id: &EntityId,
    system: &SystemId,
    member: bool,
    now: Timestamp,
) -> Result<Tx> {
    if !g.contains(id) {
        return Err(CoreError::NotFound(id.to_string()));
    }
    let sys = g.systems.get(system).ok_or_else(|| CoreError::NotFound(system.to_string()))?;
    let mut tx = Tx::new(format!("{} {}", if member { "file into" } else { "remove from" }, sys.name));
    let is_node = g.nodes.contains_key(id);
    let inbox = g.inbox().map(|s| s.id.clone());
    let current: Vec<SystemId> = g.systems_of(id).cloned().collect();
    if member {
        if current.contains(system) {
            return Ok(tx);
        }
        tx.push(Op::AddMembership(Membership { system: system.clone(), entity: id.clone(), added_at: now }));
        if is_node
            && !sys.inbox
            && let Some(inbox) = inbox.filter(|i| current.contains(i))
        {
            tx.push(Op::RemoveMembership(membership(g, id, &inbox)));
        }
    } else {
        if !current.contains(system) {
            return Ok(tx);
        }
        tx.push(Op::RemoveMembership(membership(g, id, system)));
        if is_node
            && current.len() == 1
            && let Some(inbox) = inbox.filter(|i| i != system)
        {
            tx.push(Op::AddMembership(Membership { system: inbox, entity: id.clone(), added_at: now }));
        }
    }
    Ok(tx)
}

fn membership(g: &Graph, id: &EntityId, system: &SystemId) -> Membership {
    Membership {
        system: system.clone(),
        entity: id.clone(),
        added_at: g.memberships[&(system.clone(), id.clone())],
    }
}

/// I2: delete an entity and, recursively, every relation attached to it,
/// with all their memberships and anchors. The inverse restores everything.
pub fn delete_entity(g: &Graph, id: &EntityId) -> Result<Tx> {
    let cascade = g.cascade(id);
    if cascade.is_empty() {
        return Err(CoreError::NotFound(id.to_string()));
    }
    let mut tx = Tx::new(if cascade.len() > 1 {
        format!("delete “{}” and {} attached", g.title(id), cascade.len() - 1)
    } else {
        format!("delete “{}”", g.title(id))
    });
    for e in &cascade {
        let systems: Vec<SystemId> = g.systems_of(e).cloned().collect();
        for s in systems {
            tx.push(Op::RemoveMembership(membership(g, e, &s)));
        }
        for a in g.anchors_of(e) {
            tx.push(Op::RemoveAnchor(a.clone()));
        }
        if let Some(n) = g.nodes.get(e) {
            tx.push(Op::RemoveNode(n.clone()));
        } else if let Some(r) = g.relations.get(e) {
            tx.push(Op::RemoveRelation(r.clone()));
        }
    }
    Ok(tx)
}

/// F-REL-6 `k`.
pub fn set_kind(g: &Graph, id: &EntityId, kind: &KindId, now: Timestamp) -> Result<Tx> {
    if !g.kinds.contains_key(kind) {
        return Err(CoreError::NotFound(kind.to_string()));
    }
    update_relation(g, id, "change kind", now, |r| r.kind = kind.clone())
}

/// F-REL-6 `Ctrl-r`.
pub fn reverse(g: &Graph, id: &EntityId, now: Timestamp) -> Result<Tx> {
    update_relation(g, id, "reverse", now, |r| {
        for e in &mut r.endpoints {
            e.role = match e.role {
                Role::Source => Role::Target,
                Role::Target => Role::Source,
                Role::Member => Role::Member,
            };
        }
        r.endpoints.sort_by_key(|e| e.role);
        for (i, e) in r.endpoints.iter_mut().enumerate() {
            e.ordinal = i as u32;
        }
    })
}

/// F-REL-7.
pub fn set_friction(g: &Graph, id: &EntityId, friction: Option<f32>, now: Timestamp) -> Result<Tx> {
    update_relation(g, id, "set friction", now, |r| r.friction = friction)
}

fn update_relation(
    g: &Graph,
    id: &EntityId,
    label: &str,
    now: Timestamp,
    f: impl FnOnce(&mut Relation),
) -> Result<Tx> {
    let before = relation(g, id)?;
    let mut after = before.clone();
    f(&mut after);
    after.updated_at = now;
    let mut tx = Tx::new(label);
    tx.push(Op::UpdateRelation { before, after });
    Ok(tx)
}

/// F-NODE-6: nudge the abstraction level by `delta`, clamped to -3..=3.
pub fn shift_level(g: &Graph, id: &EntityId, delta: i8, now: Timestamp) -> Result<Tx> {
    let before = node(g, id)?;
    let mut after = before.clone();
    after.abstraction_level = Some((before.abstraction_level.unwrap_or(0) + delta).clamp(-3, 3));
    after.updated_at = now;
    let mut tx = Tx::new("set abstraction level");
    tx.push(Op::UpdateNode { before, after });
    Ok(tx)
}

/// F-CAP-7 / F-REL-8: another anchor on an existing entity.
pub fn add_anchor(g: &Graph, id: &EntityId, mut anchor: Anchor) -> Result<(Tx, AnchorId)> {
    if !g.contains(id) {
        return Err(CoreError::NotFound(id.to_string()));
    }
    anchor.id = AnchorId::generate();
    anchor.entity = id.clone();
    let aid = anchor.id.clone();
    let mut tx = Tx::new("add anchor");
    tx.push(Op::AddAnchor(anchor));
    Ok((tx, aid))
}

/// F-CAP-1: a plain highlight creates no node. Anchors always belong to an
/// entity, so plain highlights hang off a hidden per-document carrier node
/// (created on first use) that the graph views filter out.
pub fn highlight(g: &Graph, doc: &DocumentId, mut anchor: Anchor, now: Timestamp) -> (Tx, AnchorId) {
    let carrier = highlight_carrier_id(doc);
    let mut tx = Tx::new("highlight");
    if !g.nodes.contains_key(&carrier) {
        tx.push(Op::AddNode(Node {
            id: carrier.clone(),
            title: HIGHLIGHT_CARRIER_TITLE.into(),
            body: String::new(),
            abstraction_level: None,
            color_override: None,
            created_at: now,
            updated_at: now,
        }));
    }
    anchor.id = AnchorId::generate();
    anchor.entity = carrier;
    let aid = anchor.id.clone();
    tx.push(Op::AddAnchor(anchor));
    (tx, aid)
}

pub const HIGHLIGHT_CARRIER_TITLE: &str = "⟨highlights⟩";

/// Plain highlights (no node) hang off one hidden carrier node per document.
pub fn highlight_carrier_id(doc: &DocumentId) -> EntityId {
    EntityId(format!("hl:{}", doc.0))
}

pub fn is_highlight_carrier(id: &EntityId) -> bool {
    id.0.starts_with("hl:")
}

pub fn remove_anchor(g: &Graph, anchor: &AnchorId) -> Result<Tx> {
    let a = g.anchors.get(anchor).cloned().ok_or_else(|| CoreError::NotFound(anchor.to_string()))?;
    let mut tx = Tx::new("remove anchor");
    tx.push(Op::RemoveAnchor(a));
    Ok(tx)
}

pub fn update_anchor(g: &Graph, after: Anchor) -> Result<Tx> {
    let before =
        g.anchors.get(&after.id).cloned().ok_or_else(|| CoreError::NotFound(after.id.to_string()))?;
    let mut tx = Tx::new("re-anchor");
    tx.push(Op::UpdateAnchor { before, after });
    Ok(tx)
}

pub fn add_document(g: &Graph, doc: Document) -> Tx {
    let mut tx = Tx::new(format!("add {}", doc.title.as_deref().unwrap_or(&doc.path)));
    if let Some(existing) = g.documents.get(&doc.id) {
        if existing.path != doc.path {
            let mut after = existing.clone();
            after.path = doc.path;
            tx.push(Op::UpdateDocument { before: existing.clone(), after });
        }
    } else {
        tx.push(Op::AddDocument(doc));
    }
    tx
}

pub fn create_system(name: &str, color: Color, hotkey: Option<u8>) -> (Tx, SystemId) {
    let id = SystemId::generate();
    let mut tx = Tx::new(format!("create system {name}"));
    tx.push(Op::AddSystem(System {
        id: id.clone(),
        name: name.to_owned(),
        description: String::new(),
        color,
        default_relation_kinds: vec![],
        default_layout: LayoutKind::Force,
        hotkey,
        confusion: false,
        inbox: false,
    }));
    (tx, id)
}

pub fn update_system(g: &Graph, id: &SystemId, f: impl FnOnce(&mut System)) -> Result<Tx> {
    let before = g.systems.get(id).cloned().ok_or_else(|| CoreError::NotFound(id.to_string()))?;
    let mut after = before.clone();
    f(&mut after);
    let mut tx = Tx::new(format!("edit system {}", before.name));
    tx.push(Op::UpdateSystem { before, after });
    Ok(tx)
}

/// The kind a fresh link in `system` uses by default (F-CAP-4): the system's
/// first preferred kind, else `prerequisite-of`, else any kind.
pub fn default_kind(g: &Graph, system: Option<&SystemId>) -> Option<KindId> {
    system
        .and_then(|s| g.systems.get(s))
        .and_then(|s| s.default_relation_kinds.iter().find(|k| g.kinds.contains_key(*k)).cloned())
        .or_else(|| {
            let p = KindId::from(crate::defaults::PREREQUISITE_OF);
            g.kinds.contains_key(&p).then_some(p)
        })
        .or_else(|| g.kinds.keys().next().cloned())
}

/// The kind palette for a system: preferred kinds first, then the rest.
pub fn kind_palette(g: &Graph, system: Option<&SystemId>) -> Vec<KindId> {
    let mut out: Vec<KindId> = system
        .and_then(|s| g.systems.get(s))
        .map(|s| s.default_relation_kinds.iter().filter(|k| g.kinds.contains_key(*k)).cloned().collect())
        .unwrap_or_default();
    for k in crate::defaults::KIND_ORDER {
        let k = KindId::from(*k);
        if g.kinds.contains_key(&k) && !out.contains(&k) {
            out.push(k);
        }
    }
    for k in g.kinds.keys() {
        if !out.contains(k) {
            out.push(k.clone());
        }
    }
    out
}
