//! The graph canvas (PRD §5.5): used full-screen and as the reader's graph
//! strip. Relations render in their derived state — a line (bare), a line
//! with a midpoint chip (annotated) or an inline lozenge (promoted) — and
//! relation-on-relation edges terminate on the lozenge (F-GRAPH-3). Ghost
//! endpoints render as labeled stubs (F-GRAPH-4).

use crate::workspace::to_color32;
use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2, pos2, vec2,
};
use soma_core::*;
use soma_layout::{ForceLayout, LayoutGraph};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LayoutMode {
    Force,
    Layered,
    Radial,
    Manual,
}

enum Drag {
    Move(EntityId),
    Link { from: EntityId, to: Pos2 },
    Pan,
}

/// Something the app must act on after the canvas handled input.
pub enum CanvasAction {
    Focus(EntityId),
    ToggleSelect(EntityId),
    Link {
        from: EntityId,
        to: EntityId,
    },
    /// Drag-to-link released on empty space: create a node there and link.
    LinkToNew {
        from: EntityId,
    },
    /// Double-click on empty space.
    CreateNode,
    /// Double-click on an entity.
    Edit(EntityId),
    ClearFocus,
}

pub struct Canvas {
    pub mode: LayoutMode,
    pub force: ForceLayout,
    targets: HashMap<EntityId, soma_layout::Vec2>,
    /// Drawn positions (animated towards the layout).
    pos: HashMap<EntityId, soma_layout::Vec2>,
    pub zoom: f32,
    pub pan: Vec2,
    /// Focus mode: only the k-hop neighbourhood of an entity (F-GRAPH-5).
    pub focus_mode: Option<(EntityId, usize)>,
    pub filter: String,
    pub layered_kind: KindId,
    lg: LayoutGraph,
    vis: HashMap<EntityId, Visibility>,
    built: Option<(u64, u64)>,
    drag: Option<Drag>,
    /// Screen positions from the last frame, for hints and navigation.
    pub screen: HashMap<EntityId, Pos2>,
    pub rect: Rect,
    auto_fit: bool,
    /// Relations in cycles of acyclic kinds (flagged, F-REL-9).
    cycles: HashSet<EntityId>,
    pending_pos: Option<soma_layout::Vec2>,
}

const NODE_FONT: f32 = 13.0;
const GHOST_ALPHA: f32 = 0.12;

impl Canvas {
    pub fn new() -> Self {
        Self {
            mode: LayoutMode::Force,
            force: ForceLayout::new(),
            targets: HashMap::new(),
            pos: HashMap::new(),
            zoom: 1.0,
            pan: Vec2::ZERO,
            focus_mode: None,
            filter: String::new(),
            layered_kind: KindId::from(defaults::PREREQUISITE_OF),
            lg: LayoutGraph::default(),
            vis: HashMap::new(),
            built: None,
            drag: None,
            screen: HashMap::new(),
            rect: Rect::NOTHING,
            auto_fit: true,
            cycles: HashSet::new(),
            pending_pos: None,
        }
    }

    pub fn invalidate(&mut self) {
        self.built = None;
    }

    pub fn fit_next_frame(&mut self) {
        self.auto_fit = true;
    }

    pub fn set_mode(&mut self, mode: LayoutMode) {
        self.mode = mode;
        self.invalidate();
        self.force.reheat(0.5);
        self.fit_next_frame();
    }

    pub fn visibility(&self, id: &EntityId) -> Visibility {
        self.vis.get(id).copied().unwrap_or(Visibility::Hidden)
    }

