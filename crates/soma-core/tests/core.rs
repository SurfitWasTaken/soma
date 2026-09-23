use soma_core::commands::{self, NewNode};
use soma_core::defaults::{self, PREREQUISITE_OF, UNCLEAR_LINK};
use soma_core::*;

fn seeded() -> Graph {
    let mut g = Graph::new();
    g.apply(&defaults::seed()).unwrap();
    g
}

macro_rules! run {
    ($g:ident, $e:expr $(,)?) => {{
        let tx = $e;
        $g.apply(&tx).unwrap();
        tx
    }};
}

fn apply(g: &mut Graph, tx: Tx) -> Tx {
    g.apply(&tx).unwrap();
    tx
}

fn node(g: &mut Graph, title: &str) -> EntityId {
    let (tx, id) = commands::create_node(
        g,
        NewNode { title: title.into(), body: String::new(), systems: vec![], anchors: vec![] },
        1,
    );
    apply(g, tx);
    id
}

fn link(g: &mut Graph, a: &EntityId, b: &EntityId, kind: &str) -> EntityId {
    let (tx, id) = commands::link(g, a, b, &KindId::from(kind), 2).unwrap();
    apply(g, tx);
    id
}

#[test]
fn unfiled_nodes_land_in_inbox_and_leave_it_when_filed() {
    let mut g = seeded();
    let a = node(&mut g, "A");
    let inbox = SystemId::from(defaults::INBOX_ID);
    let lapse = SystemId::from("lapse");
    assert!(g.in_system(&a, &inbox));
    run!(g, commands::set_membership(&g, &a, &lapse, true, 3).unwrap());
    assert!(g.in_system(&a, &lapse) && !g.in_system(&a, &inbox));
    run!(g, commands::set_membership(&g, &a, &lapse, false, 4).unwrap());
    assert!(g.in_system(&a, &inbox));
}

/// Acceptance test A3.
#[test]
fn a3_reified_relation_keeps_path_length_one() {
    let mut g = seeded();
    let a = node(&mut g, "quadratic variation");
    let b = node(&mut g, "Itô isometry");
    let c = node(&mut g, "dominated convergence");
    let ab = link(&mut g, &a, &b, PREREQUISITE_OF);
    let topo = g.topology();
    assert_eq!(g.relation_state(&ab), Some(RelationState::Bare));

    run!(g, commands::annotate(&g, &ab, "why does the limit exist?", "I see the algebra", 3).unwrap());
    assert_eq!(g.relation_state(&ab), Some(RelationState::Annotated));
    assert_eq!(g.topology(), topo, "I1: annotation must not change topology");

    run!(g, commands::set_membership(&g, &ab, &SystemId::from("lapse"), true, 4).unwrap());
    assert_eq!(g.relation_state(&ab), Some(RelationState::Promoted));
    assert_eq!(g.topology(), topo, "I1: promotion must not change topology");

    let c_ab = link(&mut g, &c, &ab, "elaborates");
    assert_eq!(g.relation(&c_ab).target(), Some(&ab));
    assert_eq!(g.shortest_path_len(&a, &b), Some(1));
    assert_eq!(g.shortest_path_len(&c, &a), Some(2));
    g.check_invariants().unwrap();
}

trait Rel {
    fn relation(&self, id: &EntityId) -> &Relation;
}
impl Rel for Graph {
    fn relation(&self, id: &EntityId) -> &Relation {
        &self.relations[id]
    }
}

