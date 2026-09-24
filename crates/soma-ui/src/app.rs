//! Application shell: modes, panels, the shared command vocabulary (PRD §6),
//! the zero-dialog capture flow (§5.2), palettes, composer and toasts.

use crate::canvas::{Canvas, CanvasAction, LayoutMode};
use crate::reader::{DocTab, Fit, Selection};
use crate::render::{PaperTheme, Renderer};
use crate::workspace::{PALETTE, Workspace, to_color32};
use egui::{Align2, Color32, Key, Modifiers, RichText, vec2};
use soma_core::commands::{self, NewNode};
use soma_core::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[path = "autotest.rs"]
mod autotest;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Reader,
    Canvas,
}

enum ComposerTarget {
    /// New node from the reader selection (`N`).
    FromSelection { anchor: Option<Anchor> },
    /// Edit an existing node's or relation's title/body.
    Edit(EntityId),
    /// Free-standing node (`Ctrl-N`), optionally linked from an entity.
    Free { link_from: Option<EntityId>, at: Option<soma_layout::Vec2> },
}

struct Composer {
    target: ComposerTarget,
    title: String,
    body: String,
    /// One editable note per system the entity is filed in:
    /// (system, "name", prompt, note).
    notes: Vec<(SystemId, String, String, String)>,
    at: egui::Pos2,
    focus_requested: bool,
}

/// The note field that opens after a capture: what, specifically, about
/// this highlight earns its place in the system.
struct NoteEdit {
    entity: EntityId,
    system: SystemId,
    text: String,
    at: egui::Pos2,
    focus_requested: bool,
}

struct Hints {
    source: EntityId,
    labels: Vec<(String, EntityId)>,
    typed: String,
    search: Option<String>,
}

enum Popup {
    None,
    Composer(Composer),
    Note(NoteEdit),
    Systems {
        target: EntityId,
    },
    Kinds {
        target: EntityId,
    },
    Hints(Hints),
    ConfirmDelete {
        target: EntityId,
        count: usize,
    },
    Help,
    /// Right-click menu on a highlight or an entity.
    Menu {
        target: MenuTarget,
        at: egui::Pos2,
    },
}

#[derive(Clone)]
enum MenuTarget {
    Highlight(AnchorId),
    Entity(EntityId),
}

/// What the graph strip shows.
#[derive(Clone, PartialEq)]
enum StripMode {
    /// Every node in the system you last captured into (follows you).
    CurrentSystem,
    /// Every node in one chosen system.
    System(SystemId),
    /// The 2-hop neighbourhood of the focused node.
    Neighbourhood,
}

