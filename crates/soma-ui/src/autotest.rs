//! Scripted end-to-end self-test (`SOMA_AUTOTEST=<dir>`): drives the real
//! capture → link → reify code paths through the running app, saves
//! screenshots, writes a report, and exits. Used to verify the UI without a
//! human at the keyboard.

use super::*;
use std::fmt::Write as _;

pub struct AutoTest {
    pub dir: PathBuf,
    frame: u64,
    step: usize,
    wait_until: u64,
    pending_shot: Option<String>,
    pub report: String,
    ids: Vec<EntityId>,
    /// Real key events fed through eframe's raw-input hook.
    pub inject: Vec<egui::Event>,
    /// An in-progress drag: from, to, step, release event.
    drag: Option<(egui::Pos2, egui::Pos2, u32, egui::Event)>,
    marks: (usize, usize, usize),
}

fn click(p: egui::Pos2) -> Vec<egui::Event> {
    let b = |pressed| egui::Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    vec![egui::Event::PointerMoved(p), b(true), b(false)]
}

fn key(k: egui::Key, modifiers: egui::Modifiers) -> [egui::Event; 2] {
    let ev = |pressed| egui::Event::Key { key: k, physical_key: Some(k), pressed, repeat: false, modifiers };
    [ev(true), ev(false)]
}

impl AutoTest {
    pub fn from_env() -> Option<Self> {
        let dir = PathBuf::from(std::env::var_os("SOMA_AUTOTEST")?);
        std::fs::create_dir_all(&dir).ok()?;
        Some(Self {
            dir,
            frame: 0,
            step: 0,
            wait_until: 0,
            pending_shot: None,
            report: String::new(),
            ids: vec![],
            inject: vec![],
            drag: None,
            marks: (0, 0, 0),
        })
    }
}

impl SomaApp {
    fn select_text(&mut self, needle: &str) -> bool {
        let Some(tab) = self.tabs.get_mut(self.active) else { return false };
        // Whitespace-insensitive: phrases may wrap across lines.
        let needle_l: Vec<char> = needle.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
        for page in 0..tab.page_count() {
            let Some(t) = tab.text_cloned(page) else { continue };
            let (hay, map): (Vec<char>, Vec<usize>) = t
                .chars
                .iter()
                .enumerate()
                .filter(|(_, c)| !c.ch.is_whitespace() && !c.ch.is_control())
                .map(|(i, c)| (c.ch.to_lowercase().next().unwrap_or(c.ch), i))
                .unzip();
            if let Some(j) = (0..hay.len().saturating_sub(needle_l.len()))
                .find(|&j| hay[j..j + needle_l.len()] == needle_l[..])
            {
                let (i, e) = (map[j], map[j + needle_l.len() - 1] + 1);
                tab.selection = Some(Selection::Text { page, range: i..e });
                let y = t.chars[i].rect.y0;
                tab.goto(page, (y - 150.0).max(0.0), false);
                return true;
            }
        }
        false
    }

