//! Headless workspace tool: export / import / verify / bench (PRD §7.4).

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use soma_core::commands::{self, NewNode};
use soma_core::*;
use soma_store::Store;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Parser)]
#[command(name = "soma", about = "Soma workspace tool")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create an empty, seeded workspace.
    Init { workspace: PathBuf },
    /// Register a PDF (content-hashed) with a workspace.
    AddDoc { workspace: PathBuf, pdf: PathBuf },
    /// Entity, relation, system and anchor counts.
    Stats { workspace: PathBuf },
    /// Lossless JSON export (F-DATA-5).
    Export {
        workspace: PathBuf,
        /// Output file; stdout if omitted.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Import a JSON export into a new workspace.
    Import { json: PathBuf, workspace: PathBuf },
    /// Check SQLite integrity and every domain invariant.
    Verify { workspace: PathBuf },
    /// Full-text search over titles and bodies.
    Search { workspace: PathBuf, query: String },
    /// Roots of the prerequisite DAG: "start here".
    Roots {
        workspace: PathBuf,
        #[arg(long, default_value = defaults::PREREQUISITE_OF)]
        kind: String,
    },
    /// Write a sample multi-page technical PDF (for trying the reader).
    DemoPdf { out: PathBuf },
    /// Synthetic scale benchmark (acceptance test A6, data side).
    Bench {
        #[arg(long, default_value_t = 10_000)]
        nodes: usize,
        #[arg(long, default_value_t = 20_000)]
        relations: usize,
        /// Also measure persisting and reloading via a temp workspace.
        #[arg(long)]
        store: bool,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Init { workspace } => {
            if workspace.exists() {
                bail!("{} already exists", workspace.display());
            }
            Store::open(&workspace)?;
            println!("created {}", workspace.display());
        }
        Cmd::AddDoc { workspace, pdf } => {
            let mut store = Store::open(&workspace)?;
            let g = store.load()?;
            let info = soma_pdf::identify(&pdf)?;
            let tx = commands::add_document(&g, info.clone());
            store.commit(&tx)?;
            println!("{}  {} pages  {}", info.id, info.page_count, info.path);
        }
        Cmd::Stats { workspace } => {
            let g = Store::open(&workspace)?.load()?;
            let annotated = g.relations.values().filter(|r| r.has_content()).count();
            let promoted =
                g.relations.keys().filter(|r| g.relation_state(r) == Some(RelationState::Promoted)).count();
            println!("documents  {}", g.documents.len());
            println!("nodes      {}", g.nodes.len());
            println!("relations  {} ({annotated} annotated, {promoted} promoted)", g.relations.len());
            println!("anchors    {}", g.anchors.len());
            for s in g.systems.values() {
                println!("  {:<24} {}", s.name, g.members_of(&s.id).count());
            }
        }
        Cmd::Export { workspace, out } => {
            let json = Store::open(&workspace)?.load()?.to_json();
            match out {
                Some(p) => std::fs::write(&p, json).with_context(|| format!("writing {}", p.display()))?,
                None => println!("{json}"),
            }
        }
        Cmd::Import { json, workspace } => {
            if workspace.exists() {
                bail!("{} already exists", workspace.display());
            }
            let text = std::fs::read_to_string(&json)?;
            let snapshot: Snapshot = serde_json::from_str(&text)?;
            Graph::from_snapshot(&snapshot)?; // validate before touching disk
            let mut store = Store::open(&workspace)?;
            let seeded = store.load()?;
            // Replace the seed with the snapshot's own kinds/systems.
            let mut tx = Tx::new("clear seed");
            tx.ops.extend(seeded.systems.values().cloned().map(Op::RemoveSystem));
            tx.ops.extend(seeded.kinds.values().cloned().map(Op::RemoveKind));
            store.apply_unlogged(&tx)?;
            store.apply_unlogged(&Graph::snapshot_tx(&snapshot))?;
            println!("imported {} entities into {}", store.load()?.entity_count(), workspace.display());
        }
        Cmd::Verify { workspace } => {
            let store = Store::open(&workspace)?;
            let mut problems = store.integrity_check()?;
            match store.load() {
                Ok(g) => {
                    if let Err(errs) = g.check_invariants() {
                        problems.extend(errs);
                    }
                    let cycles: usize =
                        g.kinds.values().filter(|k| k.acyclic).map(|k| g.cycles(&k.id).len()).sum();
                    if cycles > 0 {
                        println!("note: {cycles} cycle(s) in acyclic kinds (flagged, not errors)");
                    }
                }
                Err(e) => problems.push(e.to_string()),
            }
            if problems.is_empty() {
                println!("ok");
            } else {
                for p in &problems {
                    println!("{p}");
                }
                bail!("{} problem(s)", problems.len());
            }
        }
        Cmd::Search { workspace, query } => {
            let store = Store::open(&workspace)?;
            let g = store.load()?;
            for id in store.search(&query, 50)? {
                println!("{id}  {}", g.title(&id));
            }
        }
        Cmd::Roots { workspace, kind } => {
            let g = Store::open(&workspace)?.load()?;
            for id in g.roots(&KindId::from(kind)) {
                println!("{id}  {}", g.title(&id));
            }
        }
        Cmd::DemoPdf { out } => {
            let text = include_str!("demo.txt");
            let words: Vec<&str> = text.split_whitespace().collect();
            std::fs::write(&out, soma_pdf::testpdf::make_pdf(&soma_pdf::testpdf::flow(&words, 44, 2))?)?;
            println!("wrote {}", out.display());
        }
        Cmd::Bench { nodes, relations, store } => bench(nodes, relations, store)?,
    }
    Ok(())
}