pub struct SomaApp {
    ws: Workspace,
    renderer: Renderer,
    tabs: Vec<DocTab>,
    active: usize,
    mode: Mode,
    canvas: Canvas,
    strip: Canvas,
    strip_open: bool,
    overlay: Overlay,
    overlay_rev: u64,
    isolate_saved: Option<Overlay>,
    theme: PaperTheme,
    popup: Popup,
    /// After a link: a 4-item kind palette, one keypress overrides (F-CAP-6).
    kind_offer: Option<(EntityId, Vec<KindId>, Instant)>,
    edge_cycle: Option<(EntityId, usize)>,
    left_tab: LeftTab,
    new_system: String,
    autotest: Option<autotest::AutoTest>,
    undo_rect: egui::Rect,
    /// Key presses taken from egui before it could act on them (Tab).
    intercepted: Vec<(Key, Modifiers)>,
    /// Unsaved edits to systems' note prompts (right-click a system).
    prompt_drafts: std::collections::HashMap<SystemId, String>,
    /// After `h`: one keypress (1–8) recolours that highlight.
    colour_offer: Option<(AnchorId, Instant)>,
    /// "Link from here": the next entity clicked becomes the target.
    link_from: Option<EntityId>,
    strip_mode: StripMode,
    /// The overlay the strip shows, and a revision that changes with it.
    strip_overlay: (Overlay, u64),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LeftTab {
    Outline,
    Systems,
    Documents,
}

const HINT_CHARS: &[u8] = b"asdfghjklqweruiopzxcvnm";
const KIND_OFFER_SECS: f32 = 3.0;

impl SomaApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        workspace: PathBuf,
        pdfs: Vec<PathBuf>,
    ) -> anyhow::Result<Self> {
        let ctx = cc.egui_ctx.clone();
        let ws = Workspace::open(workspace, ctx.clone())?;
        let mut app = Self {
            ws,
            renderer: Renderer::new(ctx),
            tabs: Vec::new(),
            active: 0,
            mode: Mode::Reader,
            canvas: Canvas::new(),
            strip: Canvas::new(),
            strip_open: true,
            overlay: Overlay::default(),
            overlay_rev: 0,
            isolate_saved: None,
            theme: PaperTheme::Light,
            popup: Popup::None,
            kind_offer: None,
            edge_cycle: None,
            left_tab: LeftTab::Systems,
            new_system: String::new(),
            autotest: autotest::AutoTest::from_env(),
            undo_rect: egui::Rect::NOTHING,
            intercepted: Vec::new(),
            prompt_drafts: Default::default(),
            colour_offer: None,
            link_from: None,
            strip_mode: StripMode::CurrentSystem,
            strip_overlay: (Overlay::default(), 0),
        };
        for p in pdfs {
            app.open_pdf(&p);
        }
        if app.tabs.is_empty()
            && let Some(d) = app.ws.documents_sorted().into_iter().find(|d| Path::new(&d.path).exists())
        {
            app.open_document(d);
        }
        Ok(app)
    }

    // ------------------------------------------------------------ documents

    fn open_pdf(&mut self, path: &Path) {
        match soma_pdf::identify(path) {
            Ok(doc) => {
                let tx = commands::add_document(&self.ws.graph, doc.clone());
                let _ = self.ws.commit(tx);
                let doc = self.ws.graph.documents.get(&doc.id).cloned().unwrap_or(doc);
                self.open_document(doc);
            }
            Err(e) => self.ws.toast_error(format!("cannot open {}: {e}", path.display())),
        }
    }

    fn open_document(&mut self, doc: Document) {
        if let Some(i) = self.tabs.iter().position(|t| t.doc.id == doc.id) {
            self.active = i;
            self.mode = Mode::Reader;
            return;
        }
        match DocTab::open(doc.clone(), &self.renderer) {
            Ok(tab) => {
                self.tabs.push(tab);
                self.active = self.tabs.len() - 1;
                self.mode = Mode::Reader;
            }
            Err(e) => self.ws.toast_error(format!("cannot open {}: {e}", doc.path)),
        }
    }

    /// F-DATA-2: point a document at a new file. Same bytes → path update;
    /// different bytes → register the new version and re-anchor (PRD §4.2).
    fn relink(&mut self, old: &Document, path: &Path) {
        let new = match soma_pdf::identify(path) {
            Ok(d) => d,
            Err(e) => return self.ws.toast_error(e.to_string()),
        };
        let g = &self.ws.graph;
        let mut tx = Tx::new(format!("relink {}", old.title.as_deref().unwrap_or(&old.path)));
        if new.id == old.id {
            let mut after = old.clone();
            after.path = new.path.clone();
            tx.push(Op::UpdateDocument { before: old.clone(), after });
        } else {
            if !g.documents.contains_key(&new.id) {
                tx.push(Op::AddDocument(new.clone()));
            }
            let pdf = match soma_pdf::PdfiumDoc::open(path) {
                Ok(p) => p,
                Err(e) => return self.ws.toast_error(e.to_string()),
            };
            use soma_pdf::PdfBackend;
            let pages = pdf.page_count();
            let mut cache: std::collections::HashMap<u32, Option<soma_pdf::text::PageText>> =
                Default::default();
            let (mut ok, mut total) = (0, 0);
            for a in g.anchors_in_doc(&old.id) {
                total += 1;
                let res = soma_pdf::anchor::resolve(a, pages, &mut |p| {
                    cache.entry(p).or_insert_with(|| pdf.page_text(p).ok()).clone()
                });
                if res.confidence() >= 0.9 {
                    ok += 1;
                }
                tx.push(Op::UpdateAnchor { before: a.clone(), after: res.apply_to(a, &new.id) });
            }
            self.ws.toast(format!("re-anchored {ok}/{total} at high confidence"), false);
        }
        if self.ws.commit(tx).is_ok() {
            self.tabs.retain(|t| t.doc.id != old.id);
            self.active = self.active.min(self.tabs.len().saturating_sub(1));
            if let Some(d) = self.ws.graph.documents.get(&new.id).cloned() {
                self.open_document(d);
            }
        }
    }

    fn tab(&mut self) -> Option<&mut DocTab> {
        self.tabs.get_mut(self.active)
    }

    // -------------------------------------------------------------- capture

    fn selection_title(tab: &mut DocTab) -> String {
        let t = tab.selection_text().unwrap_or_default();
        let t = if t.is_empty() {
            match &tab.selection {
                Some(Selection::Region { page, .. }) => format!("Region p.{}", page + 1),
                Some(Selection::Object { page, .. }) => format!("Figure p.{}", page + 1),
                _ => String::new(),
            }
        } else {
            t
        };
        if t.chars().count() > 90 { format!("{}…", t.chars().take(89).collect::<String>()) } else { t }
    }

    /// F-CAP-1 `h`.
    fn highlight(&mut self) {
        let color = self.ws.last_color;
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let Some(anchor) = tab.selection_anchor(Some(color)) else { return };
        let doc = tab.doc.id.clone();
        tab.selection = None;
        let (tx, aid) = commands::highlight(&self.ws.graph, &doc, anchor, now_ms());
        if self.ws.commit(tx).is_ok() {
            self.colour_offer = Some((aid, Instant::now()));
        }
    }

    /// Recolour a highlight: a plain one on its own, a node's for the whole
    /// node (all its highlights), so colour always means the same thing.
    fn recolour_highlight(&mut self, anchor: &AnchorId, color: Option<Color>) {
        let Some(a) = self.ws.graph.anchors.get(anchor).cloned() else { return };
        let tx = if commands::is_highlight_carrier(&a.entity) {
            commands::recolor_anchor(&self.ws.graph, anchor, color.or(Some(self.ws.last_color)))
        } else {
            commands::recolor(&self.ws.graph, &a.entity, color, now_ms())
        };
        if let Ok(tx) = tx {
            let _ = self.ws.commit(tx);
        }
        if let Some(c) = color {
            self.ws.last_color = c;
        }
    }

    /// F-CAP-2 `Ctrl-<n>`: highlight + node filed in system n. Two keystrokes.
    fn capture_into(&mut self, n: u8) {
        let Some(system) = self.ws.graph.system_by_hotkey(n).cloned() else {
            return self.ws.toast_error(format!("no system on Ctrl-{n}"));
        };
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let Some(anchor) = tab.selection_anchor(Some(system.color)) else {
            return self.ws.toast_error("select something first".into());
        };
        let title = Self::selection_title(tab);
        // Open the note field just under the highlight, or inside the page
        // view if the highlight isn't on screen yet (e.g. mid-scroll).
        let fallback = tab.view.center_top() + vec2(-180.0, 80.0);
        let at = tab
            .selection_screen_rect()
            .map(|r| r.left_bottom())
            .filter(|p| tab.view.shrink(40.0).contains(*p))
            .unwrap_or(fallback);
        tab.selection = None;
        // F-CAP-10: an existing node with this exact title gets the anchor
        // instead of a duplicate being created.
        if let Some(existing) = self.ws.graph.find_node_by_title(&title).map(|n| n.id.clone()) {
            if let Ok((tx, _)) = commands::add_anchor(&self.ws.graph, &existing, anchor) {
                let mut tx = tx;
                if !self.ws.graph.in_system(&existing, &system.id)
                    && let Ok(t2) =
                        commands::set_membership(&self.ws.graph, &existing, &system.id, true, now_ms())
                {
                    tx.extend(t2);
                }
                if self.ws.commit(tx).is_ok() {
                    self.ws.note_created(&existing);
                    self.ws.toast(format!("anchored to existing “{title}”"), true);
                    self.open_note(existing, system.id.clone(), at);
                }
            }
            self.ws.last_system = Some(system.id.clone());
            self.ws.last_color = system.color;
            return;
        }
        let (tx, id) = commands::create_node(
            &self.ws.graph,
            NewNode {
                title: title.clone(),
                body: String::new(),
                systems: vec![system.id.clone()],
                anchors: vec![anchor],
            },
            now_ms(),
        );
        if self.ws.commit(tx).is_ok() {
            self.ws.note_created(&id);
            self.ws.last_system = Some(system.id.clone());
            self.ws.last_color = system.color;
            self.ws.toast(format!("{} ∙ {title}", system.name), true);
            self.strip.invalidate();
            self.open_note(id, system.id.clone(), at);
        }
    }

    /// Ask for this entity's note in `system` (opens after every capture).
    fn open_note(&mut self, entity: EntityId, system: SystemId, at: egui::Pos2) {
        let text = self.ws.graph.membership(&entity, &system).map(|m| m.note.clone()).unwrap_or_default();
        self.popup = Popup::Note(NoteEdit { entity, system, text, at, focus_requested: false });
    }

    fn commit_note(&mut self, n: NoteEdit) {
        if !self.ws.graph.contains(&n.entity) {
            return;
        }
        match commands::set_note(&self.ws.graph, &n.entity, &n.system, &n.text, now_ms()) {
            Ok(tx) if !tx.is_empty() => {
                let _ = self.ws.commit(tx);
            }
            Ok(_) => {}
            Err(e) => self.ws.toast_error(e.to_string()),
        }
    }

    /// F-CAP-4 `L`: link the just-created node from the previous one.
    fn link_previous(&mut self) {
        let (Some(new), Some(prev)) = (self.ws.created.front().cloned(), self.ws.created.get(1).cloned())
        else {
            return self.ws.toast_error("need two captured nodes to link".into());
        };
        let kind = self.ws.default_kind();
        if let Some(rel) = self.ws.link(&prev, &new, kind) {
            self.offer_kinds(rel);
        }
    }

    fn offer_kinds(&mut self, rel: EntityId) {
        let palette = commands::kind_palette(&self.ws.graph, self.ws.active_system().as_ref());
        self.kind_offer = Some((rel, palette.into_iter().take(4).collect(), Instant::now()));
    }

    /// F-CAP-5 `l`: hint labels over recent/visible entities.
    fn start_hints(&mut self, source: Option<EntityId>) {
        let Some(source) =
            source.or_else(|| self.ws.focused.clone()).or_else(|| self.ws.created.front().cloned())
        else {
            return self.ws.toast_error("nothing to link from — capture or focus something first".into());
        };
        let view = if self.mode == Mode::Canvas { &self.canvas } else { &self.strip };
        let mut candidates: Vec<EntityId> = Vec::new();
        for id in self.ws.recent.iter().chain(view.screen.keys()) {
            if *id != source && !candidates.contains(id) && self.ws.graph.contains(id) {
                candidates.push(id.clone());
            }
        }
        let labels = hint_labels(candidates.len().min(HINT_CHARS.len() * HINT_CHARS.len()));
        self.popup = Popup::Hints(Hints {
            source,
            labels: labels.into_iter().zip(candidates).collect(),
            typed: String::new(),
            search: None,
        });
    }

    fn finish_link(&mut self, source: EntityId, target: EntityId) {
        let kind = self.ws.default_kind();
        if let Some(rel) = self.ws.link(&source, &target, kind) {
            self.offer_kinds(rel);
            self.canvas.invalidate();
        }
    }

    fn open_composer(&mut self, target: ComposerTarget, at: egui::Pos2) {
        let notes = match &target {
            ComposerTarget::Edit(id) => {
                let g = &self.ws.graph;
                let mut systems: Vec<&System> = g.systems_of(id).filter_map(|s| g.systems.get(s)).collect();
                systems.sort_by_key(|s| (s.hotkey.unwrap_or(99), s.name.clone()));
                systems
                    .into_iter()
                    .filter(|s| !s.inbox)
                    .map(|s| {
                        let note = g.membership(id, &s.id).map(|m| m.note.clone()).unwrap_or_default();
                        (s.id.clone(), s.name.clone(), s.note_prompt.clone(), note)
                    })
                    .collect()
            }
            _ => Vec::new(),
        };
        let (title, body) = match &target {
            ComposerTarget::Edit(id) => (
                match self.ws.graph.relations.get(id) {
                    Some(r) => r.title.clone().unwrap_or_default(),
                    None => self.ws.graph.title(id),
                },
                self.ws.graph.body(id).to_owned(),
            ),
            ComposerTarget::FromSelection { .. } => {
                let tab = self.tabs.get_mut(self.active);
                tab.map(|t| (Self::selection_title(t), String::new())).unwrap_or_default()
            }
            ComposerTarget::Free { .. } => (String::new(), String::new()),
        };
        self.popup = Popup::Composer(Composer { target, title, body, notes, at, focus_requested: false });
    }

    fn commit_composer(&mut self, c: Composer) {
        let now = now_ms();
        match c.target {
            ComposerTarget::Edit(id) => {
                // Title, body and every per-system note: one undoable step.
                if let Ok(mut tx) =
                    commands::annotate(&self.ws.graph, &id, c.title.trim(), c.body.trim(), now)
                {
                    for (system, _, _, note) in &c.notes {
                        if let Ok(t) = commands::set_note(&self.ws.graph, &id, system, note, now) {
                            tx.extend(t);
                        }
                    }
                    let _ = self.ws.commit(tx);
                }
            }
            ComposerTarget::FromSelection { anchor } => {
                if c.title.trim().is_empty() {
                    return;
                }
                let (tx, id) = commands::create_node(
                    &self.ws.graph,
                    NewNode {
                        title: c.title.trim().to_owned(),
                        body: c.body.trim().to_owned(),
                        systems: vec![],
                        anchors: anchor.into_iter().collect(),
                    },
                    now,
                );
                if self.ws.commit(tx).is_ok() {
                    self.ws.note_created(&id);
                    if let Some(t) = self.tab() {
                        t.selection = None;
                    }
                }
            }
            ComposerTarget::Free { link_from, at } => {
                if c.title.trim().is_empty() {
                    return;
                }
                let systems = self.ws.last_system.clone().into_iter().collect();
                let (tx, id) = commands::create_node(
                    &self.ws.graph,
                    NewNode {
                        title: c.title.trim().to_owned(),
                        body: c.body.trim().to_owned(),
                        systems,
                        anchors: vec![],
                    },
                    now,
                );
                if self.ws.commit(tx).is_ok() {
                    self.ws.note_created(&id);
                    if let Some(p) = at {
                        self.canvas.place(&id, p);
                    }
                    if let Some(from) = link_from {
                        self.finish_link(from, id);
                    }
                }
            }
        }
    }

    fn delete_focused(&mut self) {
        let Some(id) = self.ws.focused.clone() else { return };
        let count = self.ws.graph.cascade(&id).len();
        if count > 3 {
            self.popup = Popup::ConfirmDelete { target: id, count };
        } else {
            self.delete(&id);
        }
    }

    fn delete(&mut self, id: &EntityId) {
        if let Ok(tx) = commands::delete_entity(&self.ws.graph, id) {
            let label = tx.label.clone();
            if self.ws.commit(tx).is_ok() {
                self.ws.forget_missing();
                self.ws.toast(label, true);
            }
        }
    }

    /// F-NODE-3: focused entity → its source passage.
    fn jump_to_source(&mut self) {
        let Some(id) = self.ws.focused.clone() else { return };
        let anchor = self.ws.graph.anchors_of(&id).find(|a| !a.is_detached()).cloned();
        let Some(anchor) = anchor else {
            return self.ws.toast_error("no anchor for this entity".into());
        };
        let Some(doc) = self.ws.graph.documents.get(&anchor.document).cloned() else { return };
        if !Path::new(&doc.path).exists() {
            return self.ws.toast_error(format!("file missing: {} — relink it in Documents", doc.path));
        }
        self.open_document(doc);
        if let Some(t) = self.tab() {
            t.reveal(anchor.page_index, anchor.quads.clone());
        }
        self.mode = Mode::Reader;
    }

    fn toggle_overlay(&mut self, n: u8) {
        if let Some(s) = self.ws.graph.system_by_hotkey(n).map(|s| s.id.clone()) {
            self.overlay.toggle(&s);
            self.overlay_changed();
        }
    }

    fn overlay_changed(&mut self) {
        self.overlay_rev += 1;
    }

    fn focus(&mut self, id: Option<EntityId>) {
        self.ws.focus(id);
        self.edge_cycle = None;
    }

    // ----------------------------------------------------------------- keys

    fn handle_keys(&mut self, ctx: &egui::Context) {
        // Only a focused *text field* means the user is typing. Any other
        // widget holding focus (a button that was clicked, egui's own Tab
        // navigation) must not swallow shortcuts, so it gives focus back.
        let typing = ctx.text_edit_focused();
        if !typing && let Some(id) = ctx.memory(|m| m.focused()) {
            ctx.memory_mut(|m| m.surrender_focus(id));
        }
        let intercepted = std::mem::take(&mut self.intercepted);
        let events: Vec<(Key, Modifiers)> = intercepted
            .into_iter()
            .chain(ctx.input(|i| {
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::Key { key, physical_key, pressed: true, modifiers, .. } => {
                            // Digits by physical position so Shift-1 is still "1".
                            let k = match physical_key {
                                Some(p) if is_digit(*p) => *p,
                                _ => *key,
                            };
                            Some((k, *modifiers))
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            }))
            .collect();
        for (key, m) in events {
            self.handle_key(ctx, key, m, typing);
        }
    }

    fn handle_key(&mut self, ctx: &egui::Context, key: Key, m: Modifiers, typing: bool) {
        let cmd = m.command || m.ctrl;
        // Popups first.
        match &mut self.popup {
            // Saving (Enter / Ctrl-Enter) happens in `popups`, after the text
            // fields have taken in this frame's typing; here only Esc.
            Popup::Composer(_) | Popup::Note(_) => {
                if key == Key::Escape {
                    self.popup = Popup::None;
                }
                return;
            }
            Popup::Hints(h) => {
                if key == Key::Escape {
                    self.popup = Popup::None;
                    return;
                }
                if h.search.is_some() {
                    return; // typing into the fuzzy box
                }
                if key == Key::Slash {
                    h.search = Some(String::new());
                    return;
                }
                if let Some(c) = key_char(key) {
                    h.typed.push(c);
                    let typed = h.typed.clone();
                    if let Some((_, target)) = h.labels.iter().find(|(l, _)| *l == typed) {
                        let (source, target) = (h.source.clone(), target.clone());
                        self.popup = Popup::None;
                        self.finish_link(source, target);
                    } else if !h.labels.iter().any(|(l, _)| l.starts_with(&typed)) {
                        h.typed.clear();
                    }
                }
                return;
            }
            Popup::Systems { target } => {
                let target = target.clone();
                if let Some(n) = digit(key) {
                    if let Some(s) = self.ws.graph.system_by_hotkey(n).map(|s| s.id.clone()) {
                        let on = !self.ws.graph.in_system(&target, &s);
                        if let Ok(tx) = commands::set_membership(&self.ws.graph, &target, &s, on, now_ms()) {
                            let _ = self.ws.commit(tx);
                        }
                    }
                } else if matches!(key, Key::Escape | Key::Enter | Key::S) {
                    self.popup = Popup::None;
                }
                return;
            }
            Popup::Kinds { target } => {
                let target = target.clone();
                if let Some(n) = digit(key) {
                    let palette = commands::kind_palette(&self.ws.graph, self.ws.active_system().as_ref());
                    if let Some(k) = palette.get(n as usize - 1)
                        && let Ok(tx) = commands::set_kind(&self.ws.graph, &target, k, now_ms())
                    {
                        let _ = self.ws.commit(tx);
                        self.ws.last_kind = Some(k.clone());
                    }
                    self.popup = Popup::None;
                } else if matches!(key, Key::Escape | Key::Enter | Key::K) {
                    self.popup = Popup::None;
                }
                return;
            }
            Popup::ConfirmDelete { target, .. } => {
                let target = target.clone();
                if key == Key::Enter || key == Key::Y {
                    self.popup = Popup::None;
                    self.delete(&target);
                } else if key == Key::Escape || key == Key::N {
                    self.popup = Popup::None;
                }
                return;
            }
            Popup::Help => {
                if matches!(key, Key::Escape | Key::Questionmark | Key::F1) {
                    self.popup = Popup::None;
                }
                return;
            }
            Popup::Menu { .. } => {
                if key == Key::Escape {
                    self.popup = Popup::None;
                }
                return;
            }
            Popup::None => {}
        }
        if typing {
            if key == Key::Escape {
                ctx.memory_mut(|mem| mem.stop_text_input());
            }
            return;
        }
        if key == Key::Escape && self.link_from.take().is_some() {
            return;
        }
        // One keypress (1–8) recolours the highlight just made with `h`.
        if let Some((aid, at)) = self.colour_offer.clone() {
            self.colour_offer = None;
            if at.elapsed().as_secs_f32() < KIND_OFFER_SECS
                && !cmd
                && let Some(n) = digit(key)
                && let Some((_, c)) = PALETTE.get(n as usize - 1)
            {
                self.recolour_highlight(&aid, Some(*c));
                return;
            }
        }
        // One-keypress kind override after a link (F-CAP-6).
        if let Some((rel, kinds, at)) = self.kind_offer.clone() {
            if at.elapsed().as_secs_f32() < KIND_OFFER_SECS
                && !cmd
                && let Some(n) = digit(key)
                && let Some(k) = kinds.get(n as usize - 1)
            {
                if let Ok(tx) = commands::set_kind(&self.ws.graph, &rel, k, now_ms()) {
                    let _ = self.ws.commit(tx);
                    self.ws.last_kind = Some(k.clone());
                }
                self.kind_offer = None;
                return;
            }
            self.kind_offer = None;
        }

        // Shared across modes.
        match (key, cmd, m.shift) {
            (Key::Z, true, false) => return self.ws.undo(),
            (Key::Z, true, true) | (Key::Y, true, _) => return self.ws.redo(),
            (Key::Tab, false, _) => {
                self.mode = if self.mode == Mode::Reader { Mode::Canvas } else { Mode::Reader };
                if self.mode == Mode::Canvas
                    && let Some(f) = &self.ws.focused
                {
                    self.canvas.center_on(f);
                }
                return;
            }
            (Key::F1, _, _) | (Key::Questionmark, _, _) => {
                self.popup = Popup::Help;
                return;
            }
            (Key::S, false, false) => {
                if let Some(t) = self.ws.focused.clone() {
                    self.popup = Popup::Systems { target: t };
                }
                return;
            }
            (Key::ArrowUp | Key::ArrowDown, true, true) => {
                if let Some(f) = self.ws.focused.clone()
                    && self.ws.graph.nodes.contains_key(&f)
                {
                    let d = if key == Key::ArrowUp { 1 } else { -1 };
                    if let Ok(tx) = commands::shift_level(&self.ws.graph, &f, d, now_ms()) {
                        let _ = self.ws.commit(tx);
                    }
                }
                return;
            }
            _ => {}
        }
        if let Some(n) = digit(key)
            && cmd
            && m.shift
        {
            // Ctrl-Shift-n: friction on a focused relation (1..5), else overlay n.
            let focused_rel = self.ws.focused.clone().filter(|f| self.ws.graph.is_relation(f));
            if let (Some(rel), true, Mode::Canvas) = (focused_rel, n <= 5, self.mode) {
                let f = [0.1, 0.3, 0.5, 0.7, 0.9][n as usize - 1];
                if let Ok(tx) = commands::set_friction(&self.ws.graph, &rel, Some(f), now_ms()) {
                    let _ = self.ws.commit(tx);
                    self.ws.toast(format!("friction {f}"), false);
                }
            } else {
                self.toggle_overlay(n);
            }
            return;
        }
        match self.mode {
            Mode::Reader => self.reader_key(ctx, key, m, cmd),
            Mode::Canvas => self.canvas_key(ctx, key, m, cmd),
        }
    }

    fn reader_key(&mut self, ctx: &egui::Context, key: Key, m: Modifiers, cmd: bool) {
        let has_selection = self.tabs.get(self.active).is_some_and(|t| t.selection.is_some());
        if let Some(n) = digit(key)
            && cmd
        {
            return self.capture_into(n);
        }
        match (key, cmd, m.shift) {
            (Key::H, false, false) => self.highlight(),
            (Key::N, false, true) if has_selection => {
                let at = self
                    .tab()
                    .and_then(|t| t.selection_screen_rect())
                    .map(|r| r.left_bottom())
                    .unwrap_or_default();
                let anchor = self.tab().and_then(|t| t.selection_anchor(None));
                self.open_composer(ComposerTarget::FromSelection { anchor }, at);
            }
            (Key::N, false, shift) => {
                // n / N cycle search hits while a search has results; with
                // none, N edits the focused node's title and notes.
                let searching = self.tabs.get(self.active).is_some_and(|t| !t.search.matches.is_empty());
                if searching || !shift {
                    if let Some(t) = self.tab() {
                        t.next_match(if shift { -1 } else { 1 });
                    }
                } else if let Some(f) = self.ws.focused.clone() {
                    let at = self.tab().map(|t| t.view.center_top() + vec2(-180.0, 80.0)).unwrap_or_default();
                    self.open_composer(ComposerTarget::Edit(f), at);
                }
            }
            (Key::L, false, true) => self.link_previous(),
            (Key::L, false, false) => self.start_hints(None),
            (Key::A, false, false) => {
                let Some(f) = self.ws.focused.clone() else {
                    return self.ws.toast_error("focus a node first (click its highlight)".into());
                };
                let color = Some(self.ws.graph.entity_color(&f));
                if let Some(anchor) = self.tab().and_then(|t| t.selection_anchor(color))
                    && let Ok((tx, _)) = commands::add_anchor(&self.ws.graph, &f, anchor)
                    && self.ws.commit(tx).is_ok()
                {
                    self.ws.toast(format!("anchor added to “{}”", self.ws.graph.title(&f)), true);
                    if let Some(t) = self.tab() {
                        t.selection = None;
                    }
                }
            }
            (Key::Slash, false, _) => {
                if let Some(t) = self.tab() {
                    t.search.open = true;
                }
                ctx.memory_mut(|mem| mem.request_focus(egui::Id::new("doc-search")));
            }
            (Key::Enter, true, _) => {
                self.mode = Mode::Canvas;
                if let Some(f) = self.ws.focused.clone() {
                    self.canvas.center_on(&f);
                }
            }
            (Key::G, false, false) => {
                if let Some(t) = self.tab() {
                    t.goto_top();
                }
            }
            (Key::G, false, true) => {
                if let Some(t) = self.tab() {
                    t.goto_bottom();
                }
            }
            (Key::Escape, _, _) => {
                if let Some(t) = self.tab() {
                    if t.selection.is_some() {
                        t.selection = None;
                    } else {
                        t.search = Default::default();
                    }
                }
            }
            (Key::Plus | Key::Equals, true, _) => {
                if let Some(t) = self.tab() {
                    t.set_zoom(t.zoom * 1.2, None);
                }
            }
            (Key::Minus, true, _) => {
                if let Some(t) = self.tab() {
                    t.set_zoom(t.zoom / 1.2, None);
                }
            }
            (Key::Num0, true, _) => {
                if let Some(t) = self.tab() {
                    t.fit = Fit::Width;
                    t.view = egui::Rect::NOTHING;
                }
            }
            (Key::ArrowLeft, _, _) if m.alt => {
                if let Some(t) = self.tab() {
                    t.go_back();
                }
            }
            (Key::ArrowRight, _, _) if m.alt => {
                if let Some(t) = self.tab() {
                    t.go_forward();
                }
            }
            (Key::Space | Key::PageDown, false, shift) => {
                if let Some(t) = self.tab() {
                    t.scroll_by_pages(if shift { -1.0 } else { 1.0 });
                }
            }
            (Key::PageUp, false, _) => {
                if let Some(t) = self.tab() {
                    t.scroll_by_pages(-1.0);
                }
            }
            (Key::J | Key::ArrowDown, false, false) => {
                if let Some(t) = self.tab() {
                    t.scroll.y += 60.0;
                }
            }
            (Key::K | Key::ArrowUp, false, false) => {
                if let Some(t) = self.tab() {
                    t.scroll.y -= 60.0;
                }
            }
            (Key::Backspace | Key::Delete, true, _) => self.delete_focused(),
            _ => {}
        }
    }

    fn canvas_key(&mut self, _ctx: &egui::Context, key: Key, m: Modifiers, cmd: bool) {
        let focused = self.ws.focused.clone();
        let dir = match key {
            Key::H | Key::ArrowLeft if !cmd => Some(vec2(-1.0, 0.0)),
            Key::L | Key::ArrowRight if !cmd && key == Key::ArrowRight => Some(vec2(1.0, 0.0)),
            Key::J | Key::ArrowDown if !cmd && !m.shift => Some(vec2(0.0, 1.0)),
            Key::K | Key::ArrowUp if !cmd && !m.shift && key == Key::ArrowUp => Some(vec2(0.0, -1.0)),
            _ => None,
        };
        // `l` links and `k` changes kind; hjkl movement uses h/j and arrows,
        // with Shift+L / Shift+K… no: PRD maps hjkl to movement *and* l/k to
        // link/kind. Arrows always move; h and j move; l and k keep their
        // command meaning.
        if let Some(d) = dir {
            if let Some(f) = &focused
                && let Some(n) = self.canvas.neighbor_in_direction(&self.ws.graph, f, d)
            {
                self.focus(Some(n));
            } else if focused.is_none()
                && let Some(first) = self.ws.recent.front().cloned()
            {
                self.focus(Some(first));
            }
            return;
        }
        match (key, cmd, m.shift) {
            (Key::E, false, _) => {
                // Cycle the relations of the focused node (or of the node we
                // started cycling from).
                let origin = match &self.edge_cycle {
                    Some((o, _)) if focused.as_ref().is_some_and(|f| self.ws.graph.is_relation(f)) => {
                        Some(o.clone())
                    }
                    _ => focused.clone(),
                };
                if let Some(o) = origin {
                    let rels: Vec<EntityId> = self.ws.graph.incident(&o).cloned().collect();
                    if !rels.is_empty() {
                        let i = match &self.edge_cycle {
                            Some((c, i)) if *c == o => (i + 1) % rels.len(),
                            _ => 0,
                        };
                        self.ws.focus(Some(rels[i].clone()));
                        self.edge_cycle = Some((o, i));
                    }
                }
            }
            (Key::Enter, false, _) => self.jump_to_source(),
            (Key::N, false, true) | (Key::F2, _, _) => {
                if let Some(f) = focused {
                    let at = self.canvas.screen.get(&f).copied().unwrap_or(self.canvas.rect.center());
                    self.open_composer(ComposerTarget::Edit(f), at);
                }
            }
            (Key::N, true, _) => {
                let at = self.canvas.rect.center();
                self.open_composer(ComposerTarget::Free { link_from: None, at: None }, at);
            }
            (Key::L, false, false) => {
                // Two selected entities: link them (F-REL-2).
                if self.ws.selected.len() == 2 {
                    let (a, b) = (self.ws.selected[0].clone(), self.ws.selected[1].clone());
                    self.ws.selected.clear();
                    self.finish_link(a, b);
                } else {
                    self.start_hints(focused);
                }
            }
            (Key::L, false, true) => {
                // Link from the focus to the previously created/touched entity.
                if let Some(f) = focused
                    && let Some(prev) = self.ws.recent.iter().find(|e| **e != f).cloned()
                {
                    self.finish_link(prev, f);
                }
            }
            (Key::K, false, false) => {
                if let Some(f) = focused.filter(|f| self.ws.graph.is_relation(f)) {
                    self.popup = Popup::Kinds { target: f };
                }
            }
            (Key::R, true, _) => {
                if let Some(f) = focused.filter(|f| self.ws.graph.is_relation(f))
                    && let Ok(tx) = commands::reverse(&self.ws.graph, &f, now_ms())
                {
                    let _ = self.ws.commit(tx);
                }
            }
            (Key::F, false, false) => {
                self.canvas.focus_mode = match (&self.canvas.focus_mode, focused) {
                    (Some(_), _) => None,
                    (None, Some(f)) => Some((f, 2)),
                    (None, None) => None,
                };
                self.canvas.invalidate();
                self.canvas.fit_next_frame();
            }
            (Key::OpenBracket, false, _) | (Key::CloseBracket, false, _) => {
                if let Some((_, k)) = &mut self.canvas.focus_mode {
                    *k = if key == Key::OpenBracket { k.saturating_sub(1).max(1) } else { (*k + 1).min(8) };
                    self.canvas.invalidate();
                    self.canvas.fit_next_frame();
                }
            }
            (Key::G, false, true) => {
                self.overlay.ghost = !self.overlay.ghost;
                self.overlay_changed();
            }
            (Key::I, false, false) => {
                if let Some(s) = focused.and_then(|f| self.ws.graph.primary_system(&f).map(|s| s.id.clone()))
                {
                    self.isolate_saved.get_or_insert_with(|| self.overlay.clone());
                    self.overlay.active = vec![s];
                    self.overlay.mode = Combine::Union;
                    self.overlay_changed();
                }
            }
            (Key::Escape, _, _) => {
                if let Some(saved) = self.isolate_saved.take() {
                    self.overlay = saved;
                    self.overlay_changed();
                } else if self.canvas.focus_mode.take().is_some() {
                    self.canvas.invalidate();
                } else {
                    self.ws.selected.clear();
                    self.focus(None);
                }
            }
            (Key::Z, false, false) => self.canvas.fit_next_frame(),
            (Key::Backspace | Key::Delete, true, _) => self.delete_focused(),
            _ => {}
        }
    }

    // ------------------------------------------------------------------- ui

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Soma").strong());
            ui.separator();
            let mut close = None;
            for (i, t) in self.tabs.iter().enumerate() {
                let title = t.doc.title.clone().unwrap_or_else(|| t.doc.path.clone());
                let short: String = title.chars().take(28).collect();
                let sel = i == self.active && self.mode == Mode::Reader;
                if ui.selectable_label(sel, short).on_hover_text(&t.doc.path).clicked() {
                    self.active = i;
                    self.mode = Mode::Reader;
                }
                if ui.small_button("×").clicked() {
                    close = Some(i);
                }
            }
            if let Some(i) = close {
                self.tabs.remove(i);
                self.active = self.active.min(self.tabs.len().saturating_sub(1));
            }
            if ui.button("Open PDF…").clicked()
                && let Some(p) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).pick_file()
            {
                self.open_pdf(&p);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if self.mode == Mode::Canvas { "📄 Reader (Tab)" } else { "⊞ Canvas (Tab)" };
                if ui.button(label).clicked() {
                    self.mode = if self.mode == Mode::Canvas { Mode::Reader } else { Mode::Canvas };
                }
                if ui.button("?").on_hover_text("Keymap (F1)").clicked() {
                    self.popup = Popup::Help;
                }
                let undo = ui.button("⟲").on_hover_text("Undo (Ctrl-Z)");
                self.undo_rect = undo.rect;
                if undo.clicked() {
                    self.ws.undo();
                }
                if ui.button("⟳").on_hover_text("Redo (Ctrl-Shift-Z)").clicked() {
                    self.ws.redo();
                }
                if self.mode == Mode::Reader {
                    ui.separator();
                    // Default colour for `h` highlights.
                    let swatch = RichText::new("⏺ h colour").color(to_color32(self.ws.last_color));
                    ui.menu_button(swatch, |ui| {
                        for (i, (name, c)) in PALETTE.iter().enumerate() {
                            let text = RichText::new(format!("⏺ {} {name}", i + 1)).color(to_color32(*c));
                            if ui.selectable_label(self.ws.last_color == *c, text).clicked() {
                                self.ws.last_color = *c;
                                ui.close();
                            }
                        }
                    });
                    egui::ComboBox::from_id_salt("theme")
                        .selected_text(match self.theme {
                            PaperTheme::Light => "Light",
                            PaperTheme::Sepia => "Sepia",
                            PaperTheme::Dark => "Dark",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.theme, PaperTheme::Light, "Light");
                            ui.selectable_value(&mut self.theme, PaperTheme::Sepia, "Sepia");
                            ui.selectable_value(&mut self.theme, PaperTheme::Dark, "Dark");
                        });
                    ui.toggle_value(&mut self.strip_open, "Graph strip");
                    if let Some(t) = self.tabs.get_mut(self.active) {
                        ui.toggle_value(&mut t.spread, "Spread");
                        if ui.selectable_label(t.fit == Fit::Page, "Fit page").clicked() {
                            t.fit = Fit::Page;
                            t.view = egui::Rect::NOTHING;
                        }
                        if ui.selectable_label(t.fit == Fit::Width, "Fit width").clicked() {
                            t.fit = Fit::Width;
                            t.view = egui::Rect::NOTHING;
                        }
                        if ui.small_button("+").clicked() {
                            t.set_zoom(t.zoom * 1.2, None);
                        }
                        ui.label(format!("{:.0}%", t.zoom * 100.0));
                        if ui.small_button("−").clicked() {
                            t.set_zoom(t.zoom / 1.2, None);
                        }
                        ui.label(format!("p. {}/{}", t.current_page() + 1, t.page_count()));
                    }
                }
            });
        });
    }

    fn systems_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Overlays").strong());
        let mut changed = false;
        ui.horizontal(|ui| {
            changed |= ui
                .selectable_value(&mut self.overlay.mode, Combine::Union, "any")
                .on_hover_text("union")
                .changed();
            changed |= ui
                .selectable_value(&mut self.overlay.mode, Combine::Intersection, "all")
                .on_hover_text("intersection")
                .changed();
            changed |= ui
                .selectable_value(&mut self.overlay.mode, Combine::Difference, "minus")
                .on_hover_text("difference: first minus the rest")
                .changed();
            changed |= ui.checkbox(&mut self.overlay.ghost, "ghost (G)").changed();
        });
        ui.add_space(4.0);
        let mut systems: Vec<System> = self.ws.graph.systems.values().cloned().collect();
        systems.sort_by_key(|s| (s.hotkey.unwrap_or(99), s.name.clone()));
        let mut prompt_saves: Vec<(SystemId, String)> = Vec::new();
        for s in &systems {
            ui.horizontal(|ui| {
                let mut on = self.overlay.is_active(&s.id);
                let (rect, _) = ui.allocate_exact_size(vec2(12.0, 12.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 5.5, to_color32(s.color));
                if ui.checkbox(&mut on, "").on_hover_text("show in overlay").changed() {
                    self.overlay.toggle(&s.id);
                    changed = true;
                }
                let key = s.hotkey.map(|h| format!("^{h} ")).unwrap_or_default();
                let count = self.ws.graph.members_of(&s.id).count();
                let active = self.ws.last_system.as_ref() == Some(&s.id);
                let text = RichText::new(format!("{key}{}  ({count})", s.name));
                let r = ui.selectable_label(active, if active { text.strong() } else { text });
                let r = if s.note_prompt.is_empty() { r } else { r.on_hover_text(&s.note_prompt) };
                if r.clicked() {
                    self.ws.last_system = Some(s.id.clone());
                    if s.hotkey.is_some() {
                        self.ws.last_color = s.color;
                    }
                }
                // Right-click: edit the question this system asks on capture.
                r.context_menu(|ui| {
                    ui.label(RichText::new("Question asked when filing here").weak());
                    let draft =
                        self.prompt_drafts.entry(s.id.clone()).or_insert_with(|| s.note_prompt.clone());
                    ui.add(egui::TextEdit::singleline(draft).desired_width(260.0));
                    if ui.button("Save").clicked() {
                        prompt_saves.push((s.id.clone(), draft.clone()));
                        ui.close();
                    }
                });
            });
        }
        for (id, prompt) in prompt_saves {
            self.prompt_drafts.remove(&id);
            if let Ok(tx) =
                commands::update_system(&self.ws.graph, &id, |s| s.note_prompt = prompt.trim().to_owned())
            {
                let _ = self.ws.commit(tx);
            }
        }
        if changed {
            self.overlay_changed();
        }
        ui.horizontal(|ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.new_system)
                    .hint_text("new system…")
                    .desired_width(130.0),
            );
            if (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) || ui.button("+").clicked())
                && !self.new_system.trim().is_empty()
            {
                let used: Vec<u8> = self.ws.graph.systems.values().filter_map(|s| s.hotkey).collect();
                let hotkey = (1..=9).find(|h| !used.contains(h));
                let palette = [
                    Color::rgb(0xe8, 0x6a, 0x92),
                    Color::rgb(0x4c, 0xb8, 0xb0),
                    Color::rgb(0xc9, 0xa2, 0x27),
                    Color::rgb(0x8a, 0x8f, 0xe6),
                    Color::rgb(0x9c, 0xc4, 0x4e),
                ];
                let color = palette[self.ws.graph.systems.len() % palette.len()];
                let (tx, _) = commands::create_system(self.new_system.trim(), color, hotkey);
                let _ = self.ws.commit(tx);
                self.new_system.clear();
            }
        });
    }

    fn documents_panel(&mut self, ui: &mut egui::Ui) {
        let mut open = None;
        let mut relink = None;
        for d in self.ws.documents_sorted() {
            let exists = Path::new(&d.path).exists();
            ui.horizontal(|ui| {
                let title = d.title.clone().unwrap_or_else(|| d.path.clone());
                let anchors = self.ws.graph.anchors_in_doc(&d.id).count();
                let r =
                    ui.add_enabled(exists, egui::Button::new(format!("{title}  ({anchors})")).frame(false));
                if r.on_hover_text(&d.path).clicked() {
                    open = Some(d.clone());
                }
                if !exists {
                    ui.colored_label(Color32::from_rgb(220, 80, 60), "missing");
                }
                if ui
                    .small_button("relink…")
                    .on_hover_text("point at a new copy; re-anchors if it changed")
                    .clicked()
                {
                    relink = Some(d.clone());
                }
            });
        }
        if let Some(d) = open {
            self.open_document(d);
        }
        if let Some(d) = relink
            && let Some(p) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).pick_file()
        {
            self.relink(&d, &p);
        }
    }

    fn left_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.left_tab, LeftTab::Systems, "Systems");
            ui.selectable_value(&mut self.left_tab, LeftTab::Outline, "Outline");
            ui.selectable_value(&mut self.left_tab, LeftTab::Documents, "Docs");
        });
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| match self.left_tab {
            LeftTab::Systems => self.systems_panel(ui),
            LeftTab::Documents => self.documents_panel(ui),
            LeftTab::Outline => {
                let mut goto = None;
                if let Some(t) = self.tabs.get(self.active) {
                    if t.outline.is_empty() {
                        ui.weak("No outline in this document.");
                    }
                    for item in &t.outline {
                        ui.horizontal(|ui| {
                            ui.add_space(item.depth as f32 * 12.0);
                            if ui.add(egui::Button::new(&item.title).frame(false)).clicked() {
                                goto = item.page;
                            }
                        });
                    }
                }
                if let Some(p) = goto
                    && let Some(t) = self.tab()
                {
                    t.goto(p, 0.0, true);
                }
            }
        });
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.ws.focused.clone().filter(|f| self.ws.graph.contains(f)) else {
            ui.weak("Nothing focused. Click a node or highlight.");
            self.roots_report(ui);
            return;
        };
        let g = &self.ws.graph;
        let is_rel = g.is_relation(&id);
        ui.label(RichText::new(g.title(&id)).strong().size(15.0));
        if is_rel {
            let r = &g.relations[&id];
            let state = g.relation_state(&id).unwrap();
            ui.label(format!("relation · {} · {:?}", r.kind, state));
            if let Some(f) = g.effective_friction(&id) {
                ui.label(format!(
                    "friction {f:.1}{}",
                    if r.friction.is_none() { " (inferred, cross-level)" } else { "" }
                ));
            }
            if g.relations_in_cycles().contains(&id) {
                ui.colored_label(Color32::from_rgb(230, 90, 60), "⚠ part of a cycle in an acyclic kind");
            }
        } else if let Some(n) = g.nodes.get(&id) {
            ui.label(format!(
                "node · level {}",
                n.abstraction_level.map(|l| format!("{l:+}")).unwrap_or("—".into())
            ));
        }
        ui.add_space(6.0);
        notes_ui(ui, g, &id);
        let mut focus_to = None;
        let mut jump = false;
        let anchors: Vec<Anchor> = g.anchors_of(&id).cloned().collect();
        if !anchors.is_empty() {
            ui.separator();
            ui.label(RichText::new("Anchors").strong());
            for a in &anchors {
                let doc = g.documents.get(&a.document).and_then(|d| d.title.clone()).unwrap_or_default();
                let label = format!(
                    "p.{} {} “{}”{}",
                    a.page_index + 1,
                    doc,
                    a.exact.chars().take(40).collect::<String>(),
                    if a.is_detached() {
                        " — detached"
                    } else if a.confidence < 0.9 {
                        " — low confidence"
                    } else {
                        ""
                    }
                );
                if ui.add(egui::Button::new(label).frame(false)).clicked() {
                    jump = true;
                }
            }
        }
        let rels: Vec<EntityId> = g.incident(&id).cloned().collect();
        let ends: Vec<EntityId> =
            g.relations.get(&id).map(|r| r.endpoint_ids().cloned().collect()).unwrap_or_default();
        if !rels.is_empty() || !ends.is_empty() {
            ui.separator();
            ui.label(RichText::new("Relations").strong());
            for e in ends {
                if ui.add(egui::Button::new(format!("- {}", g.title(&e))).frame(false)).clicked() {
                    focus_to = Some(e);
                }
            }
            for r in rels {
                let rel = &g.relations[&r];
                let other = rel.other(&id).map(|o| g.title(o)).unwrap_or_default();
                let arrow = if rel.source() == Some(&id) { "->" } else { "<-" };
                if ui.add(egui::Button::new(format!("{arrow} {} {other}", rel.kind)).frame(false)).clicked() {
                    focus_to = Some(r);
                }
            }
        }
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            if ui.button("Edit (N)").clicked() {
                let at = ui.cursor().min;
                self.open_composer(ComposerTarget::Edit(id.clone()), at);
            }
            if ui.button("Systems (s)").clicked() {
                self.popup = Popup::Systems { target: id.clone() };
            }
            if is_rel && ui.button("Kind (k)").clicked() {
                self.popup = Popup::Kinds { target: id.clone() };
            }
            if ui.button("Link from here").clicked() {
                self.link_from = Some(id.clone());
            }
            if ui.button("Delete").clicked() {
                self.delete_focused();
            }
        });
        if self.ws.graph.nodes.contains_key(&id) {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Colour").weak());
                for (name, c) in PALETTE {
                    let (r, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), egui::Sense::click());
                    ui.painter().circle_filled(r.center(), 7.0, to_color32(c));
                    if resp.on_hover_text(name).clicked()
                        && let Ok(tx) = commands::recolor(&self.ws.graph, &id, Some(c), now_ms())
                    {
                        let _ = self.ws.commit(tx);
                    }
                }
                if ui.small_button("reset").clicked()
                    && let Ok(tx) = commands::recolor(&self.ws.graph, &id, None, now_ms())
                {
                    let _ = self.ws.commit(tx);
                }
            });
        }
        if jump {
            self.jump_to_source();
        }
        if let Some(f) = focus_to {
            self.focus(Some(f));
        }
    }

    /// F-GRAPH-9: roots of the prerequisite DAG.
    fn roots_report(&mut self, ui: &mut egui::Ui) {
        let k = KindId::from(defaults::PREREQUISITE_OF);
        let roots = self.ws.graph.roots(&k);
        if roots.is_empty() {
            return;
        }
        ui.separator();
        ui.label(
            RichText::new(format!("Start here — {} root(s) with no prerequisites", roots.len())).strong(),
        );
        let mut focus = None;
        for r in roots.iter().take(30) {
            if ui.add(egui::Button::new(self.ws.graph.title(r)).frame(false)).clicked() {
                focus = Some(r.clone());
            }
        }
        if let Some(f) = focus {
            self.focus(Some(f.clone()));
            self.canvas.center_on(&f);
        }
    }

    fn canvas_controls(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Layout").strong());
        ui.horizontal_wrapped(|ui| {
            for (m, name) in [
                (LayoutMode::Force, "Force"),
                (LayoutMode::Layered, "Layered"),
                (LayoutMode::Radial, "Radial"),
                (LayoutMode::Manual, "Manual"),
            ] {
                if ui.selectable_label(self.canvas.mode == m, name).clicked() {
                    self.canvas.set_mode(m);
                }
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Fit (z)").clicked() {
                self.canvas.fit_next_frame();
            }
            if ui.button("Unpin all").clicked() {
                self.canvas.force.pinned.clear();
                self.canvas.force.reheat(0.6);
            }
        });
        if let Some((_, k)) = &self.canvas.focus_mode {
            ui.label(format!("Focus mode: {k} hop(s)  ([ / ] to adjust, f to exit)"));
        }
        ui.add_space(4.0);
        let r = ui.add(
            egui::TextEdit::singleline(&mut self.canvas.filter)
                .hint_text("filter: words system: kind: level: doc: detached")
                .desired_width(f32::INFINITY),
        );
        if r.changed() {
            self.canvas.invalidate();
        }
        ui.separator();
        self.systems_panel(ui);
    }

    fn popups(&mut self, ctx: &egui::Context) {
        match &mut self.popup {
            Popup::None | Popup::Menu { .. } => {}
            Popup::Composer(c) => {
                let mut commit = false;
                let mut cancel = false;
                let titles: Vec<String> = self.ws.graph.nodes.values().map(|n| n.title.clone()).collect();
                egui::Window::new("composer")
                    .title_bar(false)
                    .frame(opaque_frame(ctx))
                    .fade_in(false)
                    .fixed_pos(c.at + vec2(0.0, 6.0))
                    .resizable(false)
                    .show(ctx, |ui| {
                        ui.set_width(380.0);
                        let label = match &c.target {
                            ComposerTarget::Edit(_) => "Edit",
                            ComposerTarget::FromSelection { .. } => "New node from selection",
                            ComposerTarget::Free { link_from: Some(_), .. } => "New node, linked",
                            ComposerTarget::Free { .. } => "New node",
                        };
                        ui.label(RichText::new(label).weak());
                        let enter = ui.input(|i| i.key_pressed(Key::Enter) && i.modifiers.command);
                        let t = ui.add(
                            egui::TextEdit::singleline(&mut c.title)
                                .hint_text("title")
                                .desired_width(f32::INFINITY),
                        );
                        if !c.focus_requested {
                            t.request_focus();
                            c.focus_requested = true;
                        }
                        for (_, name, prompt, note) in c.notes.iter_mut() {
                            ui.add_space(4.0);
                            ui.label(RichText::new(name.as_str()).strong());
                            ui.add(
                                egui::TextEdit::multiline(note)
                                    .hint_text(prompt.as_str())
                                    .desired_rows(2)
                                    .desired_width(f32::INFINITY),
                            );
                        }
                        ui.add_space(4.0);
                        ui.label(RichText::new("General note").weak());
                        ui.add(
                            egui::TextEdit::multiline(&mut c.body)
                                .hint_text("anything else (markdown; [[ links to entities)")
                                .desired_rows(3)
                                .desired_width(f32::INFINITY),
                        );
                        // `[[` autocomplete (F-NODE-2).
                        if let Some(start) = c.body.rfind("[[")
                            && !c.body[start..].contains("]]")
                        {
                            let q = c.body[start + 2..].to_lowercase();
                            let hits: Vec<&String> =
                                titles.iter().filter(|t| t.to_lowercase().contains(&q)).take(6).collect();
                            for h in hits {
                                if ui.small_button(h.as_str()).clicked() {
                                    c.body.truncate(start);
                                    c.body.push_str(&format!("[[{h}]]"));
                                }
                            }
                        }
                        ui.horizontal(|ui| {
                            commit = ui.button("Save (Ctrl-Enter)").clicked();
                            cancel = ui.button("Cancel (Esc)").clicked();
                        });
                        commit |= enter;
                    });
                if cancel {
                    self.popup = Popup::None;
                } else if commit && let Popup::Composer(c) = std::mem::replace(&mut self.popup, Popup::None) {
                    self.commit_composer(c);
                }
            }
            Popup::Note(n) => {
                let mut save = false;
                let mut skip = false;
                let sys = self.ws.graph.systems.get(&n.system).cloned();
                let title = self.ws.graph.title(&n.entity);
                egui::Window::new("note")
                    .title_bar(false)
                    .frame(opaque_frame(ctx))
                    .fade_in(false)
                    .fixed_pos(n.at + vec2(0.0, 8.0))
                    .resizable(false)
                    .show(ctx, |ui| {
                        ui.set_width(360.0);
                        ui.horizontal(|ui| {
                            if let Some(s) = &sys {
                                let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::hover());
                                ui.painter().circle_filled(r.center(), 5.0, to_color32(s.color));
                                ui.label(RichText::new(&s.name).strong());
                            }
                            ui.label(
                                RichText::new(format!("· {}", title.chars().take(40).collect::<String>()))
                                    .weak(),
                            );
                        });
                        let prompt = sys.as_ref().map(|s| s.note_prompt.clone()).filter(|p| !p.is_empty());
                        // Read Enter before the text field consumes it; save
                        // after it has taken in this frame's typing.
                        let enter = ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift);
                        let t = ui.add(
                            egui::TextEdit::multiline(&mut n.text)
                                .hint_text(prompt.unwrap_or_else(|| "Why is this worth noting?".into()))
                                .return_key(egui::KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter))
                                .desired_rows(2)
                                .desired_width(f32::INFINITY),
                        );
                        save |= enter;
                        if !n.focus_requested {
                            t.request_focus();
                            n.focus_requested = true;
                        }
                        ui.horizontal(|ui| {
                            save |= ui.button("Save (Enter)").clicked();
                            skip = ui.button("Skip (Esc)").clicked();
                            ui.weak("Shift-Enter: new line");
                        });
                    });
                if skip {
                    self.popup = Popup::None;
                } else if save && let Popup::Note(n) = std::mem::replace(&mut self.popup, Popup::None) {
                    self.commit_note(n);
                }
            }
            Popup::Systems { target } => {
                let target = target.clone();
                let mut close = false;
                let mut toggles = Vec::new();
                egui::Window::new("File into systems")
                    .collapsible(false)
                    .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
                    .show(ctx, |ui| {
                        ui.label(RichText::new(self.ws.graph.title(&target)).strong());
                        let mut systems: Vec<&System> = self.ws.graph.systems.values().collect();
                        systems.sort_by_key(|s| (s.hotkey.unwrap_or(99), s.name.clone()));
                        for s in systems {
                            let mut on = self.ws.graph.in_system(&target, &s.id);
                            let key = s.hotkey.map(|h| format!("{h}  ")).unwrap_or("   ".into());
                            if ui.checkbox(&mut on, format!("{key}{}", s.name)).changed() {
                                toggles.push((s.id.clone(), on));
                            }
                        }
                        ui.weak("digits toggle · Enter/Esc closes");
                        close = ui.button("Done").clicked();
                    });
                for (s, on) in toggles {
                    if let Ok(tx) = commands::set_membership(&self.ws.graph, &target, &s, on, now_ms()) {
                        let _ = self.ws.commit(tx);
                    }
                }
                if close {
                    self.popup = Popup::None;
                }
            }
            Popup::Kinds { target } => {
                let target = target.clone();
                let mut pick = None;
                let palette = commands::kind_palette(&self.ws.graph, self.ws.active_system().as_ref());
                egui::Window::new("Relation kind")
                    .collapsible(false)
                    .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
                    .show(ctx, |ui| {
                        let current = self.ws.graph.relations.get(&target).map(|r| r.kind.clone());
                        for (i, k) in palette.iter().enumerate().take(9) {
                            let sel = current.as_ref() == Some(k);
                            if ui.selectable_label(sel, format!("{}  {}", i + 1, k)).clicked() {
                                pick = Some(k.clone());
                            }
                        }
                    });
                if let Some(k) = pick {
                    if let Ok(tx) = commands::set_kind(&self.ws.graph, &target, &k, now_ms()) {
                        let _ = self.ws.commit(tx);
                    }
                    self.popup = Popup::None;
                }
            }
            Popup::Hints(h) => {
                if let Some(q) = &mut h.search {
                    let mut chosen = None;
                    egui::Window::new("Link to…")
                        .collapsible(false)
                        .anchor(Align2::CENTER_TOP, vec2(0.0, 80.0))
                        .show(ctx, |ui| {
                            let r = ui.add(
                                egui::TextEdit::singleline(q)
                                    .hint_text("search entities")
                                    .desired_width(320.0),
                            );
                            r.request_focus();
                            let hits = self.ws.graph.search(q);
                            for id in hits.iter().filter(|i| !commands::is_highlight_carrier(i)).take(10) {
                                if ui.selectable_label(false, self.ws.graph.title(id)).clicked() {
                                    chosen = Some(id.clone());
                                }
                            }
                            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                                chosen = hits.into_iter().find(|i| !commands::is_highlight_carrier(i));
                            }
                        });
                    if let Some(t) = chosen {
                        let source = h.source.clone();
                        self.popup = Popup::None;
                        self.finish_link(source, t);
                    }
                }
            }
            Popup::ConfirmDelete { target, count } => {
                let (target, count) = (target.clone(), *count);
                let mut decided = None;
                egui::Window::new("Delete?").collapsible(false).anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0)).show(ctx, |ui| {
                    ui.label(format!(
                        "Deleting “{}” removes {} entities (it and every relation attached to it, recursively). Undo restores all of them.",
                        self.ws.graph.title(&target),
                        count
                    ));
                    ui.horizontal(|ui| {
                        if ui.button("Delete (Enter)").clicked() {
                            decided = Some(true);
                        }
                        if ui.button("Cancel (Esc)").clicked() {
                            decided = Some(false);
                        }
                    });
                });
                if let Some(d) = decided {
                    self.popup = Popup::None;
                    if d {
                        self.delete(&target);
                    }
                }
            }
            Popup::Help => {
                let mut open = true;
                egui::Window::new("Keymap")
                    .open(&mut open)
                    .collapsible(false)
                    .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
                    .show(ctx, |ui| {
                        ui.columns(2, |cols| {
                            cols[0].label(RichText::new("Reader").strong());
                            for (k, d) in READER_KEYS {
                                cols[0].label(format!("{k:<14} {d}"));
                            }
                            cols[1].label(RichText::new("Canvas").strong());
                            for (k, d) in CANVAS_KEYS {
                                cols[1].label(format!("{k:<14} {d}"));
                            }
                        });
                    });
                if !open {
                    self.popup = Popup::None;
                }
            }
        }
    }

    /// Draw 2-char hint labels over their targets (in whichever canvas is visible).
    fn paint_hints(&self, ctx: &egui::Context) {
        let Popup::Hints(h) = &self.popup else { return };
        if h.search.is_some() {
            return;
        }
        let view = if self.mode == Mode::Canvas { &self.canvas } else { &self.strip };
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("hints")));
        let mut shown = 0;
        for (label, id) in &h.labels {
            let Some(p) = view.screen.get(id) else { continue };
            if !label.starts_with(&h.typed) {
                continue;
            }
            shown += 1;
            let r = egui::Rect::from_center_size(*p + vec2(0.0, -16.0), vec2(26.0, 18.0));
            painter.rect(
                r,
                4.0,
                Color32::from_rgb(255, 214, 64),
                egui::Stroke::new(1.0, Color32::BLACK),
                egui::StrokeKind::Inside,
            );
            painter.text(
                r.center(),
                Align2::CENTER_CENTER,
                label,
                egui::FontId::monospace(12.0),
                Color32::BLACK,
            );
        }
        let msg = if shown == 0 {
            "No hint targets visible — press / to search".to_owned()
        } else {
            format!("Link “{}” -> type a label, / to search, Esc to cancel", self.ws.graph.title(&h.source))
        };
        let screen = ctx.content_rect();
        painter.text(
            screen.center_top() + vec2(0.0, 48.0),
            Align2::CENTER_CENTER,
            msg,
            egui::FontId::proportional(14.0),
            Color32::from_rgb(255, 214, 64),
        );
    }

    fn paint_toasts(&self, ctx: &egui::Context) {
        let screen = ctx.content_rect();
        let mut y = screen.bottom() - 40.0;
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("toasts")));
        if let Some((_, at)) = &self.colour_offer {
            let left = KIND_OFFER_SECS - at.elapsed().as_secs_f32();
            if left > 0.0 {
                let w = 8.0 * 60.0 + 70.0;
                let r = egui::Rect::from_center_size(egui::pos2(screen.center().x, y), vec2(w, 30.0));
                painter.rect_filled(r, 6.0, Color32::from_rgb(40, 40, 48));
                painter.text(
                    r.left_center() + vec2(10.0, 0.0),
                    Align2::LEFT_CENTER,
                    "colour:",
                    egui::FontId::proportional(13.0),
                    Color32::WHITE,
                );
                for (i, (_, c)) in PALETTE.iter().enumerate() {
                    let cx = r.left() + 80.0 + i as f32 * 60.0;
                    painter.circle_filled(egui::pos2(cx, y), 8.0, to_color32(*c));
                    painter.text(
                        egui::pos2(cx + 13.0, y),
                        Align2::LEFT_CENTER,
                        format!("{}", i + 1),
                        egui::FontId::proportional(13.0),
                        Color32::WHITE,
                    );
                }
                y -= 34.0;
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        if let Some(from) = &self.link_from {
            let text = format!(
                "Click a node or relation to link from “{}” · Esc cancels",
                self.ws.graph.title(from)
            );
            let galley = painter.layout_no_wrap(text, egui::FontId::proportional(13.0), Color32::BLACK);
            let r = egui::Rect::from_center_size(
                egui::pos2(screen.center().x, screen.top() + 60.0),
                galley.size() + vec2(20.0, 10.0),
            );
            painter.rect_filled(r, 6.0, Color32::from_rgb(255, 214, 64));
            painter.galley(r.min + vec2(10.0, 5.0), galley, Color32::BLACK);
        }
        if let Some((_, kinds, at)) = &self.kind_offer {
            let left = KIND_OFFER_SECS - at.elapsed().as_secs_f32();
            if left > 0.0 {
                let text = kinds
                    .iter()
                    .enumerate()
                    .map(|(i, k)| format!("{} {}", i + 1, k))
                    .collect::<Vec<_>>()
                    .join("   ");
                let galley = painter.layout_no_wrap(
                    format!("kind:  {text}"),
                    egui::FontId::proportional(13.0),
                    Color32::WHITE,
                );
                let r = egui::Rect::from_center_size(
                    egui::pos2(screen.center().x, y),
                    galley.size() + vec2(20.0, 10.0),
                );
                painter.rect_filled(r, 6.0, Color32::from_rgb(40, 70, 120));
                painter.galley(r.min + vec2(10.0, 5.0), galley, Color32::WHITE);
                y -= 34.0;
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        for t in self.ws.toasts.iter().rev() {
            let text = if t.undo_hint { format!("{}   ·  Ctrl-Z to undo", t.text) } else { t.text.clone() };
            let galley = painter.layout_no_wrap(text, egui::FontId::proportional(13.0), Color32::WHITE);
            let r = egui::Rect::from_center_size(
                egui::pos2(screen.center().x, y),
                galley.size() + vec2(20.0, 10.0),
            );
            painter.rect_filled(
                r,
                6.0,
                if t.error { Color32::from_rgb(150, 40, 40) } else { Color32::from_black_alpha(215) },
            );
            painter.galley(r.min + vec2(10.0, 5.0), galley, Color32::WHITE);
            y -= 34.0;
        }
        if !self.ws.toasts.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let g = &self.ws.graph;
            let nodes = g.nodes.keys().filter(|n| !commands::is_highlight_carrier(n)).count();
            ui.weak(format!("{nodes} nodes · {} relations · {}", g.relations.len(), self.ws.path.display()));
            ui.separator();
            let hint = match self.mode {
                Mode::Reader => "select, then Ctrl-1..9 capture · h highlight · N compose · L link prev · l link · Tab canvas · F1 keys",
                Mode::Canvas => "click/hjkl focus · e edges · N edit · l link · s systems · k kind · f focus · Enter source · F1 keys",
            };
            ui.weak(hint);
        });
    }
}

