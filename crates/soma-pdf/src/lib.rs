//! PDF access for Soma: rendering, text with per-character geometry, outline,
//! links, object hit-testing, and anchor capture/resolution (PRD §4.2, §5.1).
//!
//! PDFium sits behind the [`PdfBackend`] trait so that MuPDF or a pure-Rust
//! backend can be swapped in (PRD §10).

pub mod anchor;
pub mod testpdf;
pub mod text;

use pdfium_render::prelude::*;
use soma_core::{Document, DocumentId};
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use text::{PageText, Rect, TextChar};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PdfError {
    #[error("PDFium library not found ({0}). Run scripts/fetch-pdfium.sh or set PDFIUM_DYNAMIC_LIB_PATH")]
    Library(String),
    #[error("pdfium: {0}")]
    Pdfium(#[from] PdfiumError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("page {0} out of range")]
    PageOutOfRange(u32),
}

pub type Result<T> = std::result::Result<T, PdfError>;

#[derive(Clone, Debug, PartialEq)]
pub struct OutlineItem {
    pub title: String,
    pub page: Option<u32>,
    pub depth: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LinkTarget {
    Page(u32),
    Uri(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub rect: Rect,
    pub target: LinkTarget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectKind {
    Image,
    Path,
    Shading,
    Form,
}

/// A non-text page object, for click hit-testing (F-READ-5).
#[derive(Clone, Debug, PartialEq)]
pub struct PageObject {
    pub kind: ObjectKind,
    pub rect: Rect,
}

/// RGBA8 pixels, row-major, unpremultiplied.
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Everything the reader needs from a PDF engine.
pub trait PdfBackend {
    fn page_count(&self) -> u32;
    /// (width, height) in points.
    fn page_size(&self, page: u32) -> Result<(f32, f32)>;
    /// Render the region `[x, x+w) × [y, y+h)` (pixels) of the page scaled by
    /// `scale` pixels per point.
    fn render_tile(&self, page: u32, scale: f32, x: i32, y: i32, w: u32, h: u32) -> Result<Bitmap>;
    fn page_text(&self, page: u32) -> Result<PageText>;
    fn objects(&self, page: u32) -> Result<Vec<PageObject>>;
    fn links(&self, page: u32) -> Result<Vec<Link>>;
    fn outline(&self) -> Vec<OutlineItem>;
    fn title(&self) -> Option<String>;
}

// ------------------------------------------------------------------ loading

static PDFIUM: OnceLock<std::result::Result<Pdfium, String>> = OnceLock::new();

/// PDFium is not safe to use from several threads at once, even for
/// different documents (PRD §7.2). Every call into it holds this lock.
static PDFIUM_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn lock() -> MutexGuard<'static, ()> {
    PDFIUM_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(p) = std::env::var("PDFIUM_DYNAMIC_LIB_PATH") {
        let p = PathBuf::from(p);
        dirs.push(if p.is_file() { p.parent().map(Path::to_path_buf).unwrap_or(p) } else { p });
    }
    let mut roots = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(d) = exe.parent()
    {
        dirs.push(d.to_path_buf());
        dirs.push(d.join("../Frameworks"));
        dirs.push(d.join("../lib"));
        roots.push(d.to_path_buf());
    }
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd);
    }
    roots.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    for root in roots {
        for anc in root.ancestors() {
            dirs.push(anc.join("vendor/pdfium/lib"));
        }
    }
    dirs
}

/// The process-wide PDFium instance.
pub fn pdfium() -> Result<&'static Pdfium> {
    PDFIUM
        .get_or_init(|| {
            let mut tried = Vec::new();
            for dir in candidate_dirs() {
                let lib = Pdfium::pdfium_platform_library_name_at_path(&dir);
                if lib.exists() {
                    match Pdfium::bind_to_library(&lib) {
                        Ok(b) => return Ok(Pdfium::new(b)),
                        Err(e) => tried.push(format!("{}: {e}", lib.display())),
                    }
                }
            }
            Pdfium::bind_to_system_library()
                .map(Pdfium::new)
                .map_err(|e| format!("system: {e}; {}", tried.join("; ")))
        })
        .as_ref()
        .map_err(|e| PdfError::Library(e.clone()))
}

