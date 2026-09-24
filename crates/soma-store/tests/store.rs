use soma_core::commands::{self, NewNode};
use soma_core::defaults::{PREREQUISITE_OF, UNCLEAR_LINK};
use soma_core::*;
use soma_store::Store;

fn create(g: &mut Graph, s: &mut Store, title: &str) -> EntityId {
    let (tx, id) = commands::create_node(
        g,
        NewNode { title: title.into(), body: format!("body of {title}"), systems: vec![], anchors: vec![] },
        now_ms(),
    );
    g.apply(&tx).unwrap();
    s.commit(&tx).unwrap();
    id
}

macro_rules! commit {
    ($g:ident, $s:ident, $e:expr) => {{
        let tx: Tx = $e;
        $g.apply(&tx).unwrap();
        $s.commit(&tx).unwrap();
    }};
}

fn doc() -> Document {
    Document {
        id: DocumentId::from("d1"),
        path: "/tmp/paper.pdf".into(),
        title: Some("Paper".into()),
        page_count: 10,
        copied_local: false,
        added_at: 5,
    }
}

fn anchor() -> Anchor {
    Anchor {
        id: AnchorId::from(""),
        entity: EntityId::from(""),
        document: DocumentId::from("d1"),
        page_index: 3,
        kind: AnchorKind::Text,
        quads: vec![Quad::from_rect(10.0, 20.0, 110.5, 32.25)],
        exact: "quadratic variation".into(),
        prefix: "the ".into(),
        suffix: " of the process".into(),
        char_start: Some(100),
        char_end: Some(119),
        confidence: 1.0,
        color: Some(Color::rgb(1, 2, 3)),
    }
}

#[test]
fn persisted_graph_matches_memory_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("w.soma");
    let mut s = Store::open(&path).unwrap();
    let mut g = s.load().unwrap();
    assert!(g.inbox().is_some());

    commit!(g, s, commands::add_document(&g, doc()));
    let a = create(&mut g, &mut s, "quadratic variation");
    let b = create(&mut g, &mut s, "Itô isometry");
    commit!(g, s, commands::add_anchor(&g, &a, anchor()).unwrap().0);
    let (tx, ab) = commands::link(&g, &a, &b, &KindId::from(PREREQUISITE_OF), 7).unwrap();
    commit!(g, s, tx);
    commit!(g, s, commands::annotate(&g, &ab, "why the limit?", "algebra ok", 8).unwrap());
    commit!(g, s, commands::set_friction(&g, &ab, Some(0.7), 9).unwrap());
    commit!(g, s, commands::shift_level(&g, &a, 1, 9).unwrap());
    let (tx, _) = commands::link(&g, &a, &ab, &KindId::from(UNCLEAR_LINK), 10).unwrap();
    commit!(g, s, tx);
    commit!(g, s, commands::reverse(&g, &ab, 11).unwrap());
    drop(s);

    let s = Store::open(&path).unwrap();
    let loaded = s.load().unwrap();
    assert_eq!(loaded, g);
    assert_eq!(loaded.to_json(), g.to_json());
    assert!(s.integrity_check().unwrap().is_empty());
}

#[test]
fn undo_redo_survive_restart_and_cascade_is_one_step() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("w.soma");
    let mut s = Store::open(&path).unwrap();
    let mut g = s.load().unwrap();
    let a = create(&mut g, &mut s, "A");
    let b = create(&mut g, &mut s, "B");
    let (tx, ab) = commands::link(&g, &a, &b, &KindId::from(PREREQUISITE_OF), 1).unwrap();
    commit!(g, s, tx);
    commit!(g, s, commands::set_membership(&g, &ab, &SystemId::from("lapse"), true, 2).unwrap());
    let before_delete = g.clone();
    commit!(g, s, commands::delete_entity(&g, &a).unwrap());
    assert!(!g.contains(&ab));
    drop(s);

    // Restart, then one undo restores the whole cascade (A4 through the store).
    let mut s = Store::open(&path).unwrap();
    let mut g = s.load().unwrap();
    let tx = s.undo().unwrap().unwrap();
    g.apply(&tx).unwrap();
    assert_eq!(g, before_delete);
    assert_eq!(s.load().unwrap(), before_delete);

    let tx = s.redo().unwrap().unwrap();
    g.apply(&tx).unwrap();
    assert!(!g.contains(&a));
    assert_eq!(s.load().unwrap(), g);

    // A new action truncates the redo tail.
    let tx = s.undo().unwrap().unwrap();
    g.apply(&tx).unwrap();
    create(&mut g, &mut s, "C");
    assert!(s.redo().unwrap().is_none());
}