const READER_KEYS: &[(&str, &str)] = &[
    ("h", "highlight selection (last color)"),
    ("Ctrl-1…9", "highlight + node in system n"),
    ("N", "node with inline composer"),
    ("L", "link previous node -> new node"),
    ("l", "link to a hint-selected target"),
    ("a", "add selection as anchor to focus"),
    ("s", "system palette for focus"),
    ("Ctrl-drag", "region selection"),
    ("click", "word / figure under cursor"),
    ("/ n N", "search, next, previous"),
    ("g G", "top / bottom"),
    ("Alt-Left/Right", "back / forward"),
    ("Ctrl +/−/0", "zoom in / out / fit width"),
    ("Ctrl-Shift-n", "toggle overlay n"),
    ("Tab", "switch to canvas"),
    ("Ctrl-Enter", "open focus in canvas"),
    ("Ctrl-Z / Shift", "undo / redo"),
];

const CANVAS_KEYS: &[(&str, &str)] = &[
    ("h j + arrows", "move focus along edges"),
    ("e", "cycle focused node's relations"),
    ("Enter", "jump to source anchor"),
    ("N", "edit title/body (nodes & relations)"),
    ("l", "link from focus (hints)"),
    ("L", "link previous -> focus"),
    ("s", "system membership palette"),
    ("k", "change relation kind"),
    ("Ctrl-r", "reverse relation"),
    ("Ctrl-Shift-1…5", "friction (relation focused)"),
    ("Ctrl-Shift-Up/Down", "abstraction level"),
    ("f  [ ]", "focus mode, adjust hops"),
    ("Ctrl-Shift-n", "toggle overlay n"),
    ("G", "ghost mode"),
    ("i / Esc", "isolate system / restore"),
    ("Ctrl-N", "free-standing node"),
    ("z", "fit view"),
    ("Ctrl-Backspace", "delete (cascade preview)"),
    ("drag rim / Shift-drag", "link by mouse"),
];

