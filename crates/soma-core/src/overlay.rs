//! Overlay composition (PRD §4.5, F-OVL-1..3) and ghost endpoints (F-GRAPH-4).

use crate::commands::is_highlight_carrier;
use crate::graph::Graph;
use crate::ids::*;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Combine {
    #[default]
    Union,
    Intersection,
    /// First active system minus all the others.
    Difference,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Visibility {
    Full,
    /// Outside the overlay, drawn at 12% opacity (ghost mode).
    Ghost,
    /// Outside the overlay but an endpoint of a visible relation: drawn as a
    /// small labeled stub so the relation is never orphaned (I5).
    GhostEndpoint,
    Hidden,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Overlay {
    pub active: Vec<SystemId>,
    pub mode: Combine,
    pub ghost: bool,
}

impl Default for Overlay {
    fn default() -> Self {
        Self { active: Vec::new(), mode: Combine::Union, ghost: true }
    }
}

impl Overlay {
    pub fn toggle(&mut self, s: &SystemId) {
        if let Some(i) = self.active.iter().position(|x| x == s) {
            self.active.remove(i);
        } else {
            self.active.push(s.clone());
        }
    }

    pub fn is_active(&self, s: &SystemId) -> bool {
        self.active.contains(s)
    }

    /// Entities selected by the system combination alone.
    pub fn selected<'g>(&self, g: &'g Graph) -> Option<FxHashSet<&'g EntityId>> {
        let first = self.active.first()?;
        let rest: Vec<FxHashSet<&EntityId>> =
            self.active[1..].iter().map(|s| g.members_of(s).collect()).collect();
        let base = g.members_of(first);
        Some(match self.mode {
            Combine::Union => {
                let mut out: FxHashSet<&EntityId> = base.collect();
                out.extend(rest.into_iter().flatten());
                out
            }
            Combine::Intersection => base.filter(|e| rest.iter().all(|s| s.contains(e))).collect(),
            Combine::Difference => base.filter(|e| !rest.iter().any(|s| s.contains(e))).collect(),
        })
    }

    /// Visibility of every entity (highlight carriers excluded).
    pub fn resolve(&self, g: &Graph) -> HashMap<EntityId, Visibility> {
        self.resolve_ref(g).into_iter().map(|(k, v)| (k.clone(), v)).collect()
    }

    /// As [`Overlay::resolve`], keyed by reference into the graph.
    pub fn resolve_ref<'g>(&self, g: &'g Graph) -> FxHashMap<&'g EntityId, Visibility> {
        let entities = g.entity_ids().filter(|e| !is_highlight_carrier(e));
        let Some(selected) = self.selected(g) else {
            return entities.map(|e| (e, Visibility::Full)).collect();
        };
        let off = if self.ghost { Visibility::Ghost } else { Visibility::Hidden };
        let mut vis: FxHashMap<&EntityId, Visibility> =
            entities.map(|e| (e, if selected.contains(e) { Visibility::Full } else { off })).collect();
        // Relations between fully visible entities are visible even when the
        // relation itself is unfiled. Relation-on-relation chains need one
        // more pass per nesting level; stop when nothing changes.
        loop {
            let mut changed = false;
            for (id, r) in &g.relations {
                if vis.get(id) != Some(&Visibility::Full)
                    && r.endpoint_ids().all(|e| vis.get(e) == Some(&Visibility::Full))
                {
                    vis.insert(id, Visibility::Full);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        // I5: filed relations keep their endpoints as stubs.
        let stubs: Vec<&EntityId> = g
            .relations
            .iter()
            .filter(|(id, _)| vis.get(id) == Some(&Visibility::Full))
            .flat_map(|(_, r)| r.endpoint_ids())
            .filter(|e| vis.get(e).is_some_and(|v| *v != Visibility::Full))
            .collect();
        for e in stubs {
            vis.insert(e, Visibility::GhostEndpoint);
        }
        vis
    }
}
