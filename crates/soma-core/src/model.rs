//! Domain records (PRD §4). These are plain data; all invariants are enforced
//! by [`crate::Graph::apply`].

use crate::ids::*;
use serde::{Deserialize, Serialize};

/// Unix milliseconds.
pub type Timestamp = i64;

/// Packed `0xRRGGBBAA`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Color(pub u32);

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self(((r as u32) << 24) | ((g as u32) << 16) | ((b as u32) << 8) | 0xff)
    }
    pub fn r(self) -> u8 {
        (self.0 >> 24) as u8
    }
    pub fn g(self) -> u8 {
        (self.0 >> 16) as u8
    }
    pub fn b(self) -> u8 {
        (self.0 >> 8) as u8
    }
    pub fn a(self) -> u8 {
        self.0 as u8
    }
    /// Neutral gray used for highlights of systems without a hotkey (PRD §11 Q1).
    pub const NEUTRAL: Color = Color::rgb(0xa0, 0xa0, 0xa0);
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Document {
    pub id: DocumentId,
    pub path: String,
    pub title: Option<String>,
    pub page_count: u32,
    pub copied_local: bool,
    pub added_at: Timestamp,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Node {
    pub id: EntityId,
    pub title: String,
    pub body: String,
    /// -3..=3, user-defined ladder (PRD §4.6).
    pub abstraction_level: Option<i8>,
    pub color_override: Option<Color>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Source,
    Target,
    Member,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Source => "source",
            Role::Target => "target",
            Role::Member => "member",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "source" => Role::Source,
            "target" => Role::Target,
            "member" => Role::Member,
            _ => return None,
        })
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Endpoint {
    /// An entity — therefore possibly another relation (PRD §4.4 decision a).
    pub entity: EntityId,
    pub role: Role,
    pub ordinal: u32,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Relation {
    pub id: EntityId,
    pub kind: KindId,
    /// Ordered. The v1 UI always creates exactly one source and one target.
    pub endpoints: Vec<Endpoint>,
    /// 0.0..=1.0 switching cost across this link.
    pub friction: Option<f32>,
    /// `Some(_)` ⇒ the relation is annotated.
    pub title: Option<String>,
    pub body: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Relation {
    pub fn has_content(&self) -> bool {
        self.title.as_deref().is_some_and(|t| !t.is_empty())
            || self.body.as_deref().is_some_and(|b| !b.is_empty())
    }

    pub fn source(&self) -> Option<&EntityId> {
        self.endpoints.iter().find(|e| e.role == Role::Source).map(|e| &e.entity)
    }

    pub fn target(&self) -> Option<&EntityId> {
        self.endpoints.iter().find(|e| e.role == Role::Target).map(|e| &e.entity)
    }

    pub fn endpoint_ids(&self) -> impl Iterator<Item = &EntityId> {
        self.endpoints.iter().map(|e| &e.entity)
    }

    /// The other endpoint of a binary relation.
    pub fn other(&self, from: &EntityId) -> Option<&EntityId> {
        self.endpoint_ids().find(|e| *e != from)
    }
}

/// Derived — never stored (invariant I1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RelationState {
    /// A line.
    Bare,
    /// A line with a midpoint chip.
    Annotated,
    /// A lozenge inline on the edge; other relations may attach to it.
    Promoted,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stroke {
    Solid,
    Dashed,
    Dotted,
    Double,
    Wavy,
}

impl Stroke {
    pub fn as_str(self) -> &'static str {
        match self {
            Stroke::Solid => "solid",
            Stroke::Dashed => "dashed",
            Stroke::Dotted => "dotted",
            Stroke::Double => "double",
            Stroke::Wavy => "wavy",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "solid" => Stroke::Solid,
            "dashed" => Stroke::Dashed,
            "dotted" => Stroke::Dotted,
            "double" => Stroke::Double,
            "wavy" => Stroke::Wavy,
            _ => return None,
        })
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct RelationKind {
    pub id: KindId,
    pub name: String,
    pub directed: bool,
    /// Cycles are flagged, not blocked.
    pub acyclic: bool,
    pub stroke: Stroke,
    pub color: Color,
    /// New relations of this kind join the workspace's confusion system.
    pub joins_confusion: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayoutKind {
    Force,
    Layered,
    Radial,
    Manual,
}

impl LayoutKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LayoutKind::Force => "force",
            LayoutKind::Layered => "layered",
            LayoutKind::Radial => "radial",
            LayoutKind::Manual => "manual",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "force" => LayoutKind::Force,
            "layered" => LayoutKind::Layered,
            "radial" => LayoutKind::Radial,
            "manual" => LayoutKind::Manual,
            _ => return None,
        })
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct System {
    pub id: SystemId,
    pub name: String,
    pub description: String,
    pub color: Color,
    pub default_relation_kinds: Vec<KindId>,
    pub default_layout: LayoutKind,
    /// 1..=9 → `Ctrl-<n>` in the reader.
    pub hotkey: Option<u8>,
    /// `unclear-link` relations auto-join this system.
    pub confusion: bool,
    /// The implicit home of unfiled nodes (PRD §11 Q2).
    pub inbox: bool,
    /// The question asked when something is filed here, e.g. "What exactly
    /// don't you follow?".
    #[serde(default)]
    pub note_prompt: String,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Membership {
    pub system: SystemId,
    pub entity: EntityId,
    pub added_at: Timestamp,
    /// Why this entity is in this system — e.g. under *lapse in
    /// understanding*, what exactly isn't understood. One note per system,
    /// so the same highlight can say different things in each.
    #[serde(default)]
    pub note: String,
}

impl Membership {
    pub fn new(system: SystemId, entity: EntityId, added_at: Timestamp) -> Self {
        Self { system, entity, added_at, note: String::new() }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnchorKind {
    Text,
    Region,
    Object,
}

impl AnchorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AnchorKind::Text => "text",
            AnchorKind::Region => "region",
            AnchorKind::Object => "object",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "text" => AnchorKind::Text,
            "region" => AnchorKind::Region,
            "object" => AnchorKind::Object,
            _ => return None,
        })
    }
}