/// Each system the entity is filed in, with its note (or the system's
/// prompt when there is none), then the general note and the source passage.
fn notes_ui(ui: &mut egui::Ui, g: &Graph, id: &EntityId) {
    let mut systems: Vec<&System> = g.systems_of(id).filter_map(|s| g.systems.get(s)).collect();
    systems.sort_by_key(|s| (s.hotkey.unwrap_or(99), s.name.clone()));
    for s in systems {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().circle_filled(r.center(), 5.0, to_color32(s.color));
            ui.label(RichText::new(&s.name).strong());
        });
        let note = g.membership(id, &s.id).map(|m| m.note.trim().to_owned()).unwrap_or_default();
        if !note.is_empty() {
            ui.label(note);
        } else if !s.inbox && !s.note_prompt.is_empty() {
            ui.weak(format!("{} (N to add)", s.note_prompt));
        }
        ui.add_space(3.0);
    }
    let body = g.body(id).trim();
    if !body.is_empty() {
        ui.label(RichText::new("General note").weak());
        ui.label(body);
        ui.add_space(3.0);
    }
    if let Some(a) = g.anchors_of(id).find(|a| !a.is_detached() && !a.exact.is_empty()) {
        use egui::text::{LayoutJob, TextFormat};
        let color = ui.visuals().weak_text_color();
        let strong = ui.visuals().strong_text_color();
        let font = egui::FontId::proportional(12.5);
        let mut job = LayoutJob::default();
        job.wrap.max_width = ui.available_width();
        let plain = TextFormat { font_id: font.clone(), color, ..Default::default() };
        job.append(&format!("p.{}  …{} ", a.page_index + 1, a.prefix), 0.0, plain.clone());
        job.append(&a.exact, 0.0, TextFormat { font_id: font, color: strong, ..Default::default() });
        job.append(&format!(" {}…", a.suffix), 0.0, plain);
        ui.label(RichText::new("Source").weak());
        ui.label(job);
    }
}