/// Acceptance test A4 / invariant I2.
#[test]
fn a4_cascade_delete_and_single_undo_restores_everything() {
    let mut g = seeded();
    let a = node(&mut g, "A");
    let b = node(&mut g, "B");
    let c = node(&mut g, "C");
    let ab = link(&mut g, &a, &b, PREREQUISITE_OF);
    run!(g, commands::annotate(&g, &ab, "gap", "", 3).unwrap());
    let c_ab = link(&mut g, &c, &ab, UNCLEAR_LINK);
    let d = node(&mut g, "D");
    let d_cab = link(&mut g, &d, &c_ab, "elaborates");
    let before = g.clone();

    assert_eq!(g.cascade(&a).len(), 4, "A, A—B, C—(A—B), D—(C—(A—B))");
    let del = run!(g, commands::delete_entity(&g, &a).unwrap());
    for gone in [&a, &ab, &c_ab, &d_cab] {
        assert!(!g.contains(gone));
    }
    assert!(g.contains(&b) && g.contains(&c) && g.contains(&d));
    g.check_invariants().unwrap();

    g.apply(&del.inverse()).unwrap();
    assert_eq!(g, before);
}

#[test]
fn i3_relation_cannot_be_its_own_endpoint() {
    let mut g = seeded();
    let a = node(&mut g, "A");
    let b = node(&mut g, "B");
    let ab = link(&mut g, &a, &b, PREREQUISITE_OF);
    // Forge a relation whose endpoint is itself.
    let mut forged = g.relations[&ab].clone();
    forged.id = EntityId::from("self");
    forged.endpoints[1].entity = forged.id.clone();
    let mut tx = Tx::new("forge");
    tx.push(Op::AddRelation(forged));
    assert!(matches!(g.apply(&tx), Err(CoreError::SelfEndpoint(_))));
}

#[test]
fn i4_depth_is_capped() {
    let mut g = seeded();
    let a = node(&mut g, "A");
    let b = node(&mut g, "B");
    let mut top = link(&mut g, &a, &b, UNCLEAR_LINK);
    for d in 2..=MAX_RELATION_DEPTH {
        let n = node(&mut g, &format!("n{d}"));
        top = link(&mut g, &n, &top, UNCLEAR_LINK);
        assert_eq!(g.depth(&top), d);
    }
    let n = node(&mut g, "one too many");
    let err =
        commands::link(&g, &n, &top, &KindId::from(UNCLEAR_LINK), 9).and_then(|(tx, _)| g.clone().apply(&tx));
    assert!(matches!(err, Err(CoreError::DepthExceeded { .. })));
}

#[test]
fn i1_update_may_not_rewire() {
    let mut g = seeded();
    let a = node(&mut g, "A");
    let b = node(&mut g, "B");
    let c = node(&mut g, "C");
    let ab = link(&mut g, &a, &b, PREREQUISITE_OF);
    let before = g.relations[&ab].clone();
    let mut after = before.clone();
    after.endpoints[1].entity = c;
    let mut tx = Tx::new("rewire");
    tx.push(Op::UpdateRelation { before, after });
    assert_eq!(g.apply(&tx), Err(CoreError::TopologyChange));
}

#[test]
fn reverse_swaps_roles_and_is_undoable() {
    let mut g = seeded();
    let a = node(&mut g, "A");
    let b = node(&mut g, "B");
    let ab = link(&mut g, &a, &b, PREREQUISITE_OF);
    let tx = run!(g, commands::reverse(&g, &ab, 5).unwrap());
    assert_eq!(g.relations[&ab].source(), Some(&b));
    g.apply(&tx.inverse()).unwrap();
    assert_eq!(g.relations[&ab].source(), Some(&a));
}

#[test]
fn unclear_link_joins_confusion_system() {
    let mut g = seeded();
    let a = node(&mut g, "A");
    let b = node(&mut g, "B");
    let r = link(&mut g, &a, &b, UNCLEAR_LINK);
    assert!(g.in_system(&r, &SystemId::from("lapse")));
    assert_eq!(g.relation_state(&r), Some(RelationState::Promoted));
}

#[test]
fn failed_tx_is_atomic() {
    let mut g = seeded();
    let a = node(&mut g, "A");
    let before = g.clone();
    let mut tx = Tx::new("half");
    tx.push(Op::AddNode(Node {
        id: EntityId::from("x"),
        title: "x".into(),
        body: String::new(),
        abstraction_level: None,
        color_override: None,
        created_at: 0,
        updated_at: 0,
    }));
    tx.push(Op::AddNode(g.nodes[&a].clone())); // duplicate → fails
    assert!(g.apply(&tx).is_err());
    assert_eq!(g, before);
}

