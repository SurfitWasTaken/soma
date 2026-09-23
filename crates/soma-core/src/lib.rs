//! Soma's domain model: entities (nodes and relations), systems, anchors, and
//! the invariants that make relations first-class (PRD §4). No I/O.

pub mod commands;
pub mod defaults;
pub mod graph;
pub mod ids;
pub mod model;
pub mod op;
pub mod overlay;
pub mod snapshot;

pub use graph::{CoreError, Graph, MAX_RELATION_DEPTH};
pub use ids::*;
pub use model::*;
pub use op::{Op, Tx};
pub use overlay::{Combine, Overlay, Visibility};
pub use snapshot::Snapshot;

/// Current wall-clock time in unix milliseconds.
pub fn now_ms() -> Timestamp {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