#[test]
fn undo_depth_is_at_least_200() {
    let mut s = Store::open_in_memory().unwrap();
    let mut g = s.load().unwrap();
    let seed = g.clone();
    for i in 0..250 {
        create(&mut g, &mut s, &format!("n{i}"));
    }
    for _ in 0..250 {
        let tx = s.undo().unwrap().expect("history retained");
        g.apply(&tx).unwrap();
    }
    assert_eq!(g, seed);
}

#[test]
fn full_text_search() {
    let mut s = Store::open_in_memory().unwrap();
    let mut g = s.load().unwrap();
    let a = create(&mut g, &mut s, "quadratic variation");
    let b = create(&mut g, &mut s, "Itô isometry");
    let (tx, ab) = commands::link(&g, &a, &b, &KindId::from(PREREQUISITE_OF), 1).unwrap();
    commit!(g, s, tx);
    commit!(g, s, commands::annotate(&g, &ab, "martingale gap", "", 2).unwrap());
    assert_eq!(s.search("quadr", 10).unwrap(), vec![a.clone()]);
    assert_eq!(s.search("martingale", 10).unwrap(), vec![ab]);
    commit!(g, s, commands::delete_entity(&g, &a).unwrap());
    assert!(s.search("quadratic", 10).unwrap().is_empty());
}

#[test]
fn notes_persist_are_searchable_and_undo() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("w.soma");
    let mut s = Store::open(&path).unwrap();
    let mut g = s.load().unwrap();
    assert_eq!(g.systems[&SystemId::from("lapse")].note_prompt, "What exactly don't you follow?");
    let a = create(&mut g, &mut s, "quadratic variation");
    commit!(g, s, commands::set_note(&g, &a, &SystemId::from("lapse"), "why the limit exists", 1).unwrap());
    commit!(g, s, commands::set_note(&g, &a, &SystemId::from("terminology"), "defined in eq 3", 2).unwrap());
    assert_eq!(s.search("limit", 10).unwrap(), vec![a.clone()]);
    drop(s);
    let mut s = Store::open(&path).unwrap();
    let mut loaded = s.load().unwrap();
    assert_eq!(loaded, g);
    let tx = s.undo().unwrap().unwrap();
    loaded.apply(&tx).unwrap();
    assert!(!loaded.in_system(&a, &SystemId::from("terminology")));
    assert_eq!(s.load().unwrap(), loaded);
    assert!(s.search("eq", 10).unwrap().is_empty());
}

/// Child half of the SIGKILL test: commits forever until killed.
#[test]
#[ignore]
fn kill_child_writer() {
    let Ok(path) = std::env::var("SOMA_KILL_DB") else { return };
    let mut s = Store::open(&path).unwrap();
    let mut g = s.load().unwrap();
    let mut prev: Option<EntityId> = None;
    for i in 0.. {
        let (mut tx, id) = commands::create_node(
            &g,
            NewNode { title: format!("n{i}"), body: "x".repeat(200), systems: vec![], anchors: vec![] },
            i,
        );
        if let Some(p) = &prev {
            let mut g2 = g.clone();
            g2.apply(&tx).unwrap();
            tx.extend(commands::link(&g2, p, &id, &KindId::from(PREREQUISITE_OF), i).unwrap().0);
        }
        g.apply(&tx).unwrap();
        s.commit(&tx).unwrap();
        prev = Some(id);
    }
}

/// Acceptance test A7: SIGKILL during heavy annotation loses at most the
/// in-flight edit; the workspace opens clean.
#[test]
fn a7_sigkill_loses_at_most_in_flight_edit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kill.soma");
    Store::open(&path).unwrap();
    for round in 0..3 {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "kill_child_writer", "--ignored", "--nocapture"])
            .env("SOMA_KILL_DB", &path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400 + round * 150));
        child.kill().unwrap(); // SIGKILL on unix
        child.wait().unwrap();

        let s = Store::open(&path).unwrap();
        assert!(s.integrity_check().unwrap().is_empty());
        let g = s.load().expect("workspace loads after SIGKILL");
        g.check_invariants().unwrap();
        // Every committed transaction is fully present: each logged tx made
        // one node (and one link after the first), never half of one.
        assert!(!g.nodes.is_empty(), "child made progress");
        assert_eq!(g.nodes.len(), g.relations.len() + round as usize + 1, "no half-applied transaction");
    }
}
