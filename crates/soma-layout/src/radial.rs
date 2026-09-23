//! Radial layout: the focus at the centre, k-hop rings around it, each
//! subtree given an angular wedge proportional to its size.

use crate::{BASE_LENGTH, LayoutGraph, Vec2, layered::NODE_GAP};
use soma_core::EntityId;
use std::collections::HashMap;

pub fn radial(lg: &LayoutGraph, focus: &EntityId) -> HashMap<EntityId, Vec2> {
    let n = lg.len();
    let mut out = HashMap::new();
    let Some(&root) = lg.index.get(focus) else { return out };
    let mut adj: Vec<Vec<usize>> = vec![vec![]; n];
    for e in &lg.edges {
        for (a, _) in &e.a {
            for (b, _) in &e.b {
                if a != b {
                    adj[*a].push(*b);
                    adj[*b].push(*a);
                }
            }
        }
    }
    for a in &mut adj {
        a.sort_unstable();
        a.dedup();
    }
    // BFS tree.
    let mut parent = vec![usize::MAX; n];
    let mut depth = vec![usize::MAX; n];
    let mut order = vec![root];
    depth[root] = 0;
    let mut i = 0;
    while i < order.len() {
        let v = order[i];
        i += 1;
        for &w in &adj[v] {
            if depth[w] == usize::MAX {
                depth[w] = depth[v] + 1;
                parent[w] = v;
                order.push(w);
            }
        }
    }
    let mut children: Vec<Vec<usize>> = vec![vec![]; n];
    for &v in &order[1..] {
        children[parent[v]].push(v);
    }
    let mut size = vec![1usize; n];
    for &v in order.iter().rev() {
        if parent[v] != usize::MAX {
            size[parent[v]] += size[v];
        }
    }
    // Assign wedges.
    let mut wedge = vec![(0.0f32, std::f32::consts::TAU); n];
    for &v in &order {
        let (start, span) = wedge[v];
        let total: usize = children[v].iter().map(|c| size[*c]).sum();
        let mut a = start;
        for &c in &children[v] {
            let s = span * size[c] as f32 / total.max(1) as f32;
            wedge[c] = (a, s);
            a += s;
        }
        let r = depth[v] as f32 * BASE_LENGTH * 1.5;
        let theta = start + span / 2.0;
        out.insert(
            lg.ids[v].clone(),
            if depth[v] == 0 { Vec2::ZERO } else { Vec2::new(theta.cos() * r, theta.sin() * r) },
        );
    }
    // Unreachable points: an outer ring.
    let far: Vec<usize> = (0..n).filter(|&v| depth[v] == usize::MAX).collect();
    let max_depth = order.iter().map(|&v| depth[v]).max().unwrap_or(0);
    let r = (max_depth as f32 + 2.0) * BASE_LENGTH * 1.5;
    let r = r.max(far.len() as f32 * NODE_GAP / std::f32::consts::TAU);
    for (k, &v) in far.iter().enumerate() {
        let t = k as f32 / far.len() as f32 * std::f32::consts::TAU;
        out.insert(lg.ids[v].clone(), Vec2::new(t.cos() * r, t.sin() * r));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::chain;
    use soma_core::Overlay;

    #[test]
    fn rings_by_hop_distance() {
        let (g, ids) = chain();
        let lg = LayoutGraph::build(&g, &Overlay::default().resolve(&g), None);
        let pos = radial(&lg, &ids[0]);
        assert_eq!(pos[&ids[0]], Vec2::ZERO);
        let r1 = pos[&ids[1]].len();
        let r2 = pos[&ids[2]].len();
        assert!((pos[&ids[4]].len() - r1).abs() < 1e-3);
        assert!(r2 > r1);
    }
}
