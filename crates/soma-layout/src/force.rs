//! Incremental force-directed layout: Barnes–Hut repulsion, springs whose
//! rest length encodes friction, mild gravity, and pinning.

use crate::{BASE_LENGTH, LayoutGraph, Vec2, eval, seed_position};
use soma_core::EntityId;
use std::collections::{HashMap, HashSet};

const THETA: f32 = 0.8;
const REPULSION: f32 = BASE_LENGTH * BASE_LENGTH * 0.8;
const SPRING: f32 = 0.06;
const GRAVITY: f32 = 0.004;
const DAMPING: f32 = 0.82;
const MAX_STEP: f32 = 40.0;

pub struct ForceLayout {
    pub positions: HashMap<EntityId, Vec2>,
    pub pinned: HashSet<EntityId>,
    velocity: HashMap<EntityId, Vec2>,
    /// Decays towards 0; reheated when the graph changes.
    pub temperature: f32,
}

impl Default for ForceLayout {
    fn default() -> Self {
        Self { positions: HashMap::new(), pinned: HashSet::new(), velocity: HashMap::new(), temperature: 1.0 }
    }
}

impl ForceLayout {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reheat(&mut self, t: f32) {
        self.temperature = self.temperature.max(t);
    }

    pub fn is_settled(&self) -> bool {
        self.temperature < 0.02
    }

    /// Place newcomers next to an already-placed neighbour (so incremental
    /// additions don't jolt the picture) and drop positions of vanished ids.
    pub fn sync(&mut self, lg: &LayoutGraph) {
        let live: HashSet<&EntityId> = lg.ids.iter().collect();
        self.positions.retain(|id, _| live.contains(id));
        self.velocity.retain(|id, _| live.contains(id));
        let mut added = false;
        for (i, id) in lg.ids.iter().enumerate() {
            if self.positions.contains_key(id) {
                continue;
            }
            added = true;
            let neighbour = lg.edges.iter().find_map(|e| {
                let other = if e.a.iter().any(|(j, _)| *j == i) {
                    &e.b
                } else if e.b.iter().any(|(j, _)| *j == i) {
                    &e.a
                } else {
                    return None;
                };
                let pts: Vec<Vec2> =
                    other.iter().filter_map(|(j, _)| self.positions.get(&lg.ids[*j]).copied()).collect();
                (!pts.is_empty())
                    .then(|| pts.iter().fold(Vec2::ZERO, |a, p| a + *p) * (1.0 / pts.len() as f32))
            });
            let p = match neighbour {
                Some(n) => n + seed_position(id, BASE_LENGTH * 0.6),
                None => seed_position(id, BASE_LENGTH * (lg.len() as f32).sqrt() * 0.8),
            };
            self.positions.insert(id.clone(), p);
        }
        if added {
            self.reheat(0.6);
        }
    }

    /// Advance the simulation. Returns true while still moving.
    pub fn step(&mut self, lg: &LayoutGraph, iterations: usize) -> bool {
        self.sync(lg);
        let n = lg.len();
        if n == 0 {
            return false;
        }
        let mut pos: Vec<Vec2> = lg.ids.iter().map(|id| self.positions[id]).collect();
        let mut vel: Vec<Vec2> =
            lg.ids.iter().map(|id| self.velocity.get(id).copied().unwrap_or_default()).collect();
        let pinned: Vec<bool> = lg.ids.iter().map(|id| self.pinned.contains(id)).collect();
        for _ in 0..iterations {
            if self.is_settled() {
                break;
            }
            let mut force = vec![Vec2::ZERO; n];
            // Repulsion via a quadtree.
            let tree = QuadTree::build(&pos);
            for i in 0..n {
                force[i] += tree.repulsion(pos[i], i);
            }
            // Springs, applied to every point of each handle by weight.
            for e in &lg.edges {
                let pa = eval(&e.a, &pos);
                let pb = eval(&e.b, &pos);
                let d = pb - pa;
                let len = d.len().max(0.01);
                let f = d * (SPRING * (len - e.rest) / len);
                for (i, w) in &e.a {
                    force[*i] += f * *w;
                }
                for (i, w) in &e.b {
                    force[*i] += f * -*w;
                }
            }
            let mut moved = 0.0f32;
            for i in 0..n {
                if pinned[i] {
                    vel[i] = Vec2::ZERO;
                    continue;
                }
                force[i] += pos[i] * -GRAVITY;
                vel[i] = (vel[i] + force[i]) * DAMPING;
                let s = vel[i].len();
                let cap = MAX_STEP * self.temperature.max(0.05);
                if s > cap {
                    vel[i] = vel[i] * (cap / s);
                }
                pos[i] += vel[i] * self.temperature.max(0.05);
                moved += vel[i].len();
            }
            let avg = moved / n as f32;
            self.temperature *= if avg < 0.5 { 0.9 } else { 0.99 };
        }
        for (i, id) in lg.ids.iter().enumerate() {
            self.positions.insert(id.clone(), pos[i]);
            self.velocity.insert(id.clone(), vel[i]);
        }
        !self.is_settled()
    }

