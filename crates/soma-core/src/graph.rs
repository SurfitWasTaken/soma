//! The in-memory workspace graph. All mutation goes through [`Graph::apply`],
//! which enforces the invariants of PRD §4.4 and is atomic per [`Tx`].

use crate::ids::*;
use crate::model::*;
use crate::op::{Op, Tx};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use thiserror::Error;

/// I4: relation-on-relation nesting cap. A relation between two nodes has
/// depth 1; a relation onto that relation has depth 2, and so on.
pub const MAX_RELATION_DEPTH: usize = 4;

/// Friction assumed for cross-level relations until the user sets one (§4.6).
pub const DEFAULT_CROSS_LEVEL_FRICTION: f32 = 0.5;

#[derive(Debug, Error, PartialEq)]
pub enum CoreError {
    #[error("{0} not found")]
    NotFound(String),
    #[error("{0} already exists")]
    AlreadyExists(String),
    #[error("stale update to {0}: record changed since it was read")]
    Stale(String),
    #[error("relation {0} would be, transitively, its own endpoint (I3)")]
    SelfEndpoint(EntityId),
    #[error(
        "relation nesting depth {depth} exceeds cap of {MAX_RELATION_DEPTH} (I4) — this thought may want to be a node"
    )]
    DepthExceeded { depth: usize },
    #[error("invalid endpoints: {0}")]
    InvalidEndpoints(String),
    #[error("updating a relation may not change which entities it connects (I1)")]
    TopologyChange,
    #[error("{0} is still referenced: {1}")]
    StillReferenced(String, String),
    #[error("invalid value: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Graph {
    pub documents: BTreeMap<DocumentId, Document>,
    pub kinds: BTreeMap<KindId, RelationKind>,
    pub systems: BTreeMap<SystemId, System>,
    pub nodes: BTreeMap<EntityId, Node>,
    pub relations: BTreeMap<EntityId, Relation>,
    pub anchors: BTreeMap<AnchorId, Anchor>,
    /// (system, entity) → added_at
    pub memberships: BTreeMap<(SystemId, EntityId), Timestamp>,

    // Derived indexes, maintained by apply.
    incident: HashMap<EntityId, BTreeSet<EntityId>>,
    entity_systems: HashMap<EntityId, BTreeSet<SystemId>>,
    system_members: HashMap<SystemId, BTreeSet<EntityId>>,
    entity_anchors: HashMap<EntityId, BTreeSet<AnchorId>>,
    doc_anchors: HashMap<DocumentId, BTreeSet<AnchorId>>,
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    // ---------------------------------------------------------------- apply

    /// Apply a transaction atomically: either every op applies or none do.
    pub fn apply(&mut self, tx: &Tx) -> Result<()> {
        for (i, op) in tx.ops.iter().enumerate() {
            if let Err(e) = self.apply_op(op) {
                for done in tx.ops[..i].iter().rev() {
                    self.apply_op(&done.inverse()).expect("rollback of an applied op cannot fail");
                }
                return Err(e);
            }
        }
        Ok(())
    }

    fn apply_op(&mut self, op: &Op) -> Result<()> {
        match op {
            Op::AddDocument(d) => {
                if self.documents.contains_key(&d.id) {
                    return Err(CoreError::AlreadyExists(d.id.to_string()));
                }
                self.documents.insert(d.id.clone(), d.clone());
            }
            Op::RemoveDocument(d) => {
                self.expect_eq(&self.documents, &d.id, d)?;
                if self.doc_anchors.get(&d.id).is_some_and(|s| !s.is_empty()) {
                    return Err(CoreError::StillReferenced(d.id.to_string(), "anchors".into()));
                }
                self.documents.remove(&d.id);
            }
            Op::UpdateDocument { before, after } => {
                same_id(&before.id, &after.id)?;
                self.expect_eq(&self.documents, &before.id, before)?;
                self.documents.insert(after.id.clone(), after.clone());
            }

            Op::AddKind(k) => {
                if self.kinds.contains_key(&k.id) {
                    return Err(CoreError::AlreadyExists(k.id.to_string()));
                }
                self.kinds.insert(k.id.clone(), k.clone());
            }
            Op::RemoveKind(k) => {
                self.expect_eq(&self.kinds, &k.id, k)?;
                if self.relations.values().any(|r| r.kind == k.id) {
                    return Err(CoreError::StillReferenced(k.id.to_string(), "relations".into()));
                }
                self.kinds.remove(&k.id);
            }
            Op::UpdateKind { before, after } => {
                same_id(&before.id, &after.id)?;
                self.expect_eq(&self.kinds, &before.id, before)?;
                self.kinds.insert(after.id.clone(), after.clone());
            }

            Op::AddSystem(s) => {
                if self.systems.contains_key(&s.id) {
                    return Err(CoreError::AlreadyExists(s.id.to_string()));
                }
                validate_hotkey(s.hotkey)?;
                self.systems.insert(s.id.clone(), s.clone());
            }
            Op::RemoveSystem(s) => {
                self.expect_eq(&self.systems, &s.id, s)?;
                if self.system_members.get(&s.id).is_some_and(|m| !m.is_empty()) {
                    return Err(CoreError::StillReferenced(s.id.to_string(), "members".into()));
                }
                self.systems.remove(&s.id);
            }
            Op::UpdateSystem { before, after } => {
                same_id(&before.id, &after.id)?;
                self.expect_eq(&self.systems, &before.id, before)?;
                validate_hotkey(after.hotkey)?;
                self.systems.insert(after.id.clone(), after.clone());
            }

            Op::AddNode(n) => {
                self.expect_absent(&n.id)?;
                validate_level(n.abstraction_level)?;
                self.nodes.insert(n.id.clone(), n.clone());
            }
            Op::RemoveNode(n) => {
                self.expect_eq(&self.nodes, &n.id, n)?;
                self.expect_unreferenced(&n.id)?;
                self.nodes.remove(&n.id);
            }
            Op::UpdateNode { before, after } => {
                same_id(&before.id, &after.id)?;
                self.expect_eq(&self.nodes, &before.id, before)?;
                validate_level(after.abstraction_level)?;
                self.nodes.insert(after.id.clone(), after.clone());
            }

            Op::AddRelation(r) => {
                self.expect_absent(&r.id)?;
                self.validate_relation(r)?;
                for e in r.endpoint_ids() {
                    self.incident.entry(e.clone()).or_default().insert(r.id.clone());
                }
                self.relations.insert(r.id.clone(), r.clone());
            }
            Op::RemoveRelation(r) => {
                self.expect_eq(&self.relations, &r.id, r)?;
                self.expect_unreferenced(&r.id)?;
                for e in r.endpoint_ids() {
                    if let Some(set) = self.incident.get_mut(e) {
                        set.remove(&r.id);
                        if set.is_empty() {
                            self.incident.remove(e);
                        }
                    }
                }
                self.relations.remove(&r.id);
            }
            Op::UpdateRelation { before, after } => {
                same_id(&before.id, &after.id)?;
                self.expect_eq(&self.relations, &before.id, before)?;
                // I1: content, kind, friction and roles may change; the set of
                // connected entities may not.
                let set = |r: &Relation| r.endpoint_ids().cloned().collect::<BTreeSet<_>>();
                if set(before) != set(after) || before.endpoints.len() != after.endpoints.len() {
                    return Err(CoreError::TopologyChange);
                }
                self.validate_relation_fields(after)?;
                self.relations.insert(after.id.clone(), after.clone());
            }

            Op::AddMembership(m) => {
                if !self.systems.contains_key(&m.system) {
                    return Err(CoreError::NotFound(m.system.to_string()));
                }
                self.expect_entity(&m.entity)?;
                let key = (m.system.clone(), m.entity.clone());
                if self.memberships.contains_key(&key) {
                    return Err(CoreError::AlreadyExists(format!("{} ∈ {}", m.entity, m.system)));
                }
                self.memberships.insert(key, m.added_at);
                self.entity_systems.entry(m.entity.clone()).or_default().insert(m.system.clone());
                self.system_members.entry(m.system.clone()).or_default().insert(m.entity.clone());
            }
            Op::RemoveMembership(m) => {
                let key = (m.system.clone(), m.entity.clone());
                match self.memberships.get(&key) {
                    None => return Err(CoreError::NotFound(format!("{} ∈ {}", m.entity, m.system))),
                    Some(at) if *at != m.added_at => return Err(CoreError::Stale(m.entity.to_string())),
                    _ => {}
                }
                self.memberships.remove(&key);
                remove_from(&mut self.entity_systems, &m.entity, &m.system);
                remove_from(&mut self.system_members, &m.system, &m.entity);
            }

            Op::AddAnchor(a) => {
                if self.anchors.contains_key(&a.id) {
                    return Err(CoreError::AlreadyExists(a.id.to_string()));
                }
                self.expect_entity(&a.entity)?;
                if !self.documents.contains_key(&a.document) {
                    return Err(CoreError::NotFound(a.document.to_string()));
                }
                self.entity_anchors.entry(a.entity.clone()).or_default().insert(a.id.clone());
                self.doc_anchors.entry(a.document.clone()).or_default().insert(a.id.clone());
                self.anchors.insert(a.id.clone(), a.clone());
            }
            Op::RemoveAnchor(a) => {
                self.expect_eq(&self.anchors, &a.id, a)?;
                remove_from(&mut self.entity_anchors, &a.entity, &a.id);
                remove_from(&mut self.doc_anchors, &a.document, &a.id);
                self.anchors.remove(&a.id);
            }
            Op::UpdateAnchor { before, after } => {
                same_id(&before.id, &after.id)?;
                self.expect_eq(&self.anchors, &before.id, before)?;
                if before.entity != after.entity {
                    return Err(CoreError::Invalid("anchor may not move between entities".into()));
                }
                if !self.documents.contains_key(&after.document) {
                    return Err(CoreError::NotFound(after.document.to_string()));
                }
                remove_from(&mut self.doc_anchors, &before.document, &before.id);
                self.doc_anchors.entry(after.document.clone()).or_default().insert(after.id.clone());
                self.anchors.insert(after.id.clone(), after.clone());
            }
        }
        Ok(())
    }

    fn expect_eq<K: Ord + std::fmt::Display, V: PartialEq>(
        &self,
        map: &BTreeMap<K, V>,
        id: &K,
        expected: &V,
    ) -> Result<()> {
        match map.get(id) {
            None => Err(CoreError::NotFound(id.to_string())),
            Some(v) if v != expected => Err(CoreError::Stale(id.to_string())),
            Some(_) => Ok(()),
        }
    }

    fn expect_absent(&self, id: &EntityId) -> Result<()> {
        if self.contains(id) { Err(CoreError::AlreadyExists(id.to_string())) } else { Ok(()) }
    }

    fn expect_entity(&self, id: &EntityId) -> Result<()> {
        if self.contains(id) { Ok(()) } else { Err(CoreError::NotFound(id.to_string())) }
    }

    fn expect_unreferenced(&self, id: &EntityId) -> Result<()> {
        if self.incident.get(id).is_some_and(|s| !s.is_empty()) {
            return Err(CoreError::StillReferenced(id.to_string(), "relations".into()));
        }
        if self.entity_systems.get(id).is_some_and(|s| !s.is_empty()) {
            return Err(CoreError::StillReferenced(id.to_string(), "memberships".into()));
        }
        if self.entity_anchors.get(id).is_some_and(|s| !s.is_empty()) {
            return Err(CoreError::StillReferenced(id.to_string(), "anchors".into()));
        }
        Ok(())
    }

    fn validate_relation_fields(&self, r: &Relation) -> Result<()> {
        if !self.kinds.contains_key(&r.kind) {
            return Err(CoreError::NotFound(r.kind.to_string()));
        }
        if let Some(f) = r.friction
            && !(0.0..=1.0).contains(&f)
        {
            return Err(CoreError::Invalid(format!("friction {f} outside 0..=1")));
        }
        let sources = r.endpoints.iter().filter(|e| e.role == Role::Source).count();
        let targets = r.endpoints.iter().filter(|e| e.role == Role::Target).count();
        if sources > 1 || targets > 1 || sources != targets {
            return Err(CoreError::InvalidEndpoints(
                "a relation has either one source and one target, or only members".into(),
            ));
        }
        Ok(())
    }

    fn validate_relation(&self, r: &Relation) -> Result<()> {
        self.validate_relation_fields(r)?;
        if r.endpoints.len() < 2 {
            return Err(CoreError::InvalidEndpoints("need at least two endpoints".into()));
        }
        let distinct: HashSet<_> = r.endpoint_ids().collect();
        if distinct.len() != r.endpoints.len() {
            return Err(CoreError::InvalidEndpoints("duplicate endpoint".into()));
        }
        for e in r.endpoint_ids() {
            if *e == r.id {
                return Err(CoreError::SelfEndpoint(r.id.clone()));
            }
            self.expect_entity(e)?;
        }
        // I3: r may not be reachable through its endpoints' endpoints.
        let mut stack: Vec<&EntityId> = r.endpoint_ids().collect();
        let mut seen = HashSet::new();
        while let Some(e) = stack.pop() {
            if *e == r.id {
                return Err(CoreError::SelfEndpoint(r.id.clone()));
            }
            if seen.insert(e)
                && let Some(inner) = self.relations.get(e)
            {
                stack.extend(inner.endpoint_ids());
            }
        }
        // I4
        let depth = 1 + r.endpoint_ids().map(|e| self.depth(e)).max().unwrap_or(0);
        if depth > MAX_RELATION_DEPTH {
            return Err(CoreError::DepthExceeded { depth });
        }
        Ok(())
    }

    // -------------------------------------------------------------- queries

    pub fn contains(&self, id: &EntityId) -> bool {
        self.nodes.contains_key(id) || self.relations.contains_key(id)
    }

    pub fn is_relation(&self, id: &EntityId) -> bool {
        self.relations.contains_key(id)
    }

    pub fn entity_count(&self) -> usize {
        self.nodes.len() + self.relations.len()
    }

    pub fn entity_ids(&self) -> impl Iterator<Item = &EntityId> {
        self.nodes.keys().chain(self.relations.keys())
    }

    /// Display title: node title, relation title, or `kind` for bare relations.
    pub fn title(&self, id: &EntityId) -> String {
        if let Some(n) = self.nodes.get(id) {
            return n.title.clone();
        }
        if let Some(r) = self.relations.get(id) {
            if let Some(t) = r.title.as_deref().filter(|t| !t.is_empty()) {
                return t.to_owned();
            }
            let name = |e: Option<&EntityId>| e.map(|e| self.title(e)).unwrap_or_default();
            let kind = self.kinds.get(&r.kind).map(|k| k.name.as_str()).unwrap_or("?");
            let mut ends = r.endpoint_ids();
            return format!("{} —{}→ {}", name(ends.next()), kind, name(ends.next()));
        }
        String::new()
    }

    pub fn body(&self, id: &EntityId) -> &str {
        if let Some(n) = self.nodes.get(id) {
            return &n.body;
        }
        self.relations.get(id).and_then(|r| r.body.as_deref()).unwrap_or("")
    }

    /// Relations having `id` as an endpoint.
    pub fn incident(&self, id: &EntityId) -> impl Iterator<Item = &EntityId> {
        self.incident.get(id).into_iter().flatten()
    }

    pub fn systems_of(&self, id: &EntityId) -> impl Iterator<Item = &SystemId> {
        self.entity_systems.get(id).into_iter().flatten()
    }

    pub fn members_of(&self, system: &SystemId) -> impl Iterator<Item = &EntityId> {
        self.system_members.get(system).into_iter().flatten()
    }

    pub fn in_system(&self, id: &EntityId, system: &SystemId) -> bool {
        self.memberships.contains_key(&(system.clone(), id.clone()))
    }

    pub fn anchors_of(&self, id: &EntityId) -> impl Iterator<Item = &Anchor> {
        self.entity_anchors.get(id).into_iter().flatten().filter_map(|a| self.anchors.get(a))
    }

    pub fn anchors_on_page(&self, doc: &DocumentId, page: u32) -> impl Iterator<Item = &Anchor> {
        self.doc_anchors
            .get(doc)
            .into_iter()
            .flatten()
            .filter_map(|a| self.anchors.get(a))
            .filter(move |a| a.page_index == page)
    }

    pub fn anchors_in_doc(&self, doc: &DocumentId) -> impl Iterator<Item = &Anchor> {
        self.doc_anchors.get(doc).into_iter().flatten().filter_map(|a| self.anchors.get(a))
    }

    pub fn inbox(&self) -> Option<&System> {
        self.systems.values().find(|s| s.inbox)
    }

    pub fn confusion_system(&self) -> Option<&System> {
        self.systems.values().find(|s| s.confusion)
    }

    pub fn system_by_hotkey(&self, n: u8) -> Option<&System> {
        self.systems.values().find(|s| s.hotkey == Some(n))
    }

    /// The system whose color an entity wears: lowest hotkey wins, then any
    /// non-inbox system, then the inbox.
    pub fn primary_system(&self, id: &EntityId) -> Option<&System> {
        let mut systems: Vec<&System> = self.systems_of(id).filter_map(|s| self.systems.get(s)).collect();
        systems.sort_by_key(|s| (s.hotkey.unwrap_or(u8::MAX), s.inbox, s.name.clone()));
        systems.into_iter().next()
    }

    pub fn entity_color(&self, id: &EntityId) -> Color {
        if let Some(c) = self.nodes.get(id).and_then(|n| n.color_override) {
            return c;
        }
        match self.primary_system(id) {
            Some(s) if s.hotkey.is_some() => s.color,
            _ => Color::NEUTRAL,
        }
    }

    /// Derived relation state (PRD §4.4 "Three states").
    pub fn relation_state(&self, id: &EntityId) -> Option<RelationState> {
        let r = self.relations.get(id)?;
        let in_system = self.entity_systems.get(id).is_some_and(|s| !s.is_empty());
        let is_endpoint = self.incident.get(id).is_some_and(|s| !s.is_empty());
        Some(if in_system || is_endpoint {
            RelationState::Promoted
        } else if r.has_content() {
            RelationState::Annotated
        } else {
            RelationState::Bare
        })
    }

    /// 0 for nodes; 1 + max endpoint depth for relations.
    pub fn depth(&self, id: &EntityId) -> usize {
        match self.relations.get(id) {
            None => 0,
            Some(r) => 1 + r.endpoint_ids().map(|e| self.depth(e)).max().unwrap_or(0),
        }
    }

    /// Everything that disappears if `id` is deleted (I2), ordered so that
    /// dependents come first: removing in this order never orphans anything.
    pub fn cascade(&self, id: &EntityId) -> Vec<EntityId> {
        fn visit(g: &Graph, id: &EntityId, seen: &mut HashSet<EntityId>, out: &mut Vec<EntityId>) {
            if !seen.insert(id.clone()) {
                return;
            }
            let deps: Vec<EntityId> = g.incident(id).cloned().collect();
            for d in &deps {
                visit(g, d, seen, out);
            }
            out.push(id.clone());
        }
        let mut out = Vec::new();
        if self.contains(id) {
            visit(self, id, &mut HashSet::new(), &mut out);
        }
        out
    }

    /// Entities adjacent to `id`: other endpoints of its relations, the
    /// relations themselves, and (for a relation) its own endpoints.
    pub fn neighbors(&self, id: &EntityId) -> BTreeSet<EntityId> {
        let mut out = BTreeSet::new();
        for r in self.incident(id) {
            out.insert(r.clone());
            if let Some(rel) = self.relations.get(r) {
                out.extend(rel.endpoint_ids().filter(|e| *e != id).cloned());
            }
        }
        if let Some(rel) = self.relations.get(id) {
            out.extend(rel.endpoint_ids().cloned());
        }
        out
    }

    /// Focus mode: the k-hop neighbourhood, counting hops between concepts —
    /// a relation and its endpoints are one hop apart from each other's
    /// neighbours, not two.
    pub fn k_hop(&self, id: &EntityId, k: usize) -> BTreeSet<EntityId> {
        let mut seen = BTreeSet::from([id.clone()]);
        let mut frontier = vec![id.clone()];
        for _ in 0..k {
            let mut next = Vec::new();
            for e in &frontier {
                for n in self.concept_neighbors(e) {
                    if seen.insert(n.clone()) {
                        next.push(n);
                    }
                }
            }
            frontier = next;
        }
        // Include relations connecting members of the neighbourhood.
        let rels: Vec<EntityId> = seen
            .iter()
            .flat_map(|e| self.incident(e))
            .filter(|r| self.relations[*r].endpoint_ids().all(|e| seen.contains(e)))
            .cloned()
            .collect();
        seen.extend(rels);
        seen
    }

    /// Adjacency where a relation is a *line*: the endpoints are neighbours
    /// of each other. Relations that are themselves endpoints are vertices.
    fn concept_neighbors(&self, id: &EntityId) -> Vec<EntityId> {
        let mut out = Vec::new();
        for r in self.incident(id) {
            let rel = &self.relations[r];
            out.extend(rel.endpoint_ids().filter(|e| *e != id).cloned());
            // Promoted relations are reachable from their endpoints.
            if self.incident.get(r).is_some_and(|s| !s.is_empty()) {
                out.push(r.clone());
            }
        }
        if let Some(rel) = self.relations.get(id) {
            out.extend(rel.endpoint_ids().cloned());
        }
        out
    }

    /// Unweighted shortest path length between two entities, treating each
    /// relation as a single hop between its endpoints (acceptance test A3).
    pub fn shortest_path_len(&self, a: &EntityId, b: &EntityId) -> Option<usize> {
        if a == b {
            return Some(0);
        }
        let mut dist: HashMap<EntityId, usize> = HashMap::from([(a.clone(), 0)]);
        let mut q = VecDeque::from([a.clone()]);
        while let Some(e) = q.pop_front() {
            let d = dist[&e];
            for n in self.concept_neighbors(&e) {
                if !dist.contains_key(&n) {
                    if n == *b {
                        return Some(d + 1);
                    }
                    dist.insert(n.clone(), d + 1);
                    q.push_back(n);
                }
            }
        }
        None
    }

    /// The directed source→target subgraph of one relation kind.
    pub fn kind_digraph(&self, kind: &KindId) -> petgraph::graphmap::DiGraphMap<&EntityId, &EntityId> {
        let mut g = petgraph::graphmap::DiGraphMap::new();
        for r in self.relations.values().filter(|r| &r.kind == kind) {
            if let (Some(s), Some(t)) = (r.source(), r.target()) {
                g.add_edge(s, t, &r.id);
            }
        }
        g
    }

    /// Cycles in an acyclic kind (flagged, not blocked — F-REL-9).
    pub fn cycles(&self, kind: &KindId) -> Vec<Vec<EntityId>> {
        let g = self.kind_digraph(kind);
        petgraph::algo::tarjan_scc(&g)
            .into_iter()
            .filter(|c| c.len() > 1 || g.contains_edge(c[0], c[0]))
            .map(|c| c.into_iter().cloned().collect())
            .collect()
    }

    /// Relations of acyclic kinds that participate in a cycle.
    pub fn relations_in_cycles(&self) -> HashSet<EntityId> {
        let mut out = HashSet::new();
        for k in self.kinds.values().filter(|k| k.acyclic) {
            for cycle in self.cycles(&k.id) {
                let members: HashSet<&EntityId> = cycle.iter().collect();
                for r in self.relations.values().filter(|r| r.kind == k.id) {
                    if let (Some(s), Some(t)) = (r.source(), r.target())
                        && members.contains(s)
                        && members.contains(t)
                    {
                        out.insert(r.id.clone());
                    }
                }
            }
        }
        out
    }

    /// Roots of a DAG kind: sources with no incoming edge ("start here").
    pub fn roots(&self, kind: &KindId) -> Vec<EntityId> {
        let g = self.kind_digraph(kind);
        g.nodes()
            .filter(|n| g.neighbors_directed(n, petgraph::Direction::Incoming).next().is_none())
            .cloned()
            .collect()
    }

    pub fn leaves(&self, kind: &KindId) -> Vec<EntityId> {
        let g = self.kind_digraph(kind);
        g.nodes()
            .filter(|n| g.neighbors_directed(n, petgraph::Direction::Outgoing).next().is_none())
            .cloned()
            .collect()
    }

    /// Crosses abstraction levels: both endpoints are leveled nodes and
    /// `|Δlevel| ≥ 1` (§4.6).
    pub fn is_cross_level(&self, rel: &EntityId) -> bool {
        let Some(r) = self.relations.get(rel) else { return false };
        let levels: Vec<i8> =
            r.endpoint_ids().filter_map(|e| self.nodes.get(e).and_then(|n| n.abstraction_level)).collect();
        levels.len() >= 2 && levels.iter().max().unwrap() - levels.iter().min().unwrap() >= 1
    }

    pub fn effective_friction(&self, rel: &EntityId) -> Option<f32> {
        let r = self.relations.get(rel)?;
        r.friction.or_else(|| self.is_cross_level(rel).then_some(DEFAULT_CROSS_LEVEL_FRICTION))
    }

    /// Exact (case-insensitive) title match, for term recognition (F-CAP-10).
    pub fn find_node_by_title(&self, title: &str) -> Option<&Node> {
        let t = title.trim().to_lowercase();
        self.nodes.values().find(|n| n.title.trim().to_lowercase() == t)
    }

    /// Substring search over titles and bodies. The store has FTS for large
    /// corpora; this is for the UI's instant filter.
    pub fn search(&self, query: &str) -> Vec<EntityId> {
        let q = query.to_lowercase();
        let mut hits: Vec<(usize, EntityId)> = self
            .entity_ids()
            .filter_map(|id| {
                let title = self.title(id).to_lowercase();
                if let Some(p) = title.find(&q) {
                    Some((p, id.clone()))
                } else if self.body(id).to_lowercase().contains(&q) {
                    Some((1000, id.clone()))
                } else {
                    None
                }
            })
            .collect();
        hits.sort();
        hits.into_iter().map(|(_, id)| id).collect()
    }

    // ------------------------------------------------------------ integrity

    /// Full consistency check. Used by property tests and `soma-cli verify`.
    pub fn check_invariants(&self) -> std::result::Result<(), Vec<String>> {
        let mut errs = Vec::new();
        for id in self.nodes.keys() {
            if self.relations.contains_key(id) {
                errs.push(format!("{id} is both node and relation"));
            }
        }
        for r in self.relations.values() {
            if !self.kinds.contains_key(&r.kind) {
                errs.push(format!("{}: unknown kind {}", r.id, r.kind));
            }
            for e in r.endpoint_ids() {
                if !self.contains(e) {
                    errs.push(format!("{}: dangling endpoint {e}", r.id));
                }
                if !self.incident.get(e).is_some_and(|s| s.contains(&r.id)) {
                    errs.push(format!("{}: incident index missing {e}", r.id));
                }
            }
            // I3
            let mut stack: Vec<&EntityId> = r.endpoint_ids().collect();
            let mut seen = HashSet::new();
            while let Some(e) = stack.pop() {
                if *e == r.id {
                    errs.push(format!("{}: is transitively its own endpoint", r.id));
                    break;
                }
                if seen.insert(e)
                    && let Some(inner) = self.relations.get(e)
                {
                    stack.extend(inner.endpoint_ids());
                }
            }
            // I4
            if self.depth(&r.id) > MAX_RELATION_DEPTH {
                errs.push(format!("{}: depth exceeds cap", r.id));
            }
        }
        for (e, rels) in &self.incident {
            for r in rels {
                if !self.relations.get(r).is_some_and(|rel| rel.endpoint_ids().any(|x| x == e)) {
                    errs.push(format!("incident index: {r} does not reference {e}"));
                }
            }
        }
        for (s, e) in self.memberships.keys() {
            if !self.systems.contains_key(s) || !self.contains(e) {
                errs.push(format!("dangling membership {e} ∈ {s}"));
            }
        }
        for a in self.anchors.values() {
            if !self.contains(&a.entity) || !self.documents.contains_key(&a.document) {
                errs.push(format!("dangling anchor {}", a.id));
            }
        }
        if errs.is_empty() { Ok(()) } else { Err(errs) }
    }

    /// Undirected topology: the set of `(relation, endpoint)` pairs. I1 says
    /// annotation and promotion never change this.
    pub fn topology(&self) -> BTreeSet<(EntityId, EntityId)> {
        self.relations.values().flat_map(|r| r.endpoint_ids().map(|e| (r.id.clone(), e.clone()))).collect()
    }
}

fn same_id<T: PartialEq + std::fmt::Display>(a: &T, b: &T) -> Result<()> {
    if a == b { Ok(()) } else { Err(CoreError::Invalid(format!("update changes id {a} → {b}"))) }
}

fn validate_level(level: Option<i8>) -> Result<()> {
    match level {
        Some(l) if !(-3..=3).contains(&l) => Err(CoreError::Invalid(format!("abstraction level {l}"))),
        _ => Ok(()),
    }
}

fn validate_hotkey(h: Option<u8>) -> Result<()> {
    match h {
        Some(h) if !(1..=9).contains(&h) => Err(CoreError::Invalid(format!("hotkey {h}"))),
        _ => Ok(()),
    }
}

fn remove_from<K: std::hash::Hash + Eq + Clone, V: Ord>(
    map: &mut HashMap<K, BTreeSet<V>>,
    key: &K,
    value: &V,
) {
    if let Some(set) = map.get_mut(key) {
        set.remove(value);
        if set.is_empty() {
            map.remove(key);
        }
    }
}