    /// Recompute visibility (overlay ∩ focus mode ∩ filter) and the layout graph.
    fn rebuild(&mut self, g: &Graph, overlay: &Overlay) {
        let mut vis = overlay.resolve(g);
        if let Some((center, k)) = &self.focus_mode {
            if g.contains(center) {
                let keep = g.k_hop(center, *k);
                for (id, v) in vis.iter_mut() {
                    if !keep.contains(id) && *v != Visibility::Hidden {
                        *v = Visibility::Hidden;
                    }
                }
                // Endpoints of kept relations stay as stubs.
                for r in keep.iter().filter(|r| g.is_relation(r)) {
                    for e in g.relations[r].endpoint_ids() {
                        if vis.get(e) == Some(&Visibility::Hidden) {
                            vis.insert(e.clone(), Visibility::GhostEndpoint);
                        }
                    }
                }
            } else {
                self.focus_mode = None;
            }
        }
        let filter = self.filter.trim();
        if !filter.is_empty() {
            let matching = filter_matches(g, filter);
            for (id, v) in vis.iter_mut() {
                if *v == Visibility::Full && !matching.contains(id) {
                    *v = Visibility::Ghost;
                }
            }
        }
        let layered = matches!(self.mode, LayoutMode::Layered).then_some(&self.layered_kind);
        self.lg = LayoutGraph::build(g, &vis, layered);
        self.cycles = g.relations_in_cycles();
        self.vis = vis;
        match self.mode {
            LayoutMode::Layered => self.targets = soma_layout::layered(&self.lg),
            LayoutMode::Radial => {
                let center =
                    self.focus_mode.as_ref().map(|f| f.0.clone()).or_else(|| self.lg.ids.first().cloned());
                self.targets = center.map(|c| soma_layout::radial(&self.lg, &c)).unwrap_or_default();
            }
            LayoutMode::Force | LayoutMode::Manual => self.targets.clear(),
        }
    }

    fn world_to_screen(&self, p: soma_layout::Vec2) -> Pos2 {
        self.rect.center() + self.pan + vec2(p.x, p.y) * self.zoom
    }

    fn screen_to_world(&self, p: Pos2) -> soma_layout::Vec2 {
        let v = (p - self.rect.center() - self.pan) / self.zoom;
        soma_layout::Vec2::new(v.x, v.y)
    }

    /// Position of any drawable entity: a node's point, or the midpoint of a
    /// relation's endpoints (recursively) — where its chip/lozenge sits.
    fn point(&self, g: &Graph, id: &EntityId, depth: usize) -> Option<Pos2> {
        if let Some(p) = self.pos.get(id) {
            return Some(self.world_to_screen(*p));
        }
        if depth > MAX_RELATION_DEPTH + 1 {
            return None;
        }
        let r = g.relations.get(id)?;
        let pts: Vec<Pos2> = r.endpoint_ids().filter_map(|e| self.point(g, e, depth + 1)).collect();
        if pts.is_empty() {
            return None;
        }
        let sum = pts.iter().fold(Vec2::ZERO, |a, p| a + p.to_vec2());
        Some((sum / pts.len() as f32).to_pos2())
    }

    fn step_layout(&mut self, ctx: &egui::Context) {
        match self.mode {
            LayoutMode::Force => {
                let moving = self.force.step(&self.lg, 4);
                self.pos = self
                    .lg
                    .ids
                    .iter()
                    .filter_map(|id| self.force.positions.get(id).map(|p| (id.clone(), *p)))
                    .collect();
                if moving {
                    ctx.request_repaint();
                }
            }
            LayoutMode::Manual => {
                self.force.sync(&self.lg);
                self.pos = self
                    .lg
                    .ids
                    .iter()
                    .filter_map(|id| self.force.positions.get(id).map(|p| (id.clone(), *p)))
                    .collect();
            }
            LayoutMode::Layered | LayoutMode::Radial => {
                let mut moving = false;
                let mut next = HashMap::new();
                for id in &self.lg.ids {
                    let target = self.targets.get(id).copied().unwrap_or_default();
                    let cur = self.pos.get(id).copied().unwrap_or(target);
                    let d = target - cur;
                    let p = if d.len() < 0.5 { target } else { cur + d * 0.25 };
                    moving |= d.len() >= 0.5;
                    next.insert(id.clone(), p);
                }
                self.pos = next;
                // Keep the force layout seeded from here for a smooth switch back.
                for (id, p) in &self.pos {
                    self.force.positions.insert(id.clone(), *p);
                }
                if moving {
                    ctx.request_repaint();
                }
            }
        }
    }

    fn fit(&mut self) {
        if self.pos.is_empty() {
            return;
        }
        let (mut lo, mut hi) = (pos2(f32::MAX, f32::MAX), pos2(f32::MIN, f32::MIN));
        for p in self.pos.values() {
            lo = lo.min(pos2(p.x, p.y));
            hi = hi.max(pos2(p.x, p.y));
        }
        let size = (hi - lo).max(vec2(1.0, 1.0));
        let avail = self.rect.size() - vec2(120.0, 80.0);
        self.zoom = (avail.x / size.x).min(avail.y / size.y).clamp(0.08, 1.6);
        let center = (lo.to_vec2() + hi.to_vec2()) / 2.0;
        self.pan = -center * self.zoom;
    }