pub fn is_available() -> bool {
    pdfium().is_ok()
}

/// Content hash used as document identity.
pub fn hash_file(path: &Path) -> Result<DocumentId> {
    let bytes = std::fs::read(path)?;
    Ok(DocumentId(blake3::hash(&bytes).to_hex().to_string()))
}

/// Hash and inspect a PDF for registration in a workspace.
pub fn identify(path: &Path) -> Result<Document> {
    let path = std::fs::canonicalize(path)?;
    let id = hash_file(&path)?;
    let doc = PdfiumDoc::open(&path)?;
    Ok(Document {
        id,
        path: path.to_string_lossy().into_owned(),
        title: doc.title().or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned())),
        page_count: doc.page_count(),
        copied_local: false,
        added_at: soma_core::now_ms(),
    })
}

// ------------------------------------------------------------------- PDFium

pub struct PdfiumDoc {
    doc: ManuallyDrop<PdfDocument<'static>>,
    sizes: Vec<(f32, f32)>,
}

impl PdfiumDoc {
    pub fn open(path: &Path) -> Result<Self> {
        let _g = lock();
        let doc = pdfium()?.load_pdf_from_file(path, None)?;
        Self::from_document(doc)
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        let _g = lock();
        let doc = pdfium()?.load_pdf_from_byte_vec(bytes, None)?;
        Self::from_document(doc)
    }

    fn from_document(doc: PdfDocument<'static>) -> Result<Self> {
        let sizes = doc.pages().iter().map(|p| (p.width().value, p.height().value)).collect();
        Ok(Self { doc: ManuallyDrop::new(doc), sizes })
    }

    fn page(&self, page: u32) -> Result<PdfPage<'_>> {
        if page as usize >= self.sizes.len() {
            return Err(PdfError::PageOutOfRange(page));
        }
        Ok(self.doc.pages().get(page as PdfPageIndex)?)
    }

    fn to_rect(&self, page: u32, r: PdfRect) -> Rect {
        let h = self.sizes[page as usize].1;
        Rect::new(r.left().value, h - r.top().value, r.right().value, h - r.bottom().value)
    }

    fn dest_page(d: &PdfDestination) -> Option<u32> {
        d.page_index().ok().map(|i| i as u32)
    }
}

impl Drop for PdfiumDoc {
    fn drop(&mut self) {
        let _g = lock();
        // SAFETY: dropped exactly once, here, under the PDFium lock.
        unsafe { ManuallyDrop::drop(&mut self.doc) };
    }
}

impl PdfBackend for PdfiumDoc {
    fn page_count(&self) -> u32 {
        self.sizes.len() as u32
    }

    fn page_size(&self, page: u32) -> Result<(f32, f32)> {
        self.sizes.get(page as usize).copied().ok_or(PdfError::PageOutOfRange(page))
    }

    fn render_tile(&self, page: u32, scale: f32, x: i32, y: i32, w: u32, h: u32) -> Result<Bitmap> {
        let _g = lock();
        let p = self.page(page)?;
        let mut bitmap = PdfBitmap::empty(w as Pixels, h as Pixels, PdfBitmapFormat::BGRA)?;
        // Offset via the transform matrix (in page points, before scaling).
        // PDFium's origin offset only works on the form-data render path,
        // which is not safe to use from several threads at once.
        let config = PdfRenderConfig::new()
            .scale_page_by_factor(scale)
            .translate(PdfPoints::new(-x as f32 / scale), PdfPoints::new(-y as f32 / scale))?
            .render_annotations(true)
            .set_clear_color(PdfColor::WHITE);
        p.render_into_bitmap_with_config(&mut bitmap, &config)?;
        Ok(Bitmap { width: w, height: h, rgba: bitmap.as_rgba_bytes() })
    }

