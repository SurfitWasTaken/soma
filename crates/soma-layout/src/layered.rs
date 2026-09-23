//! Sugiyama-style layered layout for DAG kinds such as `prerequisite-of`, so
//! prerequisite chains read top to bottom instead of as a hairball.
//!
//! Steps: break cycles (reverse DFS back edges) → longest-path layering →
//! barycentric crossing reduction → even spacing per layer. Points not in
//! the hierarchy are placed in a grid beneath it.

use crate::{BASE_LENGTH, LayoutGraph, Vec2};
use soma_core::EntityId;
use std::collections::HashMap;

pub const LAYER_GAP: f32 = BASE_LENGTH * 1.2;
pub const NODE_GAP: f32 = BASE_LENGTH * 1.6;

/// The dominant point of a handle (edges onto lozenges attach to the
/// heavier endpoint for layering purposes).
fn main_point(h: &[(usize, f32)]) -> usize {
    h.iter().max_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| *i).unwrap_or(0)
}

pub fn layered(lg: &LayoutGraph) -> HashMap<EntityId, Vec2> {
    let n = lg.len();
    let mut out = HashMap::new();
    if n == 0 {
        return out;
    }
    let mut succ: Vec<Vec<usize>> = vec![vec![]; n];
    let mut in_dag = vec![false; n];
    for e in lg.edges.iter().filter(|e| e.layered) {
        let (a, b) = (main_point(&e.a), main_point(&e.b));
        if a != b && !succ[a].contains(&b) {
            succ[a].push(b);
            in_dag[a] = true;
            in_dag[b] = true;
        }
    }

    // 1. Break cycles: iterative DFS, drop back edges.
    let mut state = vec![0u8; n]; // 0 new, 1 on stack, 2 done
    for root in 0..n {
        if state[root] != 0 {
            continue;
        }
        let mut stack = vec![(root, 0usize)];
        state[root] = 1;
        while let Some((v, i)) = stack.pop() {
            if i < succ[v].len() {
                stack.push((v, i + 1));
                let w = succ[v][i];
                match state[w] {
                    0 => {
                        state[w] = 1;
                        stack.push((w, 0));
                    }
                    1 => succ[v][i] = usize::MAX, // back edge
                    _ => {}
                }
            } else {
                state[v] = 2;
            }
        }
    }
    for s in &mut succ {
        s.retain(|&w| w != usize::MAX);
    }

    // 2. Longest-path layering (roots at layer 0).
    let mut pred_count = vec![0usize; n];
    for s in &succ {
        for &w in s {
            pred_count[w] += 1;
        }
    }
    let mut layer = vec![0usize; n];
    let mut queue: Vec<usize> = (0..n).filter(|&v| in_dag[v] && pred_count[v] == 0).collect();
    let mut qi = 0;
    while qi < queue.len() {
        let v = queue[qi];
        qi += 1;
        for &w in &succ[v] {
            layer[w] = layer[w].max(layer[v] + 1);
            pred_count[w] -= 1;
            if pred_count[w] == 0 {
                queue.push(w);
            }
        }
    }
    let depth = queue.iter().map(|&v| layer[v]).max().unwrap_or(0);
    let mut layers: Vec<Vec<usize>> = vec![vec![]; depth + 1];
    for &v in &queue {
        layers[layer[v]].push(v);
    }

    // 3. Crossing reduction: alternate down/up barycenter sweeps.
    let mut preds: Vec<Vec<usize>> = vec![vec![]; n];
    for (v, s) in succ.iter().enumerate() {
        for &w in s {
            preds[w].push(v);
        }
    }
    let mut order = vec![0f32; n];
    for l in &layers {
        for (i, &v) in l.iter().enumerate() {
            order[v] = i as f32;
        }
    }
    for sweep in 0..8 {
        let down = sweep % 2 == 0;
        let range: Vec<usize> = if down {
            (1..layers.len()).collect()
        } else {
            (0..layers.len().saturating_sub(1)).rev().collect()
        };
        for li in range {
            let bary = |v: usize| {
                let adj = if down { &preds[v] } else { &succ[v] };
                if adj.is_empty() {
                    order[v]
                } else {
                    adj.iter().map(|&u| order[u]).sum::<f32>() / adj.len() as f32
                }
            };
            let mut keyed: Vec<(f32, usize)> = layers[li].iter().map(|&v| (bary(v), v)).collect();
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            layers[li] = keyed.into_iter().map(|(_, v)| v).collect();
            for (i, &v) in layers[li].iter().enumerate() {
                order[v] = i as f32;
            }
        }
    }

    // 4. Coordinates.
    for (li, l) in layers.iter().enumerate() {
        let width = (l.len().saturating_sub(1)) as f32 * NODE_GAP;
        for (i, &v) in l.iter().enumerate() {
            out.insert(
                lg.ids[v].clone(),
                Vec2::new(i as f32 * NODE_GAP - width / 2.0, li as f32 * LAYER_GAP),
            );
        }
    }
    let loose: Vec<usize> = (0..n).filter(|&v| !in_dag[v]).collect();
    let cols = (loose.len() as f32).sqrt().ceil().max(1.0) as usize;
    let top = (layers.len() as f32 + if queue.is_empty() { 0.0 } else { 1.0 }) * LAYER_GAP;
    for (k, &v) in loose.iter().enumerate() {
        let (r, c) = (k / cols, k % cols);
        let width = (cols.saturating_sub(1)) as f32 * NODE_GAP * 0.8;
        out.insert(
            lg.ids[v].clone(),
            Vec2::new(c as f32 * NODE_GAP * 0.8 - width / 2.0, top + r as f32 * LAYER_GAP * 0.7),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::chain;
    use soma_core::{KindId, Overlay, defaults::PREREQUISITE_OF};

    #[test]
    fn prerequisites_read_top_to_bottom() {
        let (g, ids) = chain();
        let lg =
            LayoutGraph::build(&g, &Overlay::default().resolve(&g), Some(&KindId::from(PREREQUISITE_OF)));
        let pos = layered(&lg);
        // A → B → C → D and A → E
        assert!(pos[&ids[0]].y < pos[&ids[1]].y);
        assert!(pos[&ids[1]].y < pos[&ids[2]].y);
        assert!(pos[&ids[2]].y < pos[&ids[3]].y);
        assert_eq!(pos[&ids[1]].y, pos[&ids[4]].y);
    }

    #[test]
    fn cycles_do_not_hang() {
        let (mut g, ids) = chain();
        let (tx, _) =
            soma_core::commands::link(&g, &ids[3], &ids[0], &KindId::from(PREREQUISITE_OF), 0).unwrap();
        g.apply(&tx).unwrap();
        let lg =
            LayoutGraph::build(&g, &Overlay::default().resolve(&g), Some(&KindId::from(PREREQUISITE_OF)));
        assert_eq!(layered(&lg).len(), 5);
    }
}