    pub fn center_on(&mut self, id: &EntityId) {
        if let Some(p) = self.pos.get(id) {
            self.pan = -vec2(p.x, p.y) * self.zoom;
        }
    }

    /// Draw and interact. `revision` is the workspace revision; `overlay_rev`
    /// changes whenever the overlay does.
    #[allow(clippy::too_many_arguments)]
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        g: &Graph,
        overlay: &Overlay,
        revision: u64,
        overlay_rev: u64,
        focused: Option<&EntityId>,
        selected: &[EntityId],
        compact: bool,
    ) -> Vec<CanvasAction> {
        let mut actions = Vec::new();
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        self.rect = response.rect;
        let painter = painter.with_clip_rect(self.rect);
        let dark = ui.visuals().dark_mode;
        painter.rect_filled(
            self.rect,
            CornerRadius::ZERO,
            if dark { Color32::from_gray(24) } else { Color32::from_gray(246) },
        );

        if self.built != Some((revision, overlay_rev)) {
            self.rebuild(g, overlay);
            self.built = Some((revision, overlay_rev));
        }
        self.step_layout(ui.ctx());
        if self.auto_fit
            && !self.pos.is_empty()
            && (self.mode != LayoutMode::Force || self.force.temperature < 0.5)
        {
            self.fit();
            if self.mode != LayoutMode::Force || self.force.is_settled() {
                self.auto_fit = false;
            }
        }
        if compact && self.auto_fit {
            self.fit();
        }

        // Zoom and pan.
        if response.hovered() {
            let (scroll, zdelta, pointer) =
                ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos()));
            let factor = if zdelta != 1.0 { zdelta } else { (1.0 + scroll.y * 0.0015).clamp(0.8, 1.25) };
            if factor != 1.0
                && let Some(p) = pointer
            {
                let before = self.screen_to_world(p);
                self.zoom = (self.zoom * factor).clamp(0.05, 4.0);
                let after = self.world_to_screen(before);
                self.pan += p - after;
                self.auto_fit = false;
            }
        }

        let ghost = |c: Color32, v: Visibility| match v {
            Visibility::Ghost => c.gamma_multiply(GHOST_ALPHA),
            Visibility::GhostEndpoint => c.gamma_multiply(0.55),
            _ => c,
        };
        let fg = if dark { Color32::from_gray(225) } else { Color32::from_gray(30) };
        let show_labels = self.zoom > 0.35 || compact;

        // ---- relations (lines first, then chips/lozenges on top)
        self.screen.clear();
        let mut relations: Vec<&Relation> =
            g.relations.values().filter(|r| self.visibility(&r.id) != Visibility::Hidden).collect();
        relations.sort_by_key(|r| g.depth(&r.id));
        let mut markers: Vec<(EntityId, Pos2, RelationState, Color32, Visibility)> = Vec::new();
        for r in &relations {
            let v = self.visibility(&r.id);
            let ends: Vec<Pos2> = r.endpoint_ids().filter_map(|e| self.point(g, e, 0)).collect();
            if ends.len() < 2 {
                continue;
            }
            let kind = g.kinds.get(&r.kind);
            let kc = kind.map(|k| to_color32(k.color)).unwrap_or(Color32::GRAY);
            let is_focus = focused == Some(&r.id);
            let width = if is_focus { 3.0 } else { 1.6 } * self.zoom.clamp(0.6, 1.4);
            let color = ghost(kc, v);
            let (a, b) = (ends[0], ends[1]);
            let stroke = Stroke::new(width, color);
            let cross = g.is_cross_level(&r.id);
            draw_edge(
                &painter,
                a,
                b,
                kind.map(|k| k.stroke).unwrap_or(soma_core::Stroke::Solid),
                stroke,
                cross,
            );
            if kind.is_some_and(|k| k.directed) {
                arrow_head(&painter, a, b, stroke, 16.0 * self.zoom.clamp(0.5, 1.2));
            }
            let mid = pos2((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
            let state = g.relation_state(&r.id).unwrap_or(RelationState::Bare);
            let sys_color = match g.primary_system(&r.id) {
                Some(s) => to_color32(s.color),
                None => kc,
            };
            markers.push((r.id.clone(), mid, state, ghost(sys_color, v), v));
            self.screen.insert(r.id.clone(), mid);
            if self.cycles.contains(&r.id) {
                painter.text(
                    mid + vec2(0.0, -14.0),
                    Align2::CENTER_CENTER,
                    "⚠",
                    FontId::proportional(12.0),
                    Color32::from_rgb(230, 80, 60),
                );
            }
        }
        // ---- nodes
        let mut node_rects: Vec<(EntityId, Rect)> = Vec::new();
        let mut ids: Vec<&EntityId> = self.pos.keys().collect();
        ids.sort();
        for id in ids {
            let Some(n) = g.nodes.get(id) else { continue };
            let v = self.visibility(id);
            if v == Visibility::Hidden {
                continue;
            }
            let p = self.world_to_screen(self.pos[id]);
            if !self.rect.expand(200.0).contains(p) {
                continue;
            }
            let color = to_color32(g.entity_color(id));
            let is_focus = focused == Some(id);
            let rect = if v == Visibility::GhostEndpoint {
                let r = Rect::from_center_size(p, vec2(10.0, 10.0));
                painter.circle(p, 5.0, color.gamma_multiply(0.5), Stroke::new(1.0, fg.gamma_multiply(0.5)));
                if show_labels {
                    painter.text(
                        p + vec2(8.0, 0.0),
                        Align2::LEFT_CENTER,
                        ellipsize(&n.title, 24),
                        FontId::proportional(10.0),
                        fg.gamma_multiply(0.55),
                    );
                }
                r
            } else if !show_labels {
                let r = Rect::from_center_size(p, vec2(8.0, 8.0));
                painter.circle_filled(p, 4.0, ghost(color, v));
                r
            } else {
                let font =
                    FontId::proportional(if compact { 11.5 } else { NODE_FONT } * self.zoom.clamp(0.75, 1.3));
                let text_color = ghost(fg, v);
                let galley = painter.layout_no_wrap(
                    ellipsize(&n.title, if compact { 28 } else { 48 }),
                    font,
                    text_color,
                );
                let r = Rect::from_center_size(p, galley.size() + vec2(16.0, 9.0));
                let fill = ghost(if dark { Color32::from_gray(44) } else { Color32::WHITE }, v);
                painter.rect(
                    r,
                    CornerRadius::same(7),
                    fill,
                    Stroke::new(if is_focus { 2.5 } else { 1.4 }, ghost(color, v)),
                    StrokeKind::Inside,
                );
                painter.rect_filled(
                    Rect::from_min_size(r.min + vec2(3.0, 3.0), vec2(4.0, r.height() - 6.0)),
                    CornerRadius::same(2),
                    ghost(color, v),
                );
                painter.galley(r.min + vec2(11.0, 4.5), galley, text_color);
                if let Some(level) = n.abstraction_level {
                    painter.text(
                        r.right_top() + vec2(-4.0, -2.0),
                        Align2::RIGHT_BOTTOM,
                        format!("{level:+}"),
                        FontId::monospace(9.0),
                        fg.gamma_multiply(0.6),
                    );
                }
                let anchors: Vec<&Anchor> = g.anchors_of(id).collect();
                if !anchors.is_empty() && anchors.iter().all(|a| a.is_detached()) {
                    painter.circle_filled(r.right_top(), 4.0, Color32::from_rgb(220, 60, 60));
                }
                r
            };
            if is_focus {
                painter.rect_stroke(
                    rect.expand(4.0),
                    CornerRadius::same(9),
                    Stroke::new(2.0, Color32::from_rgb(90, 160, 255)),
                    StrokeKind::Outside,
                );
            }
            if selected.contains(id) {
                painter.rect_stroke(
                    rect.expand(7.0),
                    CornerRadius::same(10),
                    Stroke::new(1.5, Color32::from_rgb(255, 200, 60)),
                    StrokeKind::Outside,
                );
            }
            self.screen.insert(id.clone(), p);
            node_rects.push((id.clone(), rect));
        }

        // ---- chips and lozenges sit above nodes so they stay clickable
        for (id, mid, state, color, v) in &markers {
            let is_focus = focused == Some(id);
            let sel = selected.contains(id);
            match state {
                RelationState::Bare => {
                    if is_focus || sel {
                        painter.circle_stroke(*mid, 5.0, Stroke::new(2.0, color.gamma_multiply(1.0)));
                    }
                }
                RelationState::Annotated => {
                    painter.circle_filled(*mid, 5.5, *color);
                    painter.circle_stroke(*mid, 5.5, Stroke::new(1.0, fg.gamma_multiply(0.6)));
                }
                RelationState::Promoted => {
                    let s = 9.0 * self.zoom.clamp(0.7, 1.3);
                    let pts = vec![
                        *mid + vec2(0.0, -s),
                        *mid + vec2(s * 1.3, 0.0),
                        *mid + vec2(0.0, s),
                        *mid + vec2(-s * 1.3, 0.0),
                    ];
                    painter.add(Shape::convex_polygon(pts, *color, Stroke::new(1.2, fg.gamma_multiply(0.7))));
                    if show_labels
                        && *v == Visibility::Full
                        && let Some(t) =
                            g.relations.get(id).and_then(|r| r.title.clone()).filter(|t| !t.is_empty())
                    {
                        painter.text(
                            *mid + vec2(s * 1.5, 0.0),
                            Align2::LEFT_CENTER,
                            ellipsize(&t, 36),
                            FontId::proportional(11.0),
                            fg.gamma_multiply(0.85),
                        );
                    }
                }
            }
            if is_focus {
                painter.circle_stroke(*mid, 13.0, Stroke::new(2.0, Color32::from_rgb(90, 160, 255)));
            }
            if sel {
                painter.circle_stroke(*mid, 16.0, Stroke::new(1.5, Color32::from_rgb(255, 200, 60)));
            }
        }

        // ---- interaction
        let pointer = response.interact_pointer_pos().or(response.hover_pos());
        let hit = |p: Pos2| -> Option<(EntityId, bool)> {
            // (entity, on_rim)
            for (id, r) in node_rects.iter().rev() {
                if r.expand(6.0).contains(p) {
                    let rim = !r.shrink(4.0).contains(p);
                    return Some((id.clone(), rim));
                }
            }
            markers
                .iter()
                .filter(|(_, m, _, _, _)| m.distance(p) < 11.0)
                .min_by(|a, b| a.1.distance(p).total_cmp(&b.1.distance(p)))
                .map(|(id, _, _, _, _)| (id.clone(), false))
        };
        let (cmd, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
        if response.drag_started()
            && let Some(p) = response.interact_pointer_pos()
        {
            self.drag = Some(match hit(p) {
                // Drag from a node's rim (or with Shift) starts a link.
                Some((id, rim)) if rim || shift => Drag::Link { from: id, to: p },
                Some((id, _)) if g.nodes.contains_key(&id) => Drag::Move(id),
                Some((id, _)) => Drag::Link { from: id, to: p },
                None => Drag::Pan,
            });
        }
        if response.dragged() {
            let delta = response.drag_delta();
            match &mut self.drag {
                Some(Drag::Pan) => {
                    self.pan += delta;
                    self.auto_fit = false;
                }
                Some(Drag::Move(id)) => {
                    let id = id.clone();
                    if let Some(p) = self.force.positions.get_mut(&id) {
                        p.x += delta.x / self.zoom;
                        p.y += delta.y / self.zoom;
                        let np = *p;
                        self.pos.insert(id.clone(), np);
                        self.targets.insert(id.clone(), np);
                    }
                    self.force.pinned.insert(id);
                    self.force.reheat(0.3);
                }
                Some(Drag::Link { from, to }) => {
                    if let Some(p) = response.interact_pointer_pos() {
                        *to = p;
                    }
                    let (from, to) = (from.clone(), *to);
                    if let Some(a) = self.point(g, &from, 0) {
                        painter.line_segment([a, to], Stroke::new(2.0, Color32::from_rgb(90, 160, 255)));
                        painter.circle_filled(to, 4.0, Color32::from_rgb(90, 160, 255));
                    }
                }
                None => {}
            }
        }
        if response.drag_stopped() {
            if let Some(Drag::Link { from, to }) = self.drag.take() {
                match hit(to) {
                    Some((target, _)) if target != from => {
                        actions.push(CanvasAction::Link { from, to: target })
                    }
                    Some(_) => {}
                    None => {
                        let w = self.screen_to_world(to);
                        self.pending_pos = Some(w);
                        actions.push(CanvasAction::LinkToNew { from });
                    }
                }
            }
            self.drag = None;
        }
        if response.double_clicked() {
            match pointer.and_then(hit) {
                Some((id, _)) => actions.push(CanvasAction::Edit(id)),
                None => {
                    if let Some(p) = pointer {
                        let w = self.screen_to_world(p);
                        self.pending_pos = Some(w);
                    }
                    actions.push(CanvasAction::CreateNode);
                }
            }
        } else if response.clicked() {
            match pointer.and_then(hit) {
                Some((id, _)) if cmd => actions.push(CanvasAction::ToggleSelect(id)),
                Some((id, _)) => actions.push(CanvasAction::Focus(id)),
                None => actions.push(CanvasAction::ClearFocus),
            }
        }
        if let Some(p) = response.hover_pos()
            && let Some((id, _)) = hit(p)
        {
            let body = g.body(&id).to_owned();
            let title = g.title(&id);
            if !body.is_empty() || g.is_relation(&id) {
                let tip_pos = p + vec2(14.0, 14.0);
                let text =
                    if body.is_empty() { title } else { format!("{title}\n\n{}", ellipsize(&body, 280)) };
                let galley = painter.layout(text, FontId::proportional(12.0), fg, 320.0);
                let r = Rect::from_min_size(tip_pos, galley.size() + vec2(14.0, 10.0));
                painter.rect(
                    r,
                    CornerRadius::same(5),
                    if dark { Color32::from_gray(50) } else { Color32::from_gray(255) },
                    Stroke::new(1.0, Color32::from_gray(120)),
                    StrokeKind::Inside,
                );
                painter.galley(r.min + vec2(7.0, 5.0), galley, fg);
            }
        }
        if self.mode == LayoutMode::Force && !self.force.is_settled() {
            ui.ctx().request_repaint();
        }
        actions
    }

    /// Take (and clear) the world position of a pending create-at-cursor.
    pub fn take_pending_position(&mut self) -> Option<soma_layout::Vec2> {
        self.pending_pos.take()
    }

    /// Place a freshly created entity at a world position and pin it briefly.
    pub fn place(&mut self, id: &EntityId, p: soma_layout::Vec2) {
        self.force.positions.insert(id.clone(), p);
        self.pos.insert(id.clone(), p);
    }

    /// The visible entity nearest to `from` in screen direction `dir`
    /// (hjkl navigation). Prefers graph neighbours.
    pub fn neighbor_in_direction(&self, g: &Graph, from: &EntityId, dir: Vec2) -> Option<EntityId> {
        let origin = *self.screen.get(from)?;
        let neighbours = g.neighbors(from);
        let score = |id: &EntityId, p: Pos2| -> Option<f32> {
            let d = p - origin;
            let len = d.length();
            if len < 1.0 {
                return None;
            }
            let cos = d.dot(dir) / len;
            if cos < 0.35 {
                return None;
            }
            let bonus = if neighbours.contains(id) { 0.5 } else { 1.0 };
            Some(len * (2.0 - cos) * bonus)
        };
        self.screen
            .iter()
            .filter(|(id, _)| *id != from && self.visibility(id) != Visibility::Hidden)
            .filter_map(|(id, p)| score(id, *p).map(|s| (s, id)))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id.clone())
    }
}

