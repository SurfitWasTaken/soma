//! Shipped relation kinds (PRD §4.4) and starter systems.

use crate::ids::*;
use crate::model::*;
use crate::op::{Op, Tx};

pub const PREREQUISITE_OF: &str = "prerequisite-of";
pub const ELABORATES: &str = "elaborates";
pub const CONTRADICTS: &str = "contradicts";
pub const SAME_AS: &str = "same-as";
pub const INSTANCE_OF: &str = "instance-of";
pub const DEPENDS_ON: &str = "depends-on";
pub const UNCLEAR_LINK: &str = "unclear-link";

/// Palette order when a system declares no preference.
pub const KIND_ORDER: &[&str] =
    &[PREREQUISITE_OF, UNCLEAR_LINK, ELABORATES, CONTRADICTS, SAME_AS, INSTANCE_OF, DEPENDS_ON];

pub const INBOX_ID: &str = "inbox";

pub fn kinds() -> Vec<RelationKind> {
    let k = |id: &str, directed, acyclic, stroke, color, joins_confusion| RelationKind {
        id: KindId::from(id),
        name: id.to_owned(),
        directed,
        acyclic,
        stroke,
        color,
        joins_confusion,
    };
    vec![
        k(PREREQUISITE_OF, true, true, Stroke::Solid, Color::rgb(0x5b, 0x8d, 0xef), false),
        k(ELABORATES, true, false, Stroke::Dotted, Color::rgb(0x7a, 0xb8, 0x7a), false),
        k(CONTRADICTS, false, false, Stroke::Wavy, Color::rgb(0xe0, 0x5a, 0x5a), false),
        k(SAME_AS, false, false, Stroke::Double, Color::rgb(0x9a, 0x9a, 0x9a), false),
        k(INSTANCE_OF, true, true, Stroke::Dashed, Color::rgb(0xb0, 0x88, 0xd8), false),
        k(DEPENDS_ON, true, false, Stroke::Solid, Color::rgb(0x88, 0x99, 0xaa), false),
        k(UNCLEAR_LINK, false, false, Stroke::Dashed, Color::rgb(0xf0, 0xa0, 0x30), true),
    ]
}

pub fn systems() -> Vec<System> {
    let s = |id: &str, name: &str, color, hotkey, kinds: &[&str], confusion, inbox| System {
        id: SystemId::from(id),
        name: name.to_owned(),
        description: String::new(),
        color,
        default_relation_kinds: kinds.iter().map(|k| KindId::from(*k)).collect(),
        default_layout: if kinds.first() == Some(&PREREQUISITE_OF) {
            LayoutKind::Layered
        } else {
            LayoutKind::Force
        },
        hotkey,
        confusion,
        inbox,
    };
    vec![
        s(
            "lapse",
            "lapse in understanding",
            Color::rgb(0xf2, 0x9e, 0x4c),
            Some(1),
            &[PREREQUISITE_OF, UNCLEAR_LINK, DEPENDS_ON, CONTRADICTS],
            true,
            false,
        ),
        s(
            "terminology",
            "terminology",
            Color::rgb(0x5b, 0xa7, 0xe8),
            Some(2),
            &[SAME_AS, INSTANCE_OF, ELABORATES, CONTRADICTS],
            false,
            false,
        ),
        s(
            "proofs",
            "proof obligations",
            Color::rgb(0xb0, 0x7c, 0xe0),
            Some(3),
            &[PREREQUISITE_OF, DEPENDS_ON, ELABORATES, UNCLEAR_LINK],
            false,
            false,
        ),
        s(
            "open",
            "open questions",
            Color::rgb(0x6c, 0xc4, 0x7e),
            Some(4),
            &[UNCLEAR_LINK, CONTRADICTS, ELABORATES, DEPENDS_ON],
            false,
            false,
        ),
        s(INBOX_ID, "inbox", Color::NEUTRAL, None, &[], false, true),
    ]
}

/// Seeds a fresh workspace. Not undoable (the store applies it outside the log).
pub fn seed() -> Tx {
    let mut tx = Tx::new("seed");
    for k in kinds() {
        tx.push(Op::AddKind(k));
    }
    for s in systems() {
        tx.push(Op::AddSystem(s));
    }
    tx
}