#[test]
fn cycles_are_flagged_and_roots_found() {
    let mut g = seeded();
    let [a, b, c, d] = ["A", "B", "C", "D"].map(|t| node(&mut g, t));
    link(&mut g, &a, &b, PREREQUISITE_OF);
    link(&mut g, &b, &c, PREREQUISITE_OF);
    link(&mut g, &d, &c, PREREQUISITE_OF);
    let k = KindId::from(PREREQUISITE_OF);
    assert!(g.cycles(&k).is_empty());
    let mut roots = g.roots(&k);
    roots.sort();
    let mut expect = vec![a.clone(), d.clone()];
    expect.sort();
    assert_eq!(roots, expect);
    let ca = link(&mut g, &c, &a, PREREQUISITE_OF);
    assert_eq!(g.cycles(&k).len(), 1);
    assert!(g.relations_in_cycles().contains(&ca));
}

#[test]
fn overlay_modes_and_ghost_endpoints() {
    let mut g = seeded();
    let [a, b, c] = ["A", "B", "C"].map(|t| node(&mut g, t));
    let lapse = SystemId::from("lapse");
    let term = SystemId::from("terminology");
    for (e, s) in [(&a, &lapse), (&b, &lapse), (&b, &term), (&c, &term)] {
        run!(g, commands::set_membership(&g, e, s, true, 3).unwrap());
    }
    let ab = link(&mut g, &a, &b, PREREQUISITE_OF);
    let bc = link(&mut g, &b, &c, "elaborates");

    let mut ov =
        Overlay { active: vec![lapse.clone(), term.clone()], mode: Combine::Intersection, ghost: false };
    let v = ov.resolve(&g);
    assert_eq!(v[&b], Visibility::Full);
    assert_eq!(v[&a], Visibility::Hidden);

    ov.mode = Combine::Difference;
    let v = ov.resolve(&g);
    assert_eq!(v[&a], Visibility::Full);
    assert_eq!(v[&b], Visibility::Hidden);

    // A relation filed in a system whose endpoints are not: I5.
    let open = SystemId::from("open");
    run!(g, commands::set_membership(&g, &bc, &open, true, 4).unwrap());
    let v = Overlay { active: vec![open], mode: Combine::Union, ghost: true }.resolve(&g);
    assert_eq!(v[&bc], Visibility::Full);
    assert_eq!(v[&b], Visibility::GhostEndpoint);
    assert_eq!(v[&c], Visibility::GhostEndpoint);
    assert_eq!(v[&a], Visibility::Ghost);
    assert_eq!(v[&ab], Visibility::Ghost);

    // Union: relation between two visible nodes is visible although unfiled.
    let v = Overlay { active: vec![lapse], mode: Combine::Union, ghost: false }.resolve(&g);
    assert_eq!(v[&ab], Visibility::Full);
}

/// Acceptance test A8 at the core level.
#[test]
fn a8_json_round_trip_is_byte_identical() {
    let mut g = seeded();
    let [a, b, c] = ["A", "B", "C"].map(|t| node(&mut g, t));
    let ab = link(&mut g, &a, &b, PREREQUISITE_OF);
    run!(g, commands::annotate(&g, &ab, "t", "b", 3).unwrap());
    run!(g, commands::set_friction(&g, &ab, Some(0.3), 3).unwrap());
    link(&mut g, &c, &ab, UNCLEAR_LINK);
    run!(
        g,
        commands::add_document(
            &g,
            Document {
                id: DocumentId::from("abc"),
                path: "/x.pdf".into(),
                title: Some("X".into()),
                page_count: 3,
                copied_local: false,
                added_at: 1,
            },
        ),
    );
    let anchor = Anchor {
        id: AnchorId::from(""),
        entity: a.clone(),
        document: DocumentId::from("abc"),
        page_index: 1,
        kind: AnchorKind::Text,
        quads: vec![Quad::from_rect(1.5, 2.25, 30.0, 12.1)],
        exact: "quadratic variation".into(),
        prefix: "the ".into(),
        suffix: " of".into(),
        char_start: Some(4),
        char_end: Some(23),
        confidence: 1.0,
        color: None,
    };
    run!(g, commands::add_anchor(&g, &a, anchor).unwrap().0);

    let json = g.to_json();
    let g2 = Graph::from_json(&json).unwrap();
    assert_eq!(g2, g);
    assert_eq!(g2.to_json(), json);
}