/// Popups that sit on the page need a solid background to stay readable.
fn opaque_frame(ctx: &egui::Context) -> egui::Frame {
    let style = ctx.global_style();
    let fill = style.visuals.window_fill();
    egui::Frame::window(&style).fill(Color32::from_rgb(fill.r(), fill.g(), fill.b()))
}

fn is_digit(k: Key) -> bool {
    digit(k).is_some()
}

fn digit(k: Key) -> Option<u8> {
    Some(match k {
        Key::Num1 => 1,
        Key::Num2 => 2,
        Key::Num3 => 3,
        Key::Num4 => 4,
        Key::Num5 => 5,
        Key::Num6 => 6,
        Key::Num7 => 7,
        Key::Num8 => 8,
        Key::Num9 => 9,
        _ => return None,
    })
}

fn key_char(k: Key) -> Option<char> {
    let name = k.name();
    let mut chars = name.chars();
    let c = chars.next()?;
    (chars.next().is_none() && c.is_ascii_alphabetic()).then(|| c.to_ascii_lowercase())
}

fn hint_labels(n: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(n);
    'outer: for &a in HINT_CHARS {
        for &b in HINT_CHARS {
            if out.len() >= n {
                break 'outer;
            }
            out.push(format!("{}{}", a as char, b as char));
        }
    }
    out
}

