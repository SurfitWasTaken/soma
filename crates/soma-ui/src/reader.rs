//! The PDF reader surface (PRD §5.1): continuous scroll, spreads, zoom and
//! fit modes, tiled rendering, text / word / line / region / object
//! selection, highlights, search, links, back/forward and the gutter.

use crate::render::{PaperTheme, Renderer, TILE, TileKey, scale_step, step_scale};
use crate::workspace::to_color32;
use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};
use soma_core::{AnchorKind, Document, EntityId, Graph, Quad, commands};
use soma_pdf::text::{PageText, Rect as PRect};
use soma_pdf::{Link, LinkTarget, OutlineItem, PageObject, PdfBackend, PdfiumDoc};
use std::collections::HashMap;
use std::ops::Range;
use std::time::Instant;

const PAGE_GAP: f32 = 14.0;
const MARGIN: f32 = 20.0;
pub const MIN_ZOOM: f32 = 0.25;
pub const MAX_ZOOM: f32 = 8.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fit {
    Width,
    Page,
    Manual,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Selection {
    Text { page: u32, range: Range<usize> },
    Region { page: u32, rect: PRect },
    Object { page: u32, rect: PRect },
}

impl Selection {
    pub fn page(&self) -> u32 {
        match self {
            Selection::Text { page, .. }
            | Selection::Region { page, .. }
            | Selection::Object { page, .. } => *page,
        }
    }
}

#[derive(Default)]
pub struct Search {
    pub open: bool,
    pub query: String,
    pub matches: Vec<(u32, Range<usize>)>,
    pub current: usize,
    pub searched: String,
}

enum Drag {
    Text { page: u32, start: usize },
    Region { page: u32, start: (f32, f32), end: (f32, f32) },
}

/// What happened in the reader this frame that the app must act on.
#[derive(Default)]
pub struct ReaderOutput {
    pub clicked_entities: Vec<EntityId>,
}

pub struct DocTab {
    pub doc: Document,
    pub render_idx: usize,
    pdf: PdfiumDoc,
    pub sizes: Vec<(f32, f32)>,
    texts: HashMap<u32, PageText>,
    objects: HashMap<u32, Vec<PageObject>>,
    links: HashMap<u32, Vec<Link>>,
    pub outline: Vec<OutlineItem>,
    pub zoom: f32,
    pub fit: Fit,
    pub spread: bool,
    /// Scroll offset of the viewport's top-left in layout space.
    pub scroll: Vec2,
    pub selection: Option<Selection>,
    drag: Option<Drag>,
    pub search: Search,
    back: Vec<(u32, f32)>,
    forward: Vec<(u32, f32)>,
    pub flash: Option<(u32, Vec<Quad>, Instant)>,
    pending_goto: Option<(u32, f32)>,
    pub view: Rect,
}

struct Slot {
    page: u32,
    /// Top-left in layout space (egui points at current zoom).
    pos: Vec2,
    size: Vec2,
}

impl DocTab {
    pub fn open(doc: Document, renderer: &Renderer) -> anyhow::Result<Self> {
        let pdf = PdfiumDoc::open(std::path::Path::new(&doc.path))?;
        let sizes = (0..pdf.page_count()).map(|p| pdf.page_size(p).unwrap_or((612.0, 792.0))).collect();
        let outline = pdf.outline();
        let render_idx = renderer.add_doc(doc.path.clone().into());
        Ok(Self {
            doc,
            render_idx,
            pdf,
            sizes,
            texts: HashMap::new(),
            objects: HashMap::new(),
            links: HashMap::new(),
            outline,
            zoom: 1.0,
            fit: Fit::Width,
            spread: false,
            scroll: Vec2::ZERO,
            selection: None,
            drag: None,
            search: Search::default(),
            back: Vec::new(),
            forward: Vec::new(),
            flash: None,
            pending_goto: None,
            view: Rect::NOTHING,
        })
    }

    pub fn page_count(&self) -> u32 {
        self.sizes.len() as u32
    }

    pub fn text(&mut self, page: u32) -> Option<&PageText> {
        if !self.texts.contains_key(&page) {
            let t = self.pdf.page_text(page).ok()?;
            self.texts.insert(page, t);
        }
        self.texts.get(&page)
    }

    pub fn text_cloned(&mut self, page: u32) -> Option<PageText> {
        self.text(page).cloned()
    }

    fn objects(&mut self, page: u32) -> &[PageObject] {
        let pdf = &self.pdf;
        self.objects.entry(page).or_insert_with(|| pdf.objects(page).unwrap_or_default())
    }

    fn links(&mut self, page: u32) -> &[Link] {
        let pdf = &self.pdf;
        self.links.entry(page).or_insert_with(|| pdf.links(page).unwrap_or_default())
    }

    /// Text of the current selection (whitespace-normalized).
    pub fn selection_text(&mut self) -> Option<String> {
        match self.selection.clone()? {
            Selection::Text { page, range } => {
                Some(soma_pdf::text::normalize_ws(&self.text(page)?.slice(range)))
            }
            Selection::Region { page, rect } | Selection::Object { page, rect } => {
                let t = self.text(page)?;
                let r = t.range_in_rect(&rect)?;
                Some(soma_pdf::text::normalize_ws(&t.slice(r)))
            }
        }
    }

    /// Build an anchor for the current selection (id/entity filled later).
    pub fn selection_anchor(&mut self, color: Option<soma_core::Color>) -> Option<soma_core::Anchor> {
        let doc = self.doc.id.clone();
        match self.selection.clone()? {
            Selection::Text { page, range } => {
                let t = self.text(page)?;
                Some(soma_pdf::anchor::capture_text(&doc, page, t, range, color))
            }
            Selection::Region { page, rect } => {
                let text = self.selection_text().unwrap_or_default();
                Some(soma_pdf::anchor::capture_region(&doc, page, AnchorKind::Region, rect, text, color))
            }
            Selection::Object { page, rect } => {
                let text = self.selection_text().unwrap_or_default();
                Some(soma_pdf::anchor::capture_region(&doc, page, AnchorKind::Object, rect, text, color))
            }
        }
    }

    // --------------------------------------------------------------- layout

    fn slots(&self) -> Vec<Slot> {
        let mut out = Vec::with_capacity(self.sizes.len());
        let max_w = self.content_width_pt() * self.zoom;
        let mut y = MARGIN;
        let mut i = 0;
        while i < self.sizes.len() {
            let row: Vec<usize> =
                if self.spread && i > 0 && i + 1 < self.sizes.len() { vec![i, i + 1] } else { vec![i] };
            let row_w: f32 = row.iter().map(|&p| self.sizes[p].0 * self.zoom).sum::<f32>()
                + PAGE_GAP * (row.len() - 1) as f32;
            let row_h = row.iter().map(|&p| self.sizes[p].1 * self.zoom).fold(0.0, f32::max);
            let mut x = MARGIN + (max_w - row_w) / 2.0;
            for &p in &row {
                let size = vec2(self.sizes[p].0 * self.zoom, self.sizes[p].1 * self.zoom);
                out.push(Slot { page: p as u32, pos: vec2(x, y), size });
                x += size.x + PAGE_GAP;
            }
            y += row_h + PAGE_GAP;
            i += row.len();
        }
        out
    }

    fn content_width_pt(&self) -> f32 {
        let single = self.sizes.iter().map(|s| s.0).fold(0.0, f32::max);
        if self.spread { single * 2.0 + PAGE_GAP / self.zoom.max(0.01) } else { single }
    }

    fn content_size(&self) -> Vec2 {
        let slots = self.slots();
        let h = slots.last().map(|s| s.pos.y + s.size.y).unwrap_or(0.0) + MARGIN;
        vec2(self.content_width_pt() * self.zoom + 2.0 * MARGIN, h)
    }

    fn slot_of(&self, page: u32) -> Option<Slot> {
        self.slots().into_iter().find(|s| s.page == page)
    }

    fn apply_fit(&mut self, view: Rect) {
        let (w, h) = (view.width() - 2.0 * MARGIN - 12.0, view.height() - 2.0 * MARGIN);
        let max_w = self.content_width_pt();
        let max_h = self.sizes.iter().map(|s| s.1).fold(0.0, f32::max);
        match self.fit {
            Fit::Width if max_w > 0.0 => self.zoom = (w / max_w).clamp(MIN_ZOOM, MAX_ZOOM),
            Fit::Page if max_w > 0.0 && max_h > 0.0 => {
                self.zoom = (w / max_w).min(h / max_h).clamp(MIN_ZOOM, MAX_ZOOM)
            }
            _ => {}
        }
    }

    /// Zoom keeping the layout point under `focus` (screen) fixed.
    pub fn set_zoom(&mut self, zoom: f32, focus: Option<Pos2>) {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let focus = focus.unwrap_or(self.view.center());
        let anchor = self.screen_to_page(focus);
        self.fit = Fit::Manual;
        self.zoom = zoom;
        if let Some((page, x, y)) = anchor
            && let Some(slot) = self.slot_of(page)
        {
            let layout = slot.pos + vec2(x, y) * self.zoom;
            self.scroll = layout - (focus - self.view.min);
        }
    }

    pub fn current_page(&self) -> u32 {
        let probe = self.scroll.y + self.view.height() * 0.3;
        self.slots().iter().rev().find(|s| s.pos.y <= probe).map(|s| s.page).unwrap_or(0)
    }

    fn position(&self) -> (u32, f32) {
        let page = self.current_page();
        let y = self.slot_of(page).map(|s| (self.scroll.y - s.pos.y) / self.zoom).unwrap_or(0.0);
        (page, y)
    }

    /// Scroll so that page-space y on `page` sits near the top.
    pub fn goto(&mut self, page: u32, y_pt: f32, remember: bool) {
        if remember {
            self.back.push(self.position());
            self.forward.clear();
        }
        self.pending_goto = Some((page.min(self.page_count().saturating_sub(1)), y_pt));
    }

    pub fn go_back(&mut self) {
        if let Some(p) = self.back.pop() {
            self.forward.push(self.position());
            self.pending_goto = Some(p);
        }
    }

    pub fn go_forward(&mut self) {
        if let Some(p) = self.forward.pop() {
            self.back.push(self.position());
            self.pending_goto = Some(p);
        }
    }

    pub fn scroll_by_pages(&mut self, pages: f32) {
        self.scroll.y += pages * self.view.height() * 0.9;
    }

    pub fn goto_top(&mut self) {
        self.goto(0, 0.0, true);
    }

    pub fn goto_bottom(&mut self) {
        let last = self.page_count().saturating_sub(1);
        self.goto(last, self.sizes.get(last as usize).map(|s| s.1).unwrap_or(0.0), true);
    }

    /// Jump to an anchor and flash it (F-NODE-3).
    pub fn reveal(&mut self, page: u32, quads: Vec<Quad>) {
        let y = quads.first().map(|q| q.bounds()[1]).unwrap_or(0.0);
        self.goto(page, (y - 80.0).max(0.0), true);
        self.flash = Some((page, quads, Instant::now()));
    }

    /// Screen position of a page-space point (for tests and popups).
    pub fn page_point_to_screen(&self, page: u32, x: f32, y: f32) -> Option<Pos2> {
        self.slot_of(page).map(|s| self.page_to_screen(&s, x, y))
    }

    fn screen_to_page(&self, p: Pos2) -> Option<(u32, f32, f32)> {
        let layout = (p - self.view.min) + self.scroll;
        self.slots().into_iter().find_map(|s| {
            let r = Rect::from_min_size(s.pos.to_pos2(), s.size).expand(8.0);
            r.contains(layout.to_pos2()).then(|| {
                let local = (layout - s.pos) / self.zoom;
                (s.page, local.x, local.y)
            })
        })
    }

    fn page_to_screen(&self, slot: &Slot, x: f32, y: f32) -> Pos2 {
        self.view.min + slot.pos + vec2(x, y) * self.zoom - self.scroll
    }

    fn page_rect_to_screen(&self, slot: &Slot, r: [f32; 4]) -> Rect {
        Rect::from_min_max(self.page_to_screen(slot, r[0], r[1]), self.page_to_screen(slot, r[2], r[3]))
    }

    // --------------------------------------------------------------- search

    pub fn run_search(&mut self) {
        let q = self.search.query.trim().to_owned();
        if q == self.search.searched {
            return;
        }
        let n = self.page_count();
        let mut texts = |p: u32| self.pdf.page_text(p).ok();
        self.search.matches = soma_pdf::search(&mut texts, n, &q);
        self.search.searched = q;
        let here = self.current_page();
        self.search.current = self.search.matches.iter().position(|(p, _)| *p >= here).unwrap_or(0);
        self.show_current_match();
    }

    pub fn next_match(&mut self, dir: i32) {
        let n = self.search.matches.len();
        if n == 0 {
            return;
        }
        self.search.current = (self.search.current as i32 + dir).rem_euclid(n as i32) as usize;
        self.show_current_match();
    }

    fn show_current_match(&mut self) {
        if let Some((page, range)) = self.search.matches.get(self.search.current).cloned()
            && let Some(t) = self.text(page)
        {
            let quads = t.quads(range);
            let y = quads.first().map(|q| q.bounds()[1]).unwrap_or(0.0);
            self.goto(page, (y - 120.0).max(0.0), false);
        }
    }

    // ----------------------------------------------------------------- draw

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        renderer: &mut Renderer,
        graph: &Graph,
        focused: Option<&EntityId>,
        theme: PaperTheme,
    ) -> ReaderOutput {
        let mut out = ReaderOutput::default();
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let view = response.rect;
        let first_frame = self.view == Rect::NOTHING;
        if self.view.size() != view.size() || first_frame {
            self.view = view;
            self.apply_fit(view);
        }
        self.view = view;
        let bg = if theme == PaperTheme::Dark { Color32::from_gray(20) } else { Color32::from_gray(58) };
        painter.rect_filled(view, CornerRadius::ZERO, bg);

        // Input: scroll and zoom.
        if response.hovered() || response.dragged() {
            let (scroll, zoom_delta, cmd, pointer) = ui.input(|i| {
                (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers.command, i.pointer.hover_pos())
            });
            if zoom_delta != 1.0 {
                self.set_zoom(self.zoom * zoom_delta, pointer);
            } else if cmd && scroll.y != 0.0 {
                self.set_zoom(self.zoom * (1.0 + scroll.y * 0.002), pointer);
            } else {
                self.scroll -= scroll;
            }
        }
        if let Some((page, y)) = self.pending_goto.take()
            && let Some(slot) = self.slot_of(page)
        {
            self.scroll.y = slot.pos.y + y * self.zoom - MARGIN;
        }
        let content = self.content_size();
        self.scroll.y = self.scroll.y.clamp(0.0, (content.y - view.height()).max(0.0));
        self.scroll.x = if content.x <= view.width() {
            -(view.width() - content.x) / 2.0
        } else {
            self.scroll.x.clamp(0.0, content.x - view.width())
        };

        self.handle_pointer(ui, &response, graph, &mut out);

        // Pages.
        let ppp = ui.ctx().pixels_per_point();
        let step = scale_step(self.zoom * ppp);
        let scale = step_scale(step);
        let visible: Vec<Slot> = self
            .slots()
            .into_iter()
            .filter(|s| {
                let r = Rect::from_min_size(view.min + s.pos - self.scroll, s.size);
                r.intersects(view.expand(view.height()))
            })
            .collect();
        let mut wanted = Vec::new();
        let painter = painter.with_clip_rect(view);
        for slot in &visible {
            let page_rect = Rect::from_min_size(view.min + slot.pos - self.scroll, slot.size);
            let on_screen = page_rect.intersects(view);
            let paper = match theme {
                PaperTheme::Light => Color32::WHITE,
                PaperTheme::Sepia => Color32::from_rgb(250, 236, 205),
                PaperTheme::Dark => Color32::from_gray(30),
            };
            painter.rect_filled(page_rect, CornerRadius::ZERO, paper);
            let (pw, ph) = self.sizes[slot.page as usize];
            // Low-res preview underneath.
            let preview_step = scale_step(700.0 / pw.max(ph)).min(step);
            let preview =
                TileKey { doc: self.render_idx, page: slot.page, step: preview_step, tx: 0, ty: 0, theme };
            let preview_scale = step_scale(preview_step);
            if let Some(tex) = renderer.get(&preview) {
                let size = tex.size_vec2();
                let r = Rect::from_min_size(page_rect.min, size / preview_scale * self.zoom);
                painter.image(
                    tex.id(),
                    r,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            } else {
                wanted.push((on_screen, 0, preview));
            }
            if preview_step == step && pw * scale <= TILE as f32 && ph * scale <= TILE as f32 {
                continue;
            }
            // Full-resolution tiles intersecting the viewport.
            let tiles_x = ((pw * scale) / TILE as f32).ceil() as i32;
            let tiles_y = ((ph * scale) / TILE as f32).ceil() as i32;
            let tile_pts = TILE as f32 / scale * self.zoom;
            for ty in 0..tiles_y {
                for tx in 0..tiles_x {
                    let r = Rect::from_min_size(
                        page_rect.min + vec2(tx as f32, ty as f32) * tile_pts,
                        Vec2::splat(tile_pts),
                    );
                    if !r.intersects(view.expand(200.0)) {
                        continue;
                    }
                    let key = TileKey { doc: self.render_idx, page: slot.page, step, tx, ty, theme };
                    if let Some(tex) = renderer.get(&key) {
                        let size = tex.size_vec2();
                        let r = Rect::from_min_size(r.min, size / scale * self.zoom);
                        painter.image(
                            tex.id(),
                            r,
                            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    } else {
                        wanted.push((r.intersects(view), 1, key));
                    }
                }
            }
            painter.rect_stroke(
                page_rect,
                CornerRadius::ZERO,
                Stroke::new(1.0, Color32::from_black_alpha(60)),
                StrokeKind::Outside,
            );
        }
        // Prefetch previews ±2 pages around the visible range (F-READ-2).
        if let (Some(first), Some(last)) = (visible.first(), visible.last()) {
            let lo = first.page.saturating_sub(2);
            let hi = (last.page + 2).min(self.page_count().saturating_sub(1));
            for p in lo..=hi {
                let (pw, ph) = self.sizes[p as usize];
                let preview_step = scale_step(700.0 / pw.max(ph)).min(step);
                wanted.push((
                    false,
                    2,
                    TileKey { doc: self.render_idx, page: p, step: preview_step, tx: 0, ty: 0, theme },
                ));
            }
        }
        wanted.sort_by_key(|(on, pri, _)| (!on, *pri));
        let mut seen = std::collections::HashSet::new();
        renderer.want(wanted.into_iter().map(|(_, _, k)| k).filter(|k| seen.insert(k.clone())).collect());

        // Overlays: highlights, search hits, selection, flash.
        for slot in &visible {
            self.paint_marks(&painter, slot, graph, focused);
        }
        self.paint_gutter(ui, &painter, graph);
        out
    }

    fn paint_marks(
        &mut self,
        painter: &egui::Painter,
        slot: &Slot,
        graph: &Graph,
        focused: Option<&EntityId>,
    ) {
        let page = slot.page;
        for a in graph.anchors_on_page(&self.doc.id, page) {
            if a.is_detached() {
                continue;
            }
            let color = a.color.unwrap_or_else(|| graph.entity_color(&a.entity));
            let c = to_color32(color);
            let is_focus = focused == Some(&a.entity);
            let fill = Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), if is_focus { 120 } else { 80 });
            for q in &a.quads {
                let r = self.page_rect_to_screen(slot, q.bounds());
                if a.kind == AnchorKind::Text {
                    painter.rect_filled(r, CornerRadius::same(2), fill);
                } else {
                    painter.rect_stroke(
                        r.expand(2.0),
                        CornerRadius::same(3),
                        Stroke::new(2.0, c),
                        StrokeKind::Outside,
                    );
                }
                if is_focus {
                    painter.rect_stroke(
                        r.expand(1.5),
                        CornerRadius::same(2),
                        Stroke::new(1.5, c),
                        StrokeKind::Outside,
                    );
                }
                if a.confidence < 0.5 {
                    // Geometry-only re-anchor: flagged.
                    painter.rect_stroke(
                        r.expand(3.0),
                        CornerRadius::ZERO,
                        Stroke::new(1.0, Color32::RED),
                        StrokeKind::Outside,
                    );
                }
            }
        }
        // Search hits.
        let hits: Vec<(usize, Range<usize>)> = self
            .search
            .matches
            .iter()
            .enumerate()
            .filter(|(_, (p, _))| *p == page)
            .map(|(i, (_, r))| (i, r.clone()))
            .collect();
        if !hits.is_empty() {
            let current = self.search.current;
            if let Some(t) = self.text_cloned(page) {
                for (i, r) in hits {
                    let col = if i == current {
                        Color32::from_rgb(255, 140, 0)
                    } else {
                        Color32::from_rgb(230, 200, 0)
                    };
                    for rect in t.rects(r) {
                        let sr = self.page_rect_to_screen(slot, [rect.x0, rect.y0, rect.x1, rect.y1]);
                        painter.rect_stroke(
                            sr.expand(1.0),
                            CornerRadius::same(2),
                            Stroke::new(1.5, col),
                            StrokeKind::Outside,
                        );
                    }
                }
            }
        }
        // Selection.
        let sel_color = Color32::from_rgba_unmultiplied(60, 120, 255, 70);
        match self.selection.clone() {
            Some(Selection::Text { page: p, range }) if p == page => {
                if let Some(t) = self.text(p) {
                    let rects = t.rects(range);
                    for rect in rects {
                        let sr = self.page_rect_to_screen(slot, [rect.x0, rect.y0, rect.x1, rect.y1]);
                        painter.rect_filled(sr, CornerRadius::ZERO, sel_color);
                    }
                }
            }
            Some(Selection::Region { page: p, rect }) | Some(Selection::Object { page: p, rect })
                if p == page =>
            {
                let sr = self.page_rect_to_screen(slot, [rect.x0, rect.y0, rect.x1, rect.y1]);
                painter.rect_filled(sr, CornerRadius::ZERO, sel_color);
                painter.rect_stroke(
                    sr,
                    CornerRadius::ZERO,
                    Stroke::new(1.5, Color32::from_rgb(60, 120, 255)),
                    StrokeKind::Outside,
                );
            }
            _ => {}
        }
        if let Some(Drag::Region { page: p, start, end }) = &self.drag
            && *p == page
        {
            let sr = self.page_rect_to_screen(
                slot,
                [start.0.min(end.0), start.1.min(end.1), start.0.max(end.0), start.1.max(end.1)],
            );
            painter.rect_stroke(
                sr,
                CornerRadius::ZERO,
                Stroke::new(1.5, Color32::from_rgb(60, 120, 255)),
                StrokeKind::Outside,
            );
        }
        // Flash after a jump.
        if let Some((p, quads, at)) = &self.flash {
            let t = at.elapsed().as_secs_f32();
            if t > 1.6 {
                self.flash = None;
            } else if *p == page {
                let alpha = ((1.0 - t / 1.6) * 255.0) as u8;
                let pulse = 3.0 + (t * 12.0).sin().abs() * 4.0;
                for q in quads {
                    let r = self.page_rect_to_screen(slot, q.bounds()).expand(pulse);
                    painter.rect_stroke(
                        r,
                        CornerRadius::same(3),
                        Stroke::new(2.5, Color32::from_rgba_unmultiplied(255, 90, 40, alpha)),
                        StrokeKind::Outside,
                    );
                }
                painter.ctx().request_repaint();
            }
        }
    }

    /// F-READ-10: marks for the whole document in a strip at the right.
    fn paint_gutter(&mut self, ui: &egui::Ui, painter: &egui::Painter, graph: &Graph) {
        let view = self.view;
        let strip = Rect::from_min_max(pos2(view.max.x - 10.0, view.min.y), view.max);
        painter.rect_filled(strip, CornerRadius::ZERO, Color32::from_black_alpha(70));
        let total = self.content_size().y.max(1.0);
        let slots = self.slots();
        for a in graph.anchors_in_doc(&self.doc.id) {
            if a.is_detached() {
                continue;
            }
            let Some(slot) = slots.iter().find(|s| s.page == a.page_index) else { continue };
            let y_pt = a.quads.first().map(|q| q.bounds()[1]).unwrap_or(0.0);
            let y = strip.min.y + (slot.pos.y + y_pt * self.zoom) / total * strip.height();
            let c = to_color32(a.color.unwrap_or_else(|| graph.entity_color(&a.entity)));
            painter
                .line_segment([pos2(strip.min.x + 1.0, y), pos2(strip.max.x - 1.0, y)], Stroke::new(2.0, c));
        }
        // Viewport indicator.
        let top = strip.min.y + self.scroll.y / total * strip.height();
        let h = (view.height() / total * strip.height()).max(6.0);
        painter.rect_stroke(
            Rect::from_min_size(pos2(strip.min.x, top), vec2(strip.width(), h)),
            CornerRadius::same(2),
            Stroke::new(1.0, Color32::from_white_alpha(120)),
            StrokeKind::Inside,
        );
        let resp = ui.interact(strip, ui.id().with(("gutter", self.render_idx)), Sense::click_and_drag());
        if let Some(p) = resp.interact_pointer_pos()
            && (resp.clicked() || resp.dragged())
        {
            self.scroll.y = ((p.y - strip.min.y) / strip.height() * total - view.height() / 2.0).max(0.0);
        }
    }

    fn handle_pointer(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        graph: &Graph,
        out: &mut ReaderOutput,
    ) {
        let (cmd, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
        let pointer = response.interact_pointer_pos();

        // egui reports a drag only after the pointer has moved a few pixels;
        // the selection must start where the button went down, not there.
        let press = ui.input(|i| i.pointer.press_origin()).or(pointer);
        if response.drag_started()
            && let Some(p) = press
            && let Some((page, x, y)) = self.screen_to_page(p)
        {
            if cmd {
                self.drag = Some(Drag::Region { page, start: (x, y), end: (x, y) });
            } else if let Some(i) = self.text(page).and_then(|t| t.caret_at(x, y)) {
                self.drag = Some(Drag::Text { page, start: i });
            }
        }
        if response.dragged()
            && let Some(p) = pointer
        {
            let state = match &self.drag {
                Some(Drag::Region { page, .. }) => Some((true, *page, 0)),
                Some(Drag::Text { page, start }) => Some((false, *page, *start)),
                None => None,
            };
            match state {
                Some((true, page, _)) => {
                    if let Some(slot) = self.slot_of(page) {
                        let local = ((p - self.view.min) + self.scroll - slot.pos) / self.zoom;
                        if let Some(Drag::Region { end, .. }) = &mut self.drag {
                            *end = (local.x, local.y);
                        }
                    }
                }
                Some((false, page, start)) => {
                    if let Some(slot) = self.slot_of(page) {
                        let local = ((p - self.view.min) + self.scroll - slot.pos) / self.zoom;
                        if let Some(end) = self.text(page).and_then(|t| t.caret_at(local.x, local.y)) {
                            let range = start.min(end)..start.max(end);
                            self.selection = (!range.is_empty()).then_some(Selection::Text { page, range });
                        }
                    }
                }
                None => {}
            }
            // Auto-scroll near the edges while selecting.
            if p.y > self.view.max.y - 20.0 {
                self.scroll.y += 12.0;
            } else if p.y < self.view.min.y + 20.0 {
                self.scroll.y -= 12.0;
            }
        }
        if response.drag_stopped() {
            if let Some(Drag::Region { page, start, end }) = self.drag.take() {
                let rect = PRect::new(start.0, start.1, end.0, end.1);
                if rect.width() > 3.0 && rect.height() > 3.0 {
                    self.selection = Some(Selection::Region { page, rect });
                }
            }
            self.drag = None;
        }

        let Some(p) = pointer else { return };
        let Some((page, x, y)) = self.screen_to_page(p) else {
            if response.clicked() {
                self.selection = None;
            }
            return;
        };
        if response.triple_clicked() {
            if let Some(t) = self.text(page)
                && let Some(i) = t.char_near(x, y, 6.0)
            {
                let range = t.line_at(i);
                self.selection = Some(Selection::Text { page, range });
            }
        } else if response.double_clicked() {
            if let Some(t) = self.text(page)
                && let Some(i) = t.char_near(x, y, 4.0)
            {
                let range = t.word_at(i);
                self.selection = Some(Selection::Text { page, range });
            }
        } else if response.clicked() {
            // Shift-click extends a text selection.
            if shift
                && let Some(Selection::Text { page: sp, range }) = self.selection.clone()
                && sp == page
                && let Some(i) = self.text(page).and_then(|t| t.caret_at(x, y))
            {
                let (s, e) = (range.start.min(i), range.end.max(i));
                self.selection = Some(Selection::Text { page, range: s..e });
                return;
            }
            // 1. Existing highlight → its entities (F-NODE-4).
            let hits: Vec<EntityId> = graph
                .anchors_on_page(&self.doc.id, page)
                .filter(|a| !a.is_detached() && a.quads.iter().any(|q| q.contains(x, y)))
                .map(|a| a.entity.clone())
                .filter(|e| !commands::is_highlight_carrier(e))
                .collect();
            if !hits.is_empty() {
                out.clicked_entities = hits;
                return;
            }
            // 2. Internal link.
            let link = self.links(page).iter().find(|l| l.rect.contains(x, y)).map(|l| l.target.clone());
            if let Some(target) = link {
                match target {
                    LinkTarget::Page(p) => self.goto(p, 0.0, true),
                    LinkTarget::Uri(u) => ui.ctx().open_url(egui::OpenUrl::new_tab(u)),
                }
                return;
            }
            // 3. Object hit-testing: the token under the cursor, else the
            //    smallest image/vector object (F-READ-5).
            if let Some(t) = self.text(page)
                && let Some(i) = t.char_at(x, y)
            {
                let range = t.word_at(i);
                self.selection = Some(Selection::Text { page, range });
                return;
            }
            let obj = self
                .objects(page)
                .iter()
                .filter(|o| o.rect.contains(x, y))
                .min_by(|a, b| a.rect.area().total_cmp(&b.rect.area()))
                .map(|o| o.rect);
            self.selection = obj.map(|rect| Selection::Object { page, rect });
        }
    }

    /// Screen rect of the current selection (to place the composer).
    pub fn selection_screen_rect(&mut self) -> Option<Rect> {
        let sel = self.selection.clone()?;
        let slot = self.slot_of(sel.page())?;
        let r = match sel {
            Selection::Text { page, range } => {
                let rects = self.text(page)?.rects(range);
                let u = rects.iter().skip(1).fold(*rects.first()?, |a, b| a.union(b));
                [u.x0, u.y0, u.x1, u.y1]
            }
            Selection::Region { rect, .. } | Selection::Object { rect, .. } => {
                [rect.x0, rect.y0, rect.x1, rect.y1]
            }
        };
        Some(self.page_rect_to_screen(&slot, r))
    }
}