mod props {
    use super::*;
    use proptest::prelude::*;

    #[derive(Debug, Clone)]
    enum Action {
        Node,
        Link(usize, usize, usize),
        Annotate(usize),
        File(usize, usize, bool),
        Delete(usize),
        Reverse(usize),
    }

    fn action() -> impl Strategy<Value = Action> {
        prop_oneof![
            3 => Just(Action::Node),
            4 => (any::<usize>(), any::<usize>(), any::<usize>()).prop_map(|(a, b, k)| Action::Link(a, b, k)),
            2 => any::<usize>().prop_map(Action::Annotate),
            2 => (any::<usize>(), any::<usize>(), any::<bool>()).prop_map(|(a, s, m)| Action::File(a, s, m)),
            1 => any::<usize>().prop_map(Action::Delete),
            1 => any::<usize>().prop_map(Action::Reverse),
        ]
    }

    fn pick<'a, T>(v: &'a [T], i: usize) -> Option<&'a T> {
        (!v.is_empty()).then(|| &v[i % v.len()])
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        /// For any action sequence: invariants hold after every step, every
        /// tx's inverse restores the prior graph exactly, and content/
        /// membership edits never change topology (I1).
        #[test]
        fn invariants_hold_and_undo_is_exact(actions in prop::collection::vec(action(), 1..60)) {
            let mut g = seeded();
            let kinds: Vec<KindId> = g.kinds.keys().cloned().collect();
            let systems: Vec<SystemId> = g.systems.keys().cloned().collect();
            for act in actions {
                let ids: Vec<EntityId> = g.entity_ids().cloned().collect();
                let rels: Vec<EntityId> = g.relations.keys().cloned().collect();
                let tx = match act {
                    Action::Node => Some(commands::create_node(&g, NewNode {
                        title: "n".into(), body: String::new(), systems: vec![], anchors: vec![] }, 1).0),
                    Action::Link(a, b, k) => match (pick(&ids, a), pick(&ids, b)) {
                        (Some(a), Some(b)) if a != b =>
                            commands::link(&g, a, b, pick(&kinds, k).unwrap(), 1).ok().map(|x| x.0),
                        _ => None,
                    },
                    Action::Annotate(r) => pick(&rels, r).map(|r| commands::annotate(&g, r, "t", "b", 2).unwrap()),
                    Action::File(e, s, m) => pick(&ids, e)
                        .map(|e| commands::set_membership(&g, e, pick(&systems, s).unwrap(), m, 3).unwrap()),
                    Action::Delete(e) => pick(&ids, e).map(|e| commands::delete_entity(&g, e).unwrap()),
                    Action::Reverse(r) => pick(&rels, r).map(|r| commands::reverse(&g, r, 4).unwrap()),
                };
                let Some(tx) = tx else { continue };
                let before = g.clone();
                if g.apply(&tx).is_err() {
                    // Rejected (I3/I4) — must be a no-op.
                    prop_assert_eq!(&g, &before);
                    continue;
                }
                prop_assert!(g.check_invariants().is_ok(), "{:?}", g.check_invariants());
                if matches!(act, Action::Annotate(_) | Action::File(..) | Action::Reverse(_)) {
                    prop_assert_eq!(g.topology(), before.topology());
                }
                let mut undone = g.clone();
                undone.apply(&tx.inverse()).unwrap();
                prop_assert_eq!(&undone, &before);
            }
        }
    }
}