impl eframe::App for SomaApp {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if let Some(at) = &mut self.autotest {
            raw.events.append(&mut at.inject);
        }
        // Tab switches reader/canvas. egui would also use it to move keyboard
        // focus onto the next widget, which then silences every shortcut, so
        // take it out of egui's hands unless a text field is being edited.
        if !ctx.text_edit_focused() {
            let intercepted = &mut self.intercepted;
            raw.events.retain(|e| match e {
                egui::Event::Key { key: Key::Tab, pressed, modifiers, .. } => {
                    if *pressed {
                        intercepted.push((Key::Tab, *modifiers));
                    }
                    false
                }
                _ => true,
            });
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.ws.poll() {
            self.canvas.invalidate();
            self.strip.invalidate();
        }
        self.renderer.begin_frame(&ctx);
        // Files dropped on the window open as documents.
        let dropped: Vec<PathBuf> =
            ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        for p in dropped {
            if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
                self.open_pdf(&p);
            }
        }
        self.handle_keys(&ctx);

        egui::Panel::top("top").show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));

        match self.mode {
            Mode::Reader => {
                egui::Panel::left("left")
                    .default_size(230.0)
                    .resizable(true)
                    .show(ui, |ui| self.left_panel(ui));
                if self.strip_open {
                    egui::Panel::right("strip").default_size(300.0).resizable(true).show(ui, |ui| {
                        self.strip_header(ui);
                        let focused = self.ws.focused.clone();
                        let avail = ui.available_height() - 90.0;
                        let actions = ui
                            .allocate_ui(vec2(ui.available_width(), avail.max(100.0)), |ui| {
                                self.strip.ui(
                                    ui,
                                    &self.ws.graph,
                                    &self.strip_overlay.0,
                                    self.ws.revision,
                                    self.strip_overlay.1,
                                    focused.as_ref(),
                                    &self.ws.selected,
                                    true,
                                )
                            })
                            .inner;
                        self.canvas_actions(actions, false);
                        if let Some(f) = &self.ws.focused
                            && self.ws.graph.contains(f)
                        {
                            ui.separator();
                            ui.label(RichText::new(self.ws.graph.title(f)).strong());
                            egui::ScrollArea::vertical().id_salt("strip-notes").show(ui, |ui| {
                                notes_ui(ui, &self.ws.graph, f);
                            });
                        }
                    });
                }
                egui::CentralPanel::no_frame().show(ui, |ui| {
                    if self.tabs.is_empty() {
                        ui.centered_and_justified(|ui| {
                            ui.label("Open a PDF (button above, or drop a file on this window).");
                        });
                        return;
                    }
                    let focused = self.ws.focused.clone();
                    let theme = self.theme;
                    let active = self.active;
                    if let Some(tab) = self.tabs.get_mut(active) {
                        if tab.search.open {
                            ui.horizontal(|ui| {
                                ui.label("Find:");
                                let r = ui.add(
                                    egui::TextEdit::singleline(&mut tab.search.query)
                                        .id(egui::Id::new("doc-search"))
                                        .desired_width(260.0),
                                );
                                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                                    tab.run_search();
                                }
                                ui.label(format!("{} matches", tab.search.matches.len()));
                                if ui.small_button("prev").clicked() {
                                    tab.next_match(-1);
                                }
                                if ui.small_button("next").clicked() {
                                    tab.next_match(1);
                                }
                                if ui.small_button("×").clicked() {
                                    tab.search = Default::default();
                                }
                            });
                        }
                        let out = tab.ui(ui, &mut self.renderer, &self.ws.graph, focused.as_ref(), theme);
                        if let Some(first) = out.clicked_entities.first() {
                            self.focus(Some(first.clone()));
                        }
                        if let Some((aid, at)) = out.context {
                            if let Some(e) = self.ws.graph.anchors.get(&aid).map(|a| a.entity.clone())
                                && !commands::is_highlight_carrier(&e)
                            {
                                self.focus(Some(e));
                            }
                            self.popup = Popup::Menu { target: MenuTarget::Highlight(aid), at };
                        }
                    }
                });
            }
            Mode::Canvas => {
                egui::Panel::left("canvas-left").default_size(240.0).resizable(true).show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| self.canvas_controls(ui));
                });
                egui::Panel::right("inspector").default_size(300.0).resizable(true).show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| self.inspector(ui));
                });
                egui::CentralPanel::no_frame().show(ui, |ui| {
                    let focused = self.ws.focused.clone();
                    let actions = self.canvas.ui(
                        ui,
                        &self.ws.graph,
                        &self.overlay,
                        self.ws.revision,
                        self.overlay_rev,
                        focused.as_ref(),
                        &self.ws.selected,
                        false,
                    );
                    self.canvas_actions(actions, true);
                });
            }
        }
        self.popups(&ctx);
        self.menu_ui(&ctx);
        self.paint_hints(&ctx);
        self.paint_toasts(&ctx);
        self.autotest_frame(&ctx);
    }
}

