//! The UI's view of the workspace: the in-memory graph (mutated
//! optimistically), the store worker that persists it, and the "last-target
//! memory" that turns 8 keystrokes into 3 (PRD §6.1 principle 3).

use soma_core::*;
use soma_store::Store;
use soma_store::worker::{Event, Request, StoreWorker};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub struct Toast {
    pub text: String,
    pub until: Instant,
    pub undo_hint: bool,
    pub error: bool,
}

pub struct Workspace {
    pub graph: Graph,
    pub path: PathBuf,
    store: StoreWorker,
    /// Most recent first. `created[0]` is "the just-created node".
    pub created: VecDeque<EntityId>,
    /// Recently touched entities, most recent first (hint targets, strip).
    pub recent: VecDeque<EntityId>,
    pub focused: Option<EntityId>,
    /// Ctrl-click multi-selection in the canvas.
    pub selected: Vec<EntityId>,
    pub last_system: Option<SystemId>,
    pub last_kind: Option<KindId>,
    pub last_color: Color,
    pub toasts: Vec<Toast>,
    /// Bumped on every graph change so views can cache derived data.
    pub revision: u64,
}

impl Workspace {
    pub fn open(path: PathBuf, ctx: egui::Context) -> anyhow::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let store = Store::open(&path)?;
        let graph = store.load()?;
        let store = StoreWorker::spawn(store, move || ctx.request_repaint());
        let last_system = graph.system_by_hotkey(1).map(|s| s.id.clone());
        let last_color = graph.system_by_hotkey(1).map(|s| s.color).unwrap_or(Color::rgb(0xf5, 0xd4, 0x4c));
        let mut ws = Self {
            graph,
            path,
            store,
            created: VecDeque::new(),
            recent: VecDeque::new(),
            focused: None,
            selected: Vec::new(),
            last_system,
            last_kind: None,
            last_color,
            toasts: Vec::new(),
            revision: 0,
        };
        // Seed recency with the newest entities so hints work after restart.
        let mut newest: Vec<(&Timestamp, &EntityId)> = ws
            .graph
            .nodes
            .values()
            .filter(|n| !commands::is_highlight_carrier(&n.id))
            .map(|n| (&n.updated_at, &n.id))
            .collect();
        newest.sort_by(|a, b| b.0.cmp(a.0));
        ws.recent = newest.into_iter().take(40).map(|(_, id)| id.clone()).collect();
        ws.created = ws.recent.iter().take(2).cloned().collect();
        Ok(ws)
    }

    /// Apply optimistically and persist asynchronously.
    pub fn commit(&mut self, tx: Tx) -> Result<(), CoreError> {
        if tx.is_empty() {
            return Ok(());
        }
        if let Err(e) = self.graph.apply(&tx) {
            self.toast_error(e.to_string());
            return Err(e);
        }
        self.revision += 1;
        self.store.send(Request::Commit(tx));
        Ok(())
    }

    pub fn undo(&mut self) {
        self.store.send(Request::Undo);
    }

    pub fn redo(&mut self) {
        self.store.send(Request::Redo);
    }

    /// Drain store events. Returns true if the graph changed.
    pub fn poll(&mut self) -> bool {
        let events: Vec<Event> = self.store.poll().collect();
        let mut changed = false;
        for ev in events {
            match ev {
                Event::Committed { .. } => {}
                Event::Failed { tx, error } => {
                    let _ = self.graph.apply(&tx.inverse());
                    self.toast_error(format!("not saved: {error}"));
                    changed = true;
                }
                Event::Undone(tx) | Event::Redone(tx) => {
                    let label = tx.label.clone();
                    if let Err(e) = self.graph.apply(&tx) {
                        self.toast_error(format!("undo out of sync: {e}; reloading"));
                        if let Ok(s) = Store::open(&self.path).and_then(|s| s.load()) {
                            self.graph = s;
                        }
                    } else {
                        self.toast(label, false);
                    }
                    self.forget_missing();
                    changed = true;
                }
                Event::NothingToUndo => self.toast("nothing to undo".into(), false),
                Event::NothingToRedo => self.toast("nothing to redo".into(), false),
                Event::Error(e) => self.toast_error(e),
            }
        }
        if changed {
            self.revision += 1;
        }
        let now = Instant::now();
        self.toasts.retain(|t| t.until > now);
        changed
    }

    /// Drop references to entities that no longer exist.
    pub fn forget_missing(&mut self) {
        let g = &self.graph;
        self.created.retain(|e| g.contains(e));
        self.recent.retain(|e| g.contains(e));
        self.selected.retain(|e| g.contains(e));
        if self.focused.as_ref().is_some_and(|f| !g.contains(f)) {
            self.focused = None;
        }
    }

    pub fn touch(&mut self, id: &EntityId) {
        if commands::is_highlight_carrier(id) {
            return;
        }
        self.recent.retain(|e| e != id);
        self.recent.push_front(id.clone());
        self.recent.truncate(60);
    }

    pub fn focus(&mut self, id: Option<EntityId>) {
        if let Some(id) = &id {
            self.touch(id);
        }
        self.focused = id;
    }

    pub fn note_created(&mut self, id: &EntityId) {
        self.created.retain(|e| e != id);
        self.created.push_front(id.clone());
        self.created.truncate(8);
        self.focus(Some(id.clone()));
    }

    pub fn toast(&mut self, text: String, undo_hint: bool) {
        self.toasts.push(Toast {
            text,
            until: Instant::now() + Duration::from_millis(if undo_hint { 1500 } else { 2200 }),
            undo_hint,
            error: false,
        });
    }

    pub fn toast_error(&mut self, text: String) {
        self.toasts.push(Toast {
            text,
            until: Instant::now() + Duration::from_secs(5),
            undo_hint: false,
            error: true,
        });
    }

    /// The system a fresh link should draw its default kind from.
    pub fn active_system(&self) -> Option<SystemId> {
        self.focused
            .as_ref()
            .and_then(|f| self.graph.primary_system(f))
            .filter(|s| !s.inbox)
            .map(|s| s.id.clone())
            .or_else(|| self.last_system.clone())
    }

    pub fn default_kind(&self) -> Option<KindId> {
        commands::default_kind(&self.graph, self.active_system().as_ref())
    }

    /// Link `source → target` with the default kind. Returns the relation.
    pub fn link(&mut self, source: &EntityId, target: &EntityId, kind: Option<KindId>) -> Option<EntityId> {
        if source == target {
            self.toast_error("cannot link an entity to itself".into());
            return None;
        }
        let kind = kind.or_else(|| self.default_kind())?;
        match commands::link(&self.graph, source, target, &kind, now_ms()) {
            Ok((tx, id)) => {
                let label =
                    format!("{} -[{}]-> {}", self.graph.title(source), kind, self.graph.title(target));
                if self.commit(tx).is_ok() {
                    self.last_kind = Some(kind);
                    self.touch(target);
                    self.touch(source);
                    self.toast(label, true);
                    return Some(id);
                }
                None
            }
            Err(e) => {
                self.toast_error(e.to_string());
                None
            }
        }
    }

    pub fn documents_sorted(&self) -> Vec<Document> {
        let mut d: Vec<Document> = self.graph.documents.values().cloned().collect();
        d.sort_by(|a, b| b.added_at.cmp(&a.added_at));
        d
    }
}

pub fn to_color32(c: Color) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), c.a())
}

pub fn default_workspace_path() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join("Documents").join("Soma").join("workspace.soma")
}
