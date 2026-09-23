//! Graph layouts for the canvas (PRD §5.5 F-GRAPH-2): incremental
//! Barnes–Hut force layout with friction as spring rest length, Sugiyama
//! layered layout for DAG kinds, and radial layout around a focus.
//!
//! Layout operates on *points* (nodes, and relations that are drawn as
//! lozenges). An edge endpoint is a weighted combination of points, so an
//! edge that terminates on a relation's lozenge pulls on that relation's
//! endpoints instead of on a fake vertex — the layout never sees a concept
//! that isn't there.

mod force;
mod layered;
mod radial;

pub use force::ForceLayout;
pub use layered::layered;
pub use radial::radial;

use soma_core::{EntityId, Graph, Visibility};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
    pub fn len(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
    pub fn len_sq(self) -> f32 {
        self.x * self.x + self.y * self.y
    }
}

impl std::ops::Add for Vec2 {
    type Output = Vec2;
    fn add(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x + o.x, self.y + o.y)
    }
}
impl std::ops::Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}
impl std::ops::Mul<f32> for Vec2 {
    type Output = Vec2;
    fn mul(self, s: f32) -> Vec2 {
        Vec2::new(self.x * s, self.y * s)
    }
}
impl std::ops::AddAssign for Vec2 {
    fn add_assign(&mut self, o: Vec2) {
        self.x += o.x;
        self.y += o.y;
    }
}

/// A weighted combination of layout points; weights sum to 1.
pub type Handle = Vec<(usize, f32)>;

#[derive(Clone, Debug)]
pub struct Edge {
    pub a: Handle,
    pub b: Handle,
    /// Preferred length.
    pub rest: f32,
    /// Direction matters for layered layout (source above target).
    pub directed: bool,
    /// Participates in the layered hierarchy.
    pub layered: bool,
}

#[derive(Clone, Debug, Default)]
pub struct LayoutGraph {
    pub ids: Vec<EntityId>,
    pub index: HashMap<EntityId, usize>,
    pub edges: Vec<Edge>,
}

/// Base spring length in canvas units.
pub const BASE_LENGTH: f32 = 170.0;

impl LayoutGraph {
    /// Project the visible part of a workspace graph. Nodes (and ghost
    /// endpoints) become points; each relation becomes an edge between its
    /// endpoints' handles. `layered_kind` selects which kind forms the
    /// hierarchy for the layered layout.
    pub fn build(
        g: &Graph,
        vis: &HashMap<EntityId, Visibility>,
        layered_kind: Option<&soma_core::KindId>,
    ) -> LayoutGraph {
        let shown = |id: &EntityId| vis.get(id).is_some_and(|v| *v != Visibility::Hidden);
        let mut lg = LayoutGraph::default();
        for id in g.nodes.keys().filter(|id| shown(id)) {
            lg.index.insert(id.clone(), lg.ids.len());
            lg.ids.push(id.clone());
        }
        let mut memo: HashMap<EntityId, Option<Handle>> = HashMap::new();
        for (id, r) in &g.relations {
            if !shown(id) {
                continue;
            }
            let ends: Vec<Handle> = r.endpoint_ids().filter_map(|e| lg.handle(g, e, &mut memo)).collect();
            if ends.len() < 2 {
                continue;
            }
            let friction = g.effective_friction(id).unwrap_or(0.0);
            let kind = g.kinds.get(&r.kind);
            let directed = kind.is_some_and(|k| k.directed);
            let layered = layered_kind.is_some_and(|k| *k == r.kind);
            // Binary relations are one edge; n-ary ones a star from the first.
            for b in &ends[1..] {
                lg.edges.push(Edge {
                    a: ends[0].clone(),
                    b: b.clone(),
                    rest: BASE_LENGTH * (1.0 + 2.5 * friction),
                    directed,
                    layered,
                });
            }
        }
        lg
    }

    /// The layout handle for an entity: a node's own point, or the midpoint
    /// of a relation's endpoints (recursively).
    fn handle(
        &self,
        g: &Graph,
        id: &EntityId,
        memo: &mut HashMap<EntityId, Option<Handle>>,
    ) -> Option<Handle> {
        if let Some(&i) = self.index.get(id) {
            return Some(vec![(i, 1.0)]);
        }
        if let Some(h) = memo.get(id) {
            return h.clone();
        }
        memo.insert(id.clone(), None); // cycle guard
        let h = g.relations.get(id).and_then(|r| {
            let parts: Vec<Handle> = r.endpoint_ids().filter_map(|e| self.handle(g, e, memo)).collect();
            if parts.is_empty() {
                return None;
            }
            let w = 1.0 / parts.len() as f32;
            let mut acc: HashMap<usize, f32> = HashMap::new();
            for p in parts {
                for (i, pw) in p {
                    *acc.entry(i).or_default() += pw * w;
                }
            }
            let mut h: Handle = acc.into_iter().collect();
            h.sort_by_key(|(i, _)| *i);
            Some(h)
        });
        memo.insert(id.clone(), h.clone());
        h
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

pub fn eval(h: &Handle, pos: &[Vec2]) -> Vec2 {
    h.iter().fold(Vec2::ZERO, |acc, (i, w)| acc + pos[*i] * *w)
}

/// Stable pseudo-random initial position from an id, so layouts are
/// deterministic.
pub fn seed_position(id: &EntityId, radius: f32) -> Vec2 {
    let h = id.0.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    let a = (h & 0xffff) as f32 / 65535.0 * std::f32::consts::TAU;
    let r = ((h >> 16) & 0xffff) as f32 / 65535.0 * radius;
    Vec2::new(a.cos() * r, a.sin() * r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use soma_core::commands::{self, NewNode};
    use soma_core::defaults::{self, PREREQUISITE_OF, UNCLEAR_LINK};

    pub fn chain() -> (Graph, Vec<EntityId>) {
        let mut g = Graph::new();
        g.apply(&defaults::seed()).unwrap();
        let mut ids = Vec::new();
        for t in ["A", "B", "C", "D", "E"] {
            let (tx, id) = commands::create_node(
                &g,
                NewNode { title: t.into(), body: String::new(), systems: vec![], anchors: vec![] },
                0,
            );
            g.apply(&tx).unwrap();
            ids.push(id);
        }
        for w in [(0, 1), (1, 2), (2, 3), (0, 4)] {
            let (tx, _) =
                commands::link(&g, &ids[w.0], &ids[w.1], &soma_core::KindId::from(PREREQUISITE_OF), 0)
                    .unwrap();
            g.apply(&tx).unwrap();
        }
        (g, ids)
    }

    #[test]
    fn relation_on_relation_pulls_on_midpoint() {
        let (mut g, ids) = chain();
        let ab = g.incident(&ids[0]).next().unwrap().clone();
        let (tx, _) = commands::link(&g, &ids[3], &ab, &soma_core::KindId::from(UNCLEAR_LINK), 0).unwrap();
        g.apply(&tx).unwrap();
        let vis = soma_core::Overlay::default().resolve(&g);
        let lg = LayoutGraph::build(&g, &vis, None);
        assert_eq!(lg.len(), 5);
        assert_eq!(lg.edges.len(), 5);
        let e = lg.edges.iter().find(|e| e.b.len() == 2 || e.a.len() == 2).unwrap();
        let h = if e.b.len() == 2 { &e.b } else { &e.a };
        assert!(h.iter().all(|(_, w)| (*w - 0.5).abs() < 1e-6));
    }
}