/// Page-space quadrilateral (PDF points, origin top-left, y down).
/// Corners are in order: top-left, top-right, bottom-right, bottom-left.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct Quad(pub [[f32; 2]; 4]);

impl Quad {
    pub fn from_rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Quad([[x0, y0], [x1, y0], [x1, y1], [x0, y1]])
    }

    pub fn bounds(&self) -> [f32; 4] {
        let xs = self.0.map(|p| p[0]);
        let ys = self.0.map(|p| p[1]);
        let min = |v: [f32; 4]| v.into_iter().fold(f32::INFINITY, f32::min);
        let max = |v: [f32; 4]| v.into_iter().fold(f32::NEG_INFINITY, f32::max);
        [min(xs), min(ys), max(xs), max(ys)]
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        let [x0, y0, x1, y1] = self.bounds();
        x >= x0 && x <= x1 && y >= y0 && y <= y1
    }

    pub fn encode(quads: &[Quad]) -> Vec<u8> {
        quads.iter().flat_map(|q| q.0.iter().flatten().flat_map(|f| f.to_le_bytes())).collect()
    }

    pub fn decode(bytes: &[u8]) -> Vec<Quad> {
        bytes
            .chunks_exact(32)
            .map(|c| {
                let f = |i: usize| f32::from_le_bytes(c[i * 4..i * 4 + 4].try_into().unwrap());
                Quad([[f(0), f(1)], [f(2), f(3)], [f(4), f(5)], [f(6), f(7)]])
            })
            .collect()
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Anchor {
    pub id: AnchorId,
    pub entity: EntityId,
    pub document: DocumentId,
    pub page_index: u32,
    pub kind: AnchorKind,
    pub quads: Vec<Quad>,
    pub exact: String,
    pub prefix: String,
    pub suffix: String,
    pub char_start: Option<u32>,
    pub char_end: Option<u32>,
    /// 1.0 at capture; recomputed on re-anchor. 0.0 ⇒ detached.
    pub confidence: f32,
    /// Highlight color; `None` → derive from the entity's primary system.
    pub color: Option<Color>,
}

impl Anchor {
    pub fn is_detached(&self) -> bool {
        self.confidence <= 0.0
    }
}
