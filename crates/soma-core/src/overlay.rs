//! Overlay composition (PRD §4.5, F-OVL-1..3) and ghost endpoints (F-GRAPH-4).

use crate::commands::is_highlight_carrier;
use crate::graph::Graph;
use crate::ids::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

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
    pub fn selected(&self, g: &Graph) -> Option<HashSet<EntityId>> {
        let first = self.active.first()?;
        let sets: Vec<HashSet<&EntityId>> = self.active.iter().map(|s| g.members_of(s).collect()).collect();
        let base: HashSet<&EntityId> = g.members_of(first).collect();
        let out: HashSet<&EntityId> = match self.mode {
            Combine::Union => sets.iter().flatten().copied().collect(),
            Combine::Intersection => {
                base.into_iter().filter(|e| sets.iter().all(|s| s.contains(e))).collect()
            }
            Combine::Difference => {
                base.into_iter().filter(|e| !sets[1..].iter().any(|s| s.contains(e))).collect()
            }
        };
        Some(out.into_iter().cloned().collect())
    }

    /// Visibility of every entity (highlight carriers excluded).
    pub fn resolve(&self, g: &Graph) -> HashMap<EntityId, Visibility> {
        let entities = g.entity_ids().filter(|e| !is_highlight_carrier(e));
        let Some(selected) = self.selected(g) else {
            return entities.map(|e| (e.clone(), Visibility::Full)).collect();
        };
        let mut vis: HashMap<EntityId, Visibility> = HashMap::new();
        let off = if self.ghost { Visibility::Ghost } else { Visibility::Hidden };
        for e in entities {
            vis.insert(e.clone(), if selected.contains(e) { Visibility::Full } else { off });
        }
        // Relations between fully visible entities are visible even when the
        // relation itself is unfiled; iterate by depth so relation-on-relation
        // chains resolve.
        let mut rels: Vec<&EntityId> = g.relations.keys().collect();
        rels.sort_by_key(|r| g.depth(r));
        for r in rels {
            if vis[r] != Visibility::Full
                && g.relations[r].endpoint_ids().all(|e| vis.get(e) == Some(&Visibility::Full))
            {
                vis.insert(r.clone(), Visibility::Full);
            }
        }
        // I5: filed relations keep their endpoints as stubs.
        let full_rels: Vec<EntityId> =
            g.relations.keys().filter(|r| vis[*r] == Visibility::Full).cloned().collect();
        for r in full_rels {
            for e in g.relations[&r].endpoint_ids() {
                if let Some(v) = vis.get_mut(e)
                    && *v != Visibility::Full
                {
                    *v = Visibility::GhostEndpoint;
                }
            }
        }
        vis
    }
}