    fn page_text(&self, page: u32) -> Result<PageText> {
        let _g = lock();
        let p = self.page(page)?;
        let text = p.text()?;
        let mut chars = Vec::with_capacity(text.chars().len());
        for c in text.chars().iter() {
            let ch = c.unicode_char().unwrap_or('\u{fffd}');
            let generated = c.is_generated().unwrap_or(false);
            let rect = if generated {
                Rect::new(0.0, 0.0, 0.0, 0.0)
            } else {
                c.loose_bounds().map(|r| self.to_rect(page, r)).unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0))
            };
            chars.push(TextChar { ch, rect, generated });
        }
        Ok(PageText { chars })
    }

    fn objects(&self, page: u32) -> Result<Vec<PageObject>> {
        let _g = lock();
        let p = self.page(page)?;
        let mut out = Vec::new();
        for o in p.objects().iter() {
            let kind = match o.object_type() {
                PdfPageObjectType::Image => ObjectKind::Image,
                PdfPageObjectType::Path => ObjectKind::Path,
                PdfPageObjectType::Shading => ObjectKind::Shading,
                PdfPageObjectType::XObjectForm => ObjectKind::Form,
                _ => continue,
            };
            if let Ok(b) = o.bounds() {
                let rect = self.to_rect(page, b.to_rect());
                if rect.area() > 4.0 {
                    out.push(PageObject { kind, rect });
                }
            }
        }
        Ok(out)
    }

    fn links(&self, page: u32) -> Result<Vec<Link>> {
        let _g = lock();
        let p = self.page(page)?;
        let mut out = Vec::new();
        for l in p.links().iter() {
            let Ok(rect) = l.rect() else { continue };
            let target = if let Some(page) = l.destination().as_ref().and_then(Self::dest_page) {
                LinkTarget::Page(page)
            } else if let Some(action) = l.action() {
                if let Some(uri) = action.as_uri_action().and_then(|u| u.uri().ok()) {
                    LinkTarget::Uri(uri)
                } else if let Some(page) = action
                    .as_local_destination_action()
                    .and_then(|a| a.destination().ok())
                    .as_ref()
                    .and_then(Self::dest_page)
                {
                    LinkTarget::Page(page)
                } else {
                    continue;
                }
            } else {
                continue;
            };
            out.push(Link { rect: self.to_rect(page, rect), target });
        }
        Ok(out)
    }

    fn outline(&self) -> Vec<OutlineItem> {
        let _g = lock();
        fn walk(b: PdfBookmark, depth: usize, out: &mut Vec<OutlineItem>) {
            if out.len() > 5000 {
                return;
            }
            out.push(OutlineItem {
                title: b.title().unwrap_or_default(),
                page: b.destination().as_ref().and_then(PdfiumDoc::dest_page),
                depth,
            });
            for c in b.iter_direct_children() {
                walk(c, depth + 1, out);
            }
        }
        let mut out = Vec::new();
        if let Some(root) = self.doc.bookmarks().root() {
            for b in root.iter_siblings() {
                walk(b, 0, &mut out);
            }
        }
        out
    }

    fn title(&self) -> Option<String> {
        let _g = lock();
        self.doc
            .metadata()
            .get(PdfDocumentMetadataTagType::Title)
            .map(|t| t.value().to_owned())
            .filter(|t| !t.trim().is_empty())
    }
}

/// Search every page for `query` (case-insensitive), returning
/// `(page, char range)` matches in reading order (F-READ-7).
pub fn search(
    texts: &mut dyn FnMut(u32) -> Option<PageText>,
    page_count: u32,
    query: &str,
) -> Vec<(u32, std::ops::Range<usize>)> {
    let q: Vec<char> = query.to_lowercase().chars().collect();
    if q.is_empty() {
        return vec![];
    }
    let mut out = Vec::new();
    for page in 0..page_count {
        let Some(t) = texts(page) else { continue };
        let hay: Vec<char> = t.chars.iter().map(|c| c.ch.to_lowercase().next().unwrap_or(c.ch)).collect();
        let mut i = 0;
        while i + q.len() <= hay.len() {
            if hay[i..i + q.len()] == q[..] {
                out.push((page, i..i + q.len()));
                i += q.len();
            } else {
                i += 1;
            }
        }
    }
    out
}