impl SomaApp {
    /// Strip header: which system it shows. Also keeps the strip's overlay
    /// and focus mode in step with that choice.
    fn strip_header(&mut self, ui: &mut egui::Ui) {
        let current = self.ws.last_system.clone().filter(|s| self.ws.graph.systems.contains_key(s));
        let name = |g: &Graph, s: &SystemId| g.systems.get(s).map(|s| s.name.clone()).unwrap_or_default();
        let label = match &self.strip_mode {
            StripMode::CurrentSystem => match &current {
                Some(s) => format!("Current system: {}", name(&self.ws.graph, s)),
                None => "Current system".into(),
            },
            StripMode::System(s) => name(&self.ws.graph, s),
            StripMode::Neighbourhood => "Neighbourhood of focus".into(),
        };
        let mut mode = self.strip_mode.clone();
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("strip-mode")
                .selected_text(label)
                .width(ui.available_width() - 40.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut mode,
                        StripMode::CurrentSystem,
                        "Current system (follows your captures)",
                    );
                    ui.selectable_value(&mut mode, StripMode::Neighbourhood, "Neighbourhood of focus");
                    ui.separator();
                    let mut systems: Vec<&System> = self.ws.graph.systems.values().collect();
                    systems.sort_by_key(|s| (s.hotkey.unwrap_or(99), s.name.clone()));
                    for s in systems {
                        ui.selectable_value(&mut mode, StripMode::System(s.id.clone()), s.name.as_str());
                    }
                });
            if ui.small_button("Fit").clicked() {
                self.strip.fit_next_frame();
            }
        });
        self.strip_mode = mode;
        let system = match &self.strip_mode {
            StripMode::CurrentSystem => current,
            StripMode::System(s) => Some(s.clone()),
            StripMode::Neighbourhood => None,
        };
        let (overlay, focus) = match system {
            Some(s) => (Overlay { active: vec![s], mode: Combine::Union, ghost: false }, None),
            None => {
                let center = self.ws.focused.clone().or_else(|| self.ws.recent.front().cloned());
                (Overlay::default(), center.map(|c| (c, 2)))
            }
        };
        if overlay != self.strip_overlay.0 || focus != self.strip.focus_mode {
            self.strip_overlay = (overlay, self.strip_overlay.1 + 1);
            self.strip.focus_mode = focus;
            self.strip.invalidate();
            self.strip.fit_next_frame();
        }
    }

    fn menu_ui(&mut self, ctx: &egui::Context) {
        let Popup::Menu { target, at } = &self.popup else { return };
        let (target, at) = (target.clone(), *at);
        let g = &self.ws.graph;
        let (anchor, entity) = match &target {
            MenuTarget::Highlight(a) => match g.anchors.get(a) {
                Some(x) => (Some(x.clone()), Some(x.entity.clone())),
                None => (None, None),
            },
            MenuTarget::Entity(e) => (None, Some(e.clone())),
        };
        let Some(entity) = entity.filter(|e| g.contains(e)) else {
            self.popup = Popup::None;
            return;
        };
        let plain = commands::is_highlight_carrier(&entity);
        let is_node = g.nodes.contains_key(&entity) && !plain;
        let is_rel = g.is_relation(&entity);
        let title = if plain { "Highlight".to_owned() } else { g.title(&entity) };
        let mut systems: Vec<System> = g.systems.values().filter(|s| !s.inbox).cloned().collect();
        systems.sort_by_key(|s| (s.hotkey.unwrap_or(99), s.name.clone()));
        let kinds = if is_rel { commands::kind_palette(g, self.ws.active_system().as_ref()) } else { vec![] };
        let current_kind = g.relations.get(&entity).map(|r| r.kind.clone());
        let has_source = g.anchors_of(&entity).any(|a| !a.is_detached());

        enum Act {
            Colour(Option<Color>),
            Edit,
            Systems,
            Promote(SystemId),
            RemoveHighlight,
            Delete,
            Kind(KindId),
            Reverse,
            LinkFrom,
            Source,
            OpenCanvas,
            FocusMode,
        }
        let mut act: Option<Act> = None;
        let resp = egui::Area::new(egui::Id::new("context-menu"))
            .order(egui::Order::Foreground)
            .fade_in(false)
            .fixed_pos(at)
            .show(ctx, |ui| {
                opaque_frame(ctx).show(ui, |ui| {
                    ui.set_min_width(220.0);
                    ui.label(RichText::new(title.chars().take(48).collect::<String>()).strong());
                    ui.separator();
                    if is_node || plain {
                        ui.horizontal(|ui| {
                            for (name, c) in PALETTE {
                                let (r, resp) =
                                    ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::click());
                                ui.painter().circle_filled(r.center(), 8.0, to_color32(c));
                                if resp.on_hover_text(name).clicked() {
                                    act = Some(Act::Colour(Some(c)));
                                }
                            }
                        });
                        if is_node && ui.small_button("Reset colour to its system").clicked() {
                            act = Some(Act::Colour(None));
                        }
                        ui.separator();
                    }
                    if plain {
                        ui.collapsing("Make it a node in…", |ui| {
                            for s in &systems {
                                if ui.button(&s.name).clicked() {
                                    act = Some(Act::Promote(s.id.clone()));
                                }
                            }
                        });
                        if ui.button("Delete highlight").clicked() {
                            act = Some(Act::RemoveHighlight);
                        }
                        return;
                    }
                    if ui.button("Edit title and notes…   N").clicked() {
                        act = Some(Act::Edit);
                    }
                    if ui.button("File into systems…   s").clicked() {
                        act = Some(Act::Systems);
                    }
                    if is_rel {
                        ui.collapsing("Change kind   k", |ui| {
                            for k in &kinds {
                                if ui.selectable_label(current_kind.as_ref() == Some(k), k.as_str()).clicked()
                                {
                                    act = Some(Act::Kind(k.clone()));
                                }
                            }
                        });
                        if ui.button("Reverse direction   Ctrl-r").clicked() {
                            act = Some(Act::Reverse);
                        }
                    }
                    if ui.button("Link from here…   l").clicked() {
                        act = Some(Act::LinkFrom);
                    }
                    if anchor.is_none() && has_source && ui.button("Jump to source   Enter").clicked() {
                        act = Some(Act::Source);
                    }
                    if anchor.is_some() && ui.button("Show in canvas   Ctrl-Enter").clicked() {
                        act = Some(Act::OpenCanvas);
                    }
                    if anchor.is_none() && ui.button("Focus mode   f").clicked() {
                        act = Some(Act::FocusMode);
                    }
                    ui.separator();
                    if anchor.is_some()
                        && is_node
                        && g.anchors_of(&entity).count() > 1
                        && ui.button("Remove this highlight only").clicked()
                    {
                        act = Some(Act::RemoveHighlight);
                    }
                    let del = if is_rel { "Delete relation…" } else { "Delete node…" };
                    if ui
                        .button(
                            RichText::new(format!("{del}   Ctrl-Backspace"))
                                .color(Color32::from_rgb(230, 90, 80)),
                        )
                        .clicked()
                    {
                        act = Some(Act::Delete);
                    }
                });
            });
        // A press anywhere outside the menu closes it (but not the press
        // that opened it, which lands exactly at `at`).
        let clicked_outside = ctx.input(|i| i.pointer.any_pressed())
            && ctx
                .input(|i| i.pointer.interact_pos())
                .is_some_and(|p| !resp.response.rect.contains(p) && p != at);
        let Some(act) = act else {
            if clicked_outside {
                self.popup = Popup::None;
            }
            return;
        };
        self.popup = Popup::None;
        let now = now_ms();
        match act {
            Act::Colour(c) => match &anchor {
                Some(a) => self.recolour_highlight(&a.id, c),
                None => {
                    if let Ok(tx) = commands::recolor(&self.ws.graph, &entity, c, now) {
                        let _ = self.ws.commit(tx);
                    }
                }
            },
            Act::Edit => self.open_composer(ComposerTarget::Edit(entity), at),
            Act::Systems => self.popup = Popup::Systems { target: entity },
            Act::Promote(s) => {
                if let Some(a) = &anchor
                    && let Ok((tx, id)) = commands::promote_highlight(&self.ws.graph, &a.id, &s, now)
                    && self.ws.commit(tx).is_ok()
                {
                    self.ws.note_created(&id);
                    self.ws.last_system = Some(s.clone());
                    self.open_note(id, s, at);
                }
            }
            Act::RemoveHighlight => {
                if let Some(a) = &anchor
                    && let Ok(tx) = commands::remove_anchor(&self.ws.graph, &a.id)
                {
                    let _ = self.ws.commit(tx);
                }
            }
            Act::Delete => {
                self.focus(Some(entity));
                self.delete_focused();
            }
            Act::Kind(k) => {
                if let Ok(tx) = commands::set_kind(&self.ws.graph, &entity, &k, now) {
                    let _ = self.ws.commit(tx);
                }
            }
            Act::Reverse => {
                if let Ok(tx) = commands::reverse(&self.ws.graph, &entity, now) {
                    let _ = self.ws.commit(tx);
                }
            }
            Act::LinkFrom => self.link_from = Some(entity),
            Act::Source => {
                self.focus(Some(entity));
                self.jump_to_source();
            }
            Act::OpenCanvas => {
                self.focus(Some(entity.clone()));
                self.mode = Mode::Canvas;
                self.canvas.center_on(&entity);
            }
            Act::FocusMode => {
                self.canvas.focus_mode = Some((entity, 2));
                self.canvas.invalidate();
                self.canvas.fit_next_frame();
            }
        }
    }

    fn canvas_actions(&mut self, actions: Vec<CanvasAction>, full: bool) {
        for a in actions {
            match a {
                CanvasAction::Focus(id) => {
                    if let Some(from) = self.link_from.take()
                        && from != id
                    {
                        self.finish_link(from, id.clone());
                    }
                    self.focus(Some(id));
                }
                CanvasAction::Menu(id, at) => {
                    self.focus(Some(id.clone()));
                    self.popup = Popup::Menu { target: MenuTarget::Entity(id), at };
                }
                CanvasAction::ToggleSelect(id) => {
                    if let Some(i) = self.ws.selected.iter().position(|e| *e == id) {
                        self.ws.selected.remove(i);
                    } else {
                        self.ws.selected.push(id);
                        if self.ws.selected.len() > 2 {
                            self.ws.selected.remove(0);
                        }
                    }
                }
                CanvasAction::Link { from, to } => self.finish_link(from, to),
                CanvasAction::LinkToNew { from } => {
                    let at = if full {
                        self.canvas.take_pending_position()
                    } else {
                        self.strip.take_pending_position()
                    };
                    let pos = self.canvas.rect.center();
                    self.open_composer(ComposerTarget::Free { link_from: Some(from), at }, pos);
                }
                CanvasAction::CreateNode => {
                    let at = if full {
                        self.canvas.take_pending_position()
                    } else {
                        self.strip.take_pending_position()
                    };
                    let pos = self.canvas.rect.center();
                    self.open_composer(ComposerTarget::Free { link_from: None, at }, pos);
                }
                CanvasAction::Edit(id) => {
                    let at = self.canvas.screen.get(&id).copied().unwrap_or(self.canvas.rect.center());
                    self.focus(Some(id.clone()));
                    self.open_composer(ComposerTarget::Edit(id), at);
                }
                CanvasAction::ClearFocus => {
                    if full {
                        self.focus(None);
                    }
                }
            }
        }
    }
}