    /// Press on the first glyph of `phrase`, move in steps, release on its
    /// last glyph — exactly what a user's drag sends.
    fn drag_test(&mut self, at: &mut AutoTest, backwards: bool) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let Some(Selection::Text { page, range }) = tab.selection.clone() else { return };
        tab.selection = None;
        let Some(t) = tab.text_cloned(page) else { return };
        let (a, b) = (t.chars[range.start].rect, t.chars[range.end - 1].rect);
        let (Some(p0), Some(p1)) = (
            tab.page_point_to_screen(page, a.x0 + 0.5, (a.y0 + a.y1) / 2.0),
            tab.page_point_to_screen(page, b.x1 - 0.5, (b.y0 + b.y1) / 2.0),
        ) else {
            return;
        };
        let (p0, p1) = if backwards { (p1, p0) } else { (p0, p1) };
        let btn = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        at.inject.push(egui::Event::PointerMoved(p0));
        at.inject.push(btn(p0, true));
        at.drag = Some((p0, p1, 0, btn(p1, false)));
    }

    pub(super) fn autotest_frame(&mut self, ctx: &egui::Context) {
        let Some(mut at) = self.autotest.take() else { return };
        at.frame += 1;
        if let Some((p0, p1, step, release)) = at.drag.take() {
            if step < 12 {
                let t = (step + 1) as f32 / 12.0;
                at.inject.push(egui::Event::PointerMoved(p0 + (p1 - p0) * t));
                at.drag = Some((p0, p1, step + 1, release));
            } else {
                at.inject.push(release);
            }
            ctx.request_repaint();
            self.autotest = Some(at);
            return;
        }
        ctx.request_repaint();
        // Collect a requested screenshot.
        if let Some(name) = at.pending_shot.clone() {
            let shot = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(img) = shot {
                let [w, h] = img.size;
                let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, bytes) {
                    let _ = buf.save(at.dir.join(&name));
                    let _ = writeln!(at.report, "screenshot {name} {w}x{h}");
                }
                at.pending_shot = None;
            }
            self.autotest = Some(at);
            return;
        }
        if at.frame < at.wait_until {
            self.autotest = Some(at);
            return;
        }
        let shot = |at: &mut AutoTest, ctx: &egui::Context, name: &str| {
            at.pending_shot = Some(name.to_owned());
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        };
        let g_rel = |s: &Self| s.ws.graph.relations.len();
        match at.step {
            0 => at.wait_until = at.frame + 40,
            1 => {
                // A1: selection → filed, highlighted, anchored node in 2 keystrokes (Ctrl, 1).
                let ok = self.select_text("quadratic variation");
                self.capture_into(1);
                let id = self.ws.created.front().cloned();
                let anchored = id.as_ref().is_some_and(|i| self.ws.graph.anchors_of(i).count() == 1);
                let filed = id.as_ref().is_some_and(|i| self.ws.graph.in_system(i, &SystemId::from("lapse")));
                let _ = writeln!(at.report, "A1 capture: selected={ok} anchored={anchored} filed={filed}");
                at.ids.extend(id);
            }
            2 => {
                // A2: second node on a different page, linked in 3 keystrokes (Ctrl, 1, L).
                let ok = self.select_text("the stochastic integral is an isometry");
                self.capture_into(1);
                let before = g_rel(self);
                self.link_previous();
                let new = self.ws.created.front().cloned();
                let linked = g_rel(self) == before + 1;
                let kind_ok = self.ws.graph.relations.values().any(|r| {
                    r.kind.as_str() == defaults::PREREQUISITE_OF && Some(r.target().unwrap()) == new.as_ref()
                });
                let _ =
                    writeln!(at.report, "A2 link: selected={ok} linked={linked} prerequisite-of={kind_ok}");
                at.ids.extend(new);
            }
            3 => {
                self.select_text("The martingale property together with the no-arbitrage condition");
                self.capture_into(4);
                at.ids.extend(self.ws.created.front().cloned());
                self.select_text("Dominated convergence");
                self.highlight();
                self.select_text("Girsanov's theorem");
                self.capture_into(2);
                at.ids.extend(self.ws.created.front().cloned());
                self.select_text("quadratic variation");
                if let Some(t) = self.tab() {
                    t.goto(0, 0.0, false);
                }
                at.wait_until = at.frame + 60;
            }
            4 => shot(&mut at, ctx, "1-reader.png"),
            5 => {
                // A3: annotate the relation, file it, make it an endpoint.
                let (a, b) = (at.ids[0].clone(), at.ids[1].clone());
                let rel = self
                    .ws
                    .graph
                    .incident(&a)
                    .find(|r| self.ws.graph.relations[*r].endpoint_ids().any(|e| *e == b))
                    .cloned();
                if let Some(rel) = rel {
                    let topo = self.ws.graph.topology();
                    self.commit_composer(Composer {
                        target: ComposerTarget::Edit(rel.clone()),
                        title: "I see the algebra but not why the limit exists".into(),
                        body: "Why does the sum of squared increments converge?".into(),
                        at: egui::Pos2::ZERO,
                        focus_requested: true,
                    });
                    if let Ok(tx) = commands::set_membership(
                        &self.ws.graph,
                        &rel,
                        &SystemId::from("lapse"),
                        true,
                        now_ms(),
                    ) {
                        let _ = self.ws.commit(tx);
                    }
                    let c = at.ids[2].clone();
                    self.finish_link(c.clone(), rel.clone());
                    let state = self.ws.graph.relation_state(&rel);
                    let path = self.ws.graph.shortest_path_len(&a, &b);
                    let on_rel = self.ws.graph.incident(&rel).count();
                    let _ = writeln!(
                        at.report,
                        "A3 reify: state={state:?} path_len={path:?} relations_on_relation={on_rel} topology_unchanged_by_annotation={}",
                        self.ws.graph.topology().is_superset(&topo)
                    );
                    self.focus(Some(rel));
                }
                let d = at.ids[3].clone();
                self.finish_link(at.ids[2].clone(), d);
                self.mode = Mode::Canvas;
                self.canvas.fit_next_frame();
                at.wait_until = at.frame + 150;
            }
            6 => shot(&mut at, ctx, "2-canvas-force.png"),
            7 => {
                self.canvas.set_mode(LayoutMode::Layered);
                at.wait_until = at.frame + 60;
            }
            8 => shot(&mut at, ctx, "3-canvas-layered.png"),
            9 => {
                // Overlay: only "open questions" → ghosting + ghost endpoints.
                self.overlay.active = vec![SystemId::from("lapse")];
                self.overlay_changed();
                self.canvas.set_mode(LayoutMode::Force);
                at.wait_until = at.frame + 90;
            }
            10 => shot(&mut at, ctx, "4-canvas-overlay.png"),
            11 => {
                // A4: delete the first node; one undo restores the cascade.
                let before = self.ws.graph.clone();
                let a = at.ids[0].clone();
                let cascade = self.ws.graph.cascade(&a).len();
                self.delete(&a);
                let gone = !self.ws.graph.contains(&a);
                let _ = writeln!(at.report, "A4 delete: cascade={cascade} removed={gone}");
                self.ws.undo();
                at.ids.push(EntityId::from(format!("{}", before.entity_count())));
                at.wait_until = at.frame + 20;
            }
            12 => {
                let expected: usize = at.ids.last().unwrap().0.parse().unwrap_or(0);
                let restored = self.ws.graph.entity_count() == expected && self.ws.graph.contains(&at.ids[0]);
                let inv = self.ws.graph.check_invariants().is_ok();
                let _ = writeln!(at.report, "A4 undo: restored={restored} invariants_ok={inv}");
                self.mode = Mode::Reader;
                self.theme = PaperTheme::Dark;
                at.wait_until = at.frame + 60;
            }
            13 => shot(&mut at, ctx, "5-reader-dark.png"),
            // ---- The same flows, driven by real key events.
            14 => {
                self.theme = PaperTheme::Light;
                self.select_text("Radon-Nikodym density process");
                at.marks =
                    (self.ws.graph.nodes.len(), self.ws.graph.relations.len(), self.ws.graph.anchors.len());
                at.inject.extend(key(egui::Key::Num1, egui::Modifiers::COMMAND));
                at.wait_until = at.frame + 5;
            }
            15 => {
                let made = self.ws.graph.nodes.len() == at.marks.0 + 1;
                let _ = writeln!(at.report, "KEY Cmd-1: node_created={made}");
                self.select_text("Novikov's condition");
                at.inject.extend(key(egui::Key::Num1, egui::Modifiers::COMMAND));
                at.wait_until = at.frame + 5;
            }
            16 => {
                at.marks.1 = self.ws.graph.relations.len();
                at.inject.extend(key(egui::Key::L, egui::Modifiers::SHIFT));
                at.wait_until = at.frame + 5;
            }
            17 => {
                let linked = self.ws.graph.relations.len() == at.marks.1 + 1;
                let _ = writeln!(
                    at.report,
                    "KEY Shift-L: linked={linked} kind_bar={}",
                    self.kind_offer.is_some()
                );
                shot(&mut at, ctx, "6-after-L.png");
            }
            18 => {
                // Something non-text gets keyboard focus (a clicked button,
                // or Tab focus navigation), then undo, then keep working.
                ctx.memory_mut(|m| m.request_focus(egui::Id::new("some-button")));
                at.inject.extend(key(egui::Key::Tab, egui::Modifiers::NONE));
                at.inject.extend(key(egui::Key::Tab, egui::Modifiers::NONE));
                at.inject.extend(key(egui::Key::Z, egui::Modifiers::COMMAND));
                at.wait_until = at.frame + 15;
            }
            19 => {
                let undone = self.ws.graph.relations.len() == at.marks.1;
                at.marks.2 = self.ws.graph.anchors.len();
                self.select_text("unit expectation");
                at.inject.extend(key(egui::Key::H, egui::Modifiers::NONE));
                let _ = writeln!(at.report, "KEY Cmd-Z: undone={undone}");
                at.wait_until = at.frame + 5;
            }
            20 => {
                let hl = self.ws.graph.anchors.len() == at.marks.2 + 1;
                let _ = writeln!(at.report, "KEY h after undo: highlighted={hl}");
                at.inject.extend(key(egui::Key::F1, egui::Modifiers::NONE));
                at.wait_until = at.frame + 5;
            }
            21 => {
                let help = matches!(self.popup, Popup::Help);
                let _ = writeln!(at.report, "KEY F1: keymap_open={help}");
                at.inject.extend(key(egui::Key::Escape, egui::Modifiers::NONE));
                at.wait_until = at.frame + 5;
            }
            22 => {
                // Click the toolbar undo button with the mouse, then keep using keys.
                at.marks.0 = self.ws.graph.anchors.len();
                at.inject.extend(click(self.undo_rect.center()));
                at.wait_until = at.frame + 15;
            }
            23 => {
                let undone = self.ws.graph.anchors.len() + 1 == at.marks.0;
                let _ = writeln!(
                    at.report,
                    "MOUSE undo button: undone={undone} focused_widget={:?}",
                    ctx.memory(|m| m.focused())
                );
                at.marks.1 = self.ws.graph.nodes.len();
                self.select_text("Dominated convergence lets us");
                at.inject.extend(key(egui::Key::Num2, egui::Modifiers::COMMAND));
                at.wait_until = at.frame + 5;
            }
            24 => {
                let made = self.ws.graph.nodes.len() == at.marks.1 + 1;
                let _ = writeln!(at.report, "KEY Cmd-2 after clicking undo: node_created={made}");
                // Scroll the phrase into view first; the drag happens next step.
                self.select_text("the Itô isometry the stochastic integral");
                at.wait_until = at.frame + 10;
            }
            25 => {
                self.drag_test(&mut at, false);
                at.wait_until = at.frame + 10;
            }
            26 => {
                let got = self.tab().and_then(|t| t.selection_text()).unwrap_or_default();
                let _ = writeln!(
                    at.report,
                    "MOUSE drag-select: expected=\"the Itô isometry the stochastic integral\" got={got:?}"
                );
                at.marks.1 = self.ws.graph.nodes.len();
                at.inject.extend(key(egui::Key::Num1, egui::Modifiers::COMMAND));
                at.wait_until = at.frame + 5;
            }
            27 => {
                at.marks.2 = self.ws.graph.relations.len();
                at.inject.extend(key(egui::Key::L, egui::Modifiers::SHIFT));
                at.wait_until = at.frame + 5;
            }
            28 => {
                let linked = self.ws.graph.relations.len() == at.marks.2 + 1;
                let _ = writeln!(
                    at.report,
                    "KEY Cmd-1 then Shift-L after mouse selection: node={} linked={linked} kind_bar={}",
                    self.ws.graph.nodes.len() == at.marks.1 + 1,
                    self.kind_offer.is_some()
                );
                // Tab to the canvas and back, as a user switching views would.
                at.inject.extend(key(egui::Key::Tab, egui::Modifiers::NONE));
                at.wait_until = at.frame + 10;
            }
            29 => {
                at.inject.extend(key(egui::Key::Tab, egui::Modifiers::NONE));
                at.wait_until = at.frame + 10;
            }
            30 => {
                at.marks.0 = self.ws.graph.anchors.len();
                self.select_text("positive martingale");
                at.inject.extend(key(egui::Key::H, egui::Modifiers::NONE));
                at.wait_until = at.frame + 5;
            }
            31 => {
                let hl = self.ws.graph.anchors.len() == at.marks.0 + 1;
                let focus = ctx.memory(|m| m.focused());
                let _ = writeln!(
                    at.report,
                    "KEY Tab, Tab, then h: mode_reader={} highlighted={hl} focused_widget={focus:?}",
                    self.mode == Mode::Reader
                );
                at.marks.0 = self.ws.graph.anchors.len();
                at.inject.extend(key(egui::Key::Z, egui::Modifiers::COMMAND));
                at.wait_until = at.frame + 15;
            }
            32 => {
                let undone = self.ws.graph.anchors.len() + 1 == at.marks.0;
                at.inject.extend(key(egui::Key::F1, egui::Modifiers::NONE));
                let _ = writeln!(at.report, "KEY Cmd-Z after Tabs: undone={undone}");
                at.wait_until = at.frame + 5;
            }
            33 => {
                let _ = writeln!(
                    at.report,
                    "KEY F1 after Tabs: keymap_open={}",
                    matches!(self.popup, Popup::Help)
                );
                self.popup = Popup::None;
                self.select_text(
                    "no-arbitrage condition implies the existence of an equivalent martingale measure",
                );
                at.wait_until = at.frame + 10;
            }
            34 => {
                let spans =
                    self.tab().and_then(|t| t.selection_screen_rect()).is_some_and(|r| r.height() > 20.0);
                let _ = writeln!(at.report, "(phrase spans lines: {spans})");
                self.drag_test(&mut at, true);
                at.wait_until = at.frame + 10;
            }
            35 => {
                let got = self.tab().and_then(|t| t.selection_text()).unwrap_or_default();
                let _ = writeln!(at.report, "MOUSE backwards multi-line drag: got={got:?}");
            }
            _ => {
                let _ = std::fs::write(at.dir.join("report.txt"), &at.report);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
        }
        at.step += 1;
        self.autotest = Some(at);
    }
}