fn time<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let out = f();
    println!("{label:<40} {:>9.2} ms", t.elapsed().as_secs_f64() * 1e3);
    out
}

/// Deterministic pseudo-random graph: `nodes` nodes spread over the seeded
/// systems, `relations` relations of mixed kinds, ~5% relation-on-relation.
pub fn synthetic(nodes: usize, relations: usize) -> Graph {
    let mut g = Graph::new();
    g.apply(&defaults::seed()).unwrap();
    let systems: Vec<SystemId> = g.systems.keys().cloned().collect();
    let kinds: Vec<KindId> =
        g.kinds.keys().filter(|k| k.as_str() != defaults::UNCLEAR_LINK).cloned().collect();
    let mut rng = 0x9e3779b97f4a7c15u64;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let mut tx = Tx::new("synthetic");
    let mut ids = Vec::with_capacity(nodes);
    for i in 0..nodes {
        let (t, id) = commands::create_node(
            &g,
            NewNode {
                title: format!("node {i}"),
                body: String::new(),
                systems: vec![systems[(next() as usize) % systems.len()].clone()],
                anchors: vec![],
            },
            i as i64,
        );
        tx.extend(t);
        ids.push(id);
    }
    g.apply(&tx).unwrap();
    let mut rel_ids: Vec<EntityId> = Vec::new();
    let mut tx = Tx::new("synthetic relations");
    for i in 0..relations {
        let a = ids[(next() as usize) % ids.len()].clone();
        let b = if i % 20 == 0 && !rel_ids.is_empty() {
            rel_ids[(next() as usize) % rel_ids.len()].clone()
        } else {
            ids[(next() as usize) % ids.len()].clone()
        };
        if a == b {
            continue;
        }
        let kind = &kinds[(next() as usize) % kinds.len()];
        let id = EntityId(format!("r{i:06}"));
        tx.push(Op::AddRelation(Relation {
            id: id.clone(),
            kind: kind.clone(),
            endpoints: vec![
                Endpoint { entity: a, role: Role::Source, ordinal: 0 },
                Endpoint { entity: b, role: Role::Target, ordinal: 1 },
            ],
            friction: None,
            title: None,
            body: None,
            created_at: i as i64,
            updated_at: i as i64,
        }));
        if i % 20 != 0 {
            rel_ids.push(id);
        }
    }
    g.apply(&tx).unwrap();
    g
}

fn bench(nodes: usize, relations: usize, with_store: bool) -> Result<()> {
    let g = time(&format!("build {nodes} nodes / {relations} relations"), || synthetic(nodes, relations));
    let systems: Vec<SystemId> = g.systems.keys().cloned().collect();
    let mut overlay = Overlay::default();
    time("overlay resolve (no systems)", || overlay.resolve(&g));
    overlay.toggle(&systems[0]);
    let v = time("overlay toggle → resolve (1 system)", || overlay.resolve(&g));
    println!("  visible: {}", v.values().filter(|v| **v == Visibility::Full).count());
    overlay.toggle(&systems[1]);
    time("overlay toggle → resolve (2, union)", || overlay.resolve(&g));
    overlay.mode = Combine::Intersection;
    time("overlay resolve (intersection)", || overlay.resolve(&g));
    let first = g.nodes.keys().next().unwrap().clone();
    time("k-hop(3) focus", || g.k_hop(&first, 3));
    time("cycle detection (all acyclic kinds)", || g.relations_in_cycles());
    time("check_invariants", || g.check_invariants().is_ok());
    let json = time("export json", || g.to_json());
    let g2 = time("import json", || Graph::from_json(&json).unwrap());
    assert_eq!(g2.to_json(), json, "round trip");

    if with_store {
        let dir = std::env::temp_dir().join(format!("soma-bench-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("bench.soma");
        let mut store = Store::open(&path)?;
        let mut tx = Graph::snapshot_tx(&g.to_snapshot());
        tx.ops.retain(|op| !matches!(op, Op::AddKind(_) | Op::AddSystem(_)));
        time("persist workspace", || store.apply_unlogged(&tx))?;
        drop(store);
        let loaded = time("cold workspace load", || Store::open(&path).and_then(|s| s.load()))?;
        assert_eq!(loaded.entity_count(), g.entity_count());
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A8: export → import → export is byte-identical, through the store.
    #[test]
    fn a8_round_trip_through_store() {
        let g = synthetic(300, 600);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a8.soma");
        let mut store = Store::open(&path).unwrap();
        let mut tx = Graph::snapshot_tx(&g.to_snapshot());
        tx.ops.retain(|op| !matches!(op, Op::AddKind(_) | Op::AddSystem(_)));
        store.apply_unlogged(&tx).unwrap();
        let loaded = Store::open(&path).unwrap().load().unwrap();
        assert_eq!(loaded.to_json(), g.to_json());
    }

    /// A6 (data side): overlay toggle over 10k/20k within the 33 ms budget.
    /// Only meaningful in release builds.
    #[test]
    #[cfg_attr(debug_assertions, ignore)]
    fn a6_overlay_toggle_budget() {
        let g = synthetic(10_000, 20_000);
        let mut ov = Overlay::default();
        ov.toggle(g.systems.keys().next().unwrap());
        let t = Instant::now();
        ov.resolve(&g);
        assert!(t.elapsed().as_millis() < 33, "{:?}", t.elapsed());
    }
}