fn ellipsize(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n {
        s.to_owned()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

fn draw_edge(
    painter: &egui::Painter,
    a: Pos2,
    b: Pos2,
    style: soma_core::Stroke,
    stroke: Stroke,
    cross_level: bool,
) {
    let pts = [a, b];
    match style {
        soma_core::Stroke::Solid => {
            painter.line_segment(pts, stroke);
        }
        soma_core::Stroke::Dashed => {
            painter.extend(Shape::dashed_line(&pts, stroke, 8.0, 5.0));
        }
        soma_core::Stroke::Dotted => {
            painter.extend(Shape::dotted_line(&pts, stroke.color, 7.0, stroke.width * 0.8));
        }
        soma_core::Stroke::Double => {
            let n = (b - a).normalized().rot90() * 2.2;
            painter.line_segment([a + n, b + n], Stroke::new(stroke.width * 0.7, stroke.color));
            painter.line_segment([a - n, b - n], Stroke::new(stroke.width * 0.7, stroke.color));
        }
        soma_core::Stroke::Wavy => {
            let d = b - a;
            let len = d.length().max(1.0);
            let n = d.normalized().rot90();
            let steps = (len / 4.0) as usize;
            let wave: Vec<Pos2> = (0..=steps)
                .map(|i| {
                    let t = i as f32 / steps.max(1) as f32;
                    a + d * t + n * (t * len / 7.0).sin() * 3.0
                })
                .collect();
            painter.add(Shape::line(wave, stroke));
        }
    }
    if cross_level {
        // Cross-level relations: a contrasting dash laid through the stroke.
        let c = Color32::from_rgba_unmultiplied(255, 255, 255, (stroke.color.a() as f32 * 0.8) as u8);
        painter.extend(Shape::dashed_line(&pts, Stroke::new(stroke.width * 0.6, c), 4.0, 10.0));
    }
}

fn arrow_head(painter: &egui::Painter, a: Pos2, b: Pos2, stroke: Stroke, inset: f32) {
    let d = b - a;
    if d.length() < inset * 2.0 {
        return;
    }
    let dir = d.normalized();
    let tip = pos2(a.x + d.x / 2.0, a.y + d.y / 2.0) + dir * (d.length() * 0.18).min(40.0);
    let n = dir.rot90();
    let s = 5.0 + stroke.width;
    painter.add(Shape::convex_polygon(
        vec![tip, tip - dir * s * 1.6 + n * s, tip - dir * s * 1.6 - n * s],
        stroke.color,
        Stroke::NONE,
    ));
}

/// F-GRAPH-7 filter syntax: plain words match title/body; `system:`,
/// `kind:`, `level:`, `doc:` narrow; `detached`, `annotated`, `promoted`
/// select by state.
pub fn filter_matches(g: &Graph, filter: &str) -> HashSet<EntityId> {
    let mut words = Vec::new();
    type Pred<'a> = Box<dyn Fn(&EntityId) -> bool + 'a>;
    let mut preds: Vec<Pred> = Vec::new();
    for tok in filter.split_whitespace() {
        let tl = tok.to_lowercase();
        if let Some(v) = tl.strip_prefix("system:") {
            let v = v.to_owned();
            preds.push(Box::new(move |id| {
                g.systems_of(id).filter_map(|s| g.systems.get(s)).any(|s| s.name.to_lowercase().contains(&v))
            }));
        } else if let Some(v) = tl.strip_prefix("kind:") {
            let v = v.to_owned();
            preds.push(Box::new(move |id| g.relations.get(id).is_some_and(|r| r.kind.as_str().contains(&v))));
        } else if let Some(v) = tl.strip_prefix("level:") {
            let lv: Option<i8> = v.parse().ok();
            preds.push(Box::new(move |id| g.nodes.get(id).is_some_and(|n| n.abstraction_level == lv)));
        } else if let Some(v) = tl.strip_prefix("doc:") {
            let v = v.to_owned();
            preds.push(Box::new(move |id| {
                g.anchors_of(id).any(|a| {
                    g.documents.get(&a.document).is_some_and(|d| {
                        d.title.as_deref().unwrap_or("").to_lowercase().contains(&v)
                            || d.path.to_lowercase().contains(&v)
                    })
                })
            }));
        } else if tl == "detached" {
            preds.push(Box::new(move |id| {
                let a: Vec<_> = g.anchors_of(id).collect();
                !a.is_empty() && a.iter().all(|a| a.is_detached())
            }));
        } else if tl == "annotated" {
            preds.push(Box::new(move |id| g.relation_state(id) == Some(RelationState::Annotated)));
        } else if tl == "promoted" {
            preds.push(Box::new(move |id| g.relation_state(id) == Some(RelationState::Promoted)));
        } else {
            words.push(tl);
        }
    }
    g.entity_ids()
        .filter(|id| {
            let text = format!("{} {}", g.title(id), g.body(id)).to_lowercase();
            words.iter().all(|w| text.contains(w.as_str())) && preds.iter().all(|p| p(id))
        })
        .cloned()
        .collect()
}
