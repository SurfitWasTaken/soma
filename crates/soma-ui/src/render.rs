//! Render worker (PRD §7.2): PDFium rasterization off the UI thread, into an
//! LRU cache of GPU textures.
//!
//! The UI publishes the complete list of tiles it wants each frame; the
//! worker always renders the first wanted tile it hasn't produced yet, so
//! requests that scrolled out of view are dropped rather than queued.

use egui::{ColorImage, TextureHandle, TextureOptions};
use soma_pdf::{PdfBackend, PdfiumDoc};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

pub const TILE: u32 = 512;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PaperTheme {
    Light,
    Sepia,
    /// Lightness inverted, hue kept — figures stay recognisable (F-READ-9).
    Dark,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct TileKey {
    pub doc: usize,
    pub page: u32,
    /// Quantized scale step (see [`scale_step`]).
    pub step: i32,
    pub tx: i32,
    pub ty: i32,
    pub theme: PaperTheme,
}

/// Scales are quantized to steps of 2^(1/4) so zooming reuses tiles.
pub fn scale_step(scale: f32) -> i32 {
    (scale.max(0.05).log2() * 4.0).round() as i32
}

pub fn step_scale(step: i32) -> f32 {
    2f32.powf(step as f32 / 4.0)
}

struct Shared {
    wanted: Vec<TileKey>,
    /// Taken by the worker, result not yet collected.
    taken: HashSet<TileKey>,
    docs: Vec<PathBuf>,
    shutdown: bool,
}

pub struct Renderer {
    shared: Arc<(Mutex<Shared>, Condvar)>,
    rx: Receiver<(TileKey, Option<ColorImage>)>,
    cache: HashMap<TileKey, (TextureHandle, u64)>,
    failed: HashSet<TileKey>,
    frame: u64,
    pub max_tiles: usize,
}

impl Renderer {
    pub fn new(ctx: egui::Context) -> Self {
        let shared = Arc::new((
            Mutex::new(Shared { wanted: vec![], taken: HashSet::new(), docs: vec![], shutdown: false }),
            Condvar::new(),
        ));
        let (tx, rx) = mpsc::channel();
        let worker_shared = shared.clone();
        std::thread::Builder::new()
            .name("soma-render".into())
            .spawn(move || worker(worker_shared, tx, ctx))
            .expect("spawn render worker");
        Self { shared, rx, cache: HashMap::new(), failed: HashSet::new(), frame: 0, max_tiles: 384 }
    }

    /// Register a document path; returns its index for [`TileKey::doc`].
    pub fn add_doc(&self, path: PathBuf) -> usize {
        let mut s = self.shared.0.lock().unwrap();
        if let Some(i) = s.docs.iter().position(|p| *p == path) {
            return i;
        }
        s.docs.push(path);
        s.docs.len() - 1
    }

    /// Collect finished tiles into textures. Call once per frame.
    pub fn begin_frame(&mut self, ctx: &egui::Context) {
        self.frame += 1;
        let done: Vec<_> = self.rx.try_iter().collect();
        if !done.is_empty() {
            let mut s = self.shared.0.lock().unwrap();
            for (k, _) in &done {
                s.taken.remove(k);
            }
        }
        for (key, img) in done {
            match img {
                Some(img) => {
                    let name = format!("tile-{}-{}-{}-{}-{}", key.doc, key.page, key.step, key.tx, key.ty);
                    let tex = ctx.load_texture(name, img, TextureOptions::LINEAR);
                    self.cache.insert(key, (tex, self.frame));
                }
                None => {
                    self.failed.insert(key);
                }
            }
        }
        if self.cache.len() > self.max_tiles {
            let mut by_age: Vec<(u64, TileKey)> =
                self.cache.iter().map(|(k, (_, f))| (*f, k.clone())).collect();
            by_age.sort_by_key(|(f, _)| *f);
            for (_, k) in by_age.into_iter().take(self.cache.len() - self.max_tiles) {
                self.cache.remove(&k);
            }
        }
    }

    /// Look up a tile, marking it recently used.
    pub fn get(&mut self, key: &TileKey) -> Option<&TextureHandle> {
        let frame = self.frame;
        self.cache.get_mut(key).map(|(t, f)| {
            *f = frame;
            &*t
        })
    }

    /// Replace the wanted list (priority order). Tiles already cached,
    /// in flight or failed are skipped.
    pub fn want(&mut self, keys: Vec<TileKey>) {
        let keys: Vec<TileKey> =
            keys.into_iter().filter(|k| !self.cache.contains_key(k) && !self.failed.contains(k)).collect();
        let (lock, cv) = &*self.shared;
        let mut s = lock.lock().unwrap();
        s.wanted = keys.into_iter().filter(|k| !s.taken.contains(k)).collect();
        cv.notify_one();
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        let (lock, cv) = &*self.shared;
        lock.lock().unwrap().shutdown = true;
        cv.notify_one();
    }
}

fn worker(
    shared: Arc<(Mutex<Shared>, Condvar)>,
    tx: Sender<(TileKey, Option<ColorImage>)>,
    ctx: egui::Context,
) {
    let mut docs: HashMap<usize, Option<PdfiumDoc>> = HashMap::new();
    loop {
        let (key, path) = {
            let (lock, cv) = &*shared;
            let mut s = lock.lock().unwrap();
            while s.wanted.is_empty() && !s.shutdown {
                s = cv.wait(s).unwrap();
            }
            if s.shutdown {
                return;
            }
            let key = s.wanted.remove(0);
            s.taken.insert(key.clone());
            let path = s.docs.get(key.doc).cloned();
            (key, path)
        };
        let doc = docs.entry(key.doc).or_insert_with(|| path.and_then(|p| PdfiumDoc::open(&p).ok()));
        let img = doc.as_ref().and_then(|d| render(d, &key));
        if tx.send((key, img)).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

fn render(doc: &PdfiumDoc, key: &TileKey) -> Option<ColorImage> {
    let scale = step_scale(key.step);
    let (pw, ph) = doc.page_size(key.page).ok()?;
    let (full_w, full_h) = ((pw * scale).ceil() as i32, (ph * scale).ceil() as i32);
    let x = key.tx * TILE as i32;
    let y = key.ty * TILE as i32;
    let w = (full_w - x).clamp(1, TILE as i32) as u32;
    let h = (full_h - y).clamp(1, TILE as i32) as u32;
    let mut bm = doc.render_tile(key.page, scale, x, y, w, h).ok()?;
    apply_theme(&mut bm.rgba, key.theme);
    Some(ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &bm.rgba))
}

pub fn apply_theme(rgba: &mut [u8], theme: PaperTheme) {
    match theme {
        PaperTheme::Light => {}
        PaperTheme::Sepia => {
            for p in rgba.chunks_exact_mut(4) {
                p[0] = (p[0] as f32 * 0.98 + 2.0) as u8;
                p[1] = (p[1] as f32 * 0.92 + 4.0) as u8;
                p[2] = (p[2] as f32 * 0.78 + 6.0) as u8;
            }
        }
        PaperTheme::Dark => {
            // Invert luma, keep chroma: text and paper swap, figure hues stay.
            for p in rgba.chunks_exact_mut(4) {
                let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
                let y = 0.299 * r + 0.587 * g + 0.114 * b;
                let target = 24.0 + (255.0 - y) * 0.82;
                let d = target - y;
                p[0] = (r + d).clamp(0.0, 255.0) as u8;
                p[1] = (g + d).clamp(0.0, 255.0) as u8;
                p[2] = (b + d).clamp(0.0, 255.0) as u8;
            }
        }
    }
}