    /// Run to convergence (for tests and headless use).
    pub fn settle(&mut self, lg: &LayoutGraph, max_iterations: usize) {
        self.reheat(1.0);
        let mut done = 0;
        while done < max_iterations && self.step(lg, 50) {
            done += 50;
        }
    }
}

/// Barnes–Hut quadtree over point masses.
struct QuadTree {
    nodes: Vec<Quad>,
}

struct Quad {
    center: Vec2,
    half: f32,
    mass: f32,
    com: Vec2,
    children: Option<[usize; 4]>,
    point: Option<usize>,
}

impl QuadTree {
    fn build(pos: &[Vec2]) -> QuadTree {
        let (mut lo, mut hi) = (Vec2::new(f32::MAX, f32::MAX), Vec2::new(f32::MIN, f32::MIN));
        for p in pos {
            lo = Vec2::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = Vec2::new(hi.x.max(p.x), hi.y.max(p.y));
        }
        let center = (lo + hi) * 0.5;
        let half = ((hi.x - lo.x).max(hi.y - lo.y) * 0.5).max(1.0) + 1.0;
        let mut t = QuadTree {
            nodes: vec![Quad { center, half, mass: 0.0, com: Vec2::ZERO, children: None, point: None }],
        };
        for (i, p) in pos.iter().enumerate() {
            t.insert(0, i, *p, pos, 0);
        }
        t
    }

    fn insert(&mut self, q: usize, i: usize, p: Vec2, pos: &[Vec2], depth: usize) {
        let m = self.nodes[q].mass;
        self.nodes[q].com = (self.nodes[q].com * m + p) * (1.0 / (m + 1.0));
        self.nodes[q].mass = m + 1.0;
        if depth > 24 {
            return; // coincident points: just accumulate mass
        }
        if let Some(ch) = self.nodes[q].children {
            let c = self.child_for(q, p);
            self.insert(ch[c], i, p, pos, depth + 1);
            return;
        }
        match self.nodes[q].point {
            None if m == 0.0 => self.nodes[q].point = Some(i),
            existing => {
                let (center, half) = (self.nodes[q].center, self.nodes[q].half / 2.0);
                let mut ch = [0; 4];
                for (k, slot) in ch.iter_mut().enumerate() {
                    let off = Vec2::new(
                        if k & 1 == 0 { -half } else { half },
                        if k & 2 == 0 { -half } else { half },
                    );
                    *slot = self.nodes.len();
                    self.nodes.push(Quad {
                        center: center + off,
                        half,
                        mass: 0.0,
                        com: Vec2::ZERO,
                        children: None,
                        point: None,
                    });
                }
                self.nodes[q].children = Some(ch);
                self.nodes[q].point = None;
                if let Some(j) = existing {
                    let c = self.child_for(q, pos[j]);
                    self.insert(ch[c], j, pos[j], pos, depth + 1);
                }
                let c = self.child_for(q, p);
                self.insert(ch[c], i, p, pos, depth + 1);
            }
        }
    }

    fn child_for(&self, q: usize, p: Vec2) -> usize {
        let c = self.nodes[q].center;
        (if p.x >= c.x { 1 } else { 0 }) | (if p.y >= c.y { 2 } else { 0 })
    }

    fn repulsion(&self, p: Vec2, me: usize) -> Vec2 {
        let mut f = Vec2::ZERO;
        let mut stack = vec![0usize];
        while let Some(q) = stack.pop() {
            let node = &self.nodes[q];
            if node.mass == 0.0 || node.point == Some(me) {
                continue;
            }
            let d = p - node.com;
            let dist_sq = d.len_sq().max(1.0);
            let is_leaf = node.children.is_none();
            if is_leaf || (node.half * 2.0) * (node.half * 2.0) < THETA * THETA * dist_sq {
                let dist = dist_sq.sqrt();
                // Coincident points: nudge apart deterministically.
                let dir = if d.len_sq() < 1e-6 {
                    Vec2::new((me % 7) as f32 - 3.0, (me % 5) as f32 - 2.0)
                } else {
                    d
                };
                f += dir * (REPULSION * node.mass / (dist_sq * dist.max(1.0)));
            } else if let Some(ch) = node.children {
                stack.extend(ch);
            }
        }
        f
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::chain;
    use soma_core::{Overlay, commands};

    #[test]
    fn settles_with_linked_nodes_near_and_friction_far() {
        let (mut g, ids) = chain();
        let vis = Overlay::default().resolve(&g);
        let lg = LayoutGraph::build(&g, &vis, None);
        let mut f = ForceLayout::new();
        f.settle(&lg, 3000);
        fn d(f: &ForceLayout, a: &EntityId, b: &EntityId) -> f32 {
            (f.positions[a] - f.positions[b]).len()
        }
        let near = d(&f, &ids[0], &ids[1]);
        assert!(near > 20.0 && near < 3.0 * BASE_LENGTH, "{near}");
        assert!(d(&f, &ids[0], &ids[3]) > near);

        // Setting high friction on B→C lengthens it.
        let bc = g.incident(&ids[2]).find(|r| g.relations[*r].target() == Some(&ids[2])).unwrap().clone();
        let before = d(&f, &ids[1], &ids[2]);
        g.apply(&commands::set_friction(&g, &bc, Some(0.9), 0).unwrap()).unwrap();
        let lg = LayoutGraph::build(&g, &vis, None);
        f.settle(&lg, 3000);
        assert!(d(&f, &ids[1], &ids[2]) > before * 1.4, "{} vs {before}", d(&f, &ids[1], &ids[2]));
    }

    #[test]
    fn pinned_nodes_do_not_move() {
        let (g, ids) = chain();
        let lg = LayoutGraph::build(&g, &Overlay::default().resolve(&g), None);
        let mut f = ForceLayout::new();
        f.sync(&lg);
        f.pinned.insert(ids[0].clone());
        let p0 = f.positions[&ids[0]];
        f.settle(&lg, 500);
        assert_eq!(f.positions[&ids[0]], p0);
    }

    #[test]
    fn scales_to_thousands() {
        let mut lg = LayoutGraph::default();
        for i in 0..3000 {
            let id = EntityId(format!("n{i}"));
            lg.index.insert(id.clone(), i);
            lg.ids.push(id);
        }
        for i in 1..3000 {
            lg.edges.push(crate::Edge {
                a: vec![(i, 1.0)],
                b: vec![((i * 7) % i, 1.0)],
                rest: BASE_LENGTH,
                directed: false,
                layered: false,
            });
        }
        let mut f = ForceLayout::new();
        let t = std::time::Instant::now();
        f.step(&lg, 5);
        assert!(t.elapsed().as_secs_f32() < 5.0);
        assert!(f.positions.values().all(|p| p.x.is_finite() && p.y.is_finite()));
    }
}
