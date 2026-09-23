//! Integration tests against real PDFium. Skipped (with a message) when the
//! library is not installed — run scripts/fetch-pdfium.sh.

use soma_core::*;
use soma_pdf::anchor::{self, Resolution};
use soma_pdf::testpdf::{flow, make_pdf};
use soma_pdf::{PdfBackend, PdfiumDoc};

const CORPUS: &str = "Let X be a continuous semimartingale with respect to the filtration. The quadratic variation of X \
is defined as the limit in probability of sums of squared increments over partitions whose mesh tends to zero. \
By the Itô isometry the stochastic integral of a predictable process is an isometry between the space of square \
integrable integrands and the space of square integrable martingales. The martingale property together with the \
no-arbitrage condition implies the existence of an equivalent martingale measure under which discounted prices \
are martingales. Dominated convergence lets us exchange limit and expectation when the integrands are bounded by \
an integrable random variable. Girsanov's theorem describes how the drift changes under an equivalent change of \
measure, and the Radon-Nikodym derivative is itself a positive martingale with unit expectation.";

fn words(n_repeats: usize) -> Vec<String> {
    (0..n_repeats)
        .flat_map(|i| {
            CORPUS
                .split_whitespace()
                .map(move |w| if w == "Let" { format!("Section{i}:") } else { w.to_owned() })
        })
        .collect()
}

macro_rules! require_pdfium {
    () => {
        if !soma_pdf::is_available() {
            eprintln!("PDFium not available; skipping");
            return;
        }
    };
}

#[test]
fn text_extraction_has_geometry_and_two_columns_read_in_order() {
    require_pdfium!();
    let w = words(12);
    let w: Vec<&str> = w.iter().map(String::as_str).collect();
    let bytes = make_pdf(&flow(&w, 40, 2)).unwrap();
    let doc = PdfiumDoc::from_bytes(bytes).unwrap();
    assert!(doc.page_count() >= 1);
    let (pw, ph) = doc.page_size(0).unwrap();
    assert!((pw - 612.0).abs() < 2.0 && (ph - 792.0).abs() < 2.0, "{pw}x{ph}");
    let t = doc.page_text(0).unwrap();
    let s = t.text();
    assert!(s.contains("quadratic"), "{s}");
    // Every painted char has a box inside the page, y-down.
    for c in t.chars.iter().filter(|c| !c.generated && !c.ch.is_whitespace()) {
        assert!(c.rect.x0 >= 60.0 && c.rect.x1 <= pw && c.rect.y0 >= 60.0 && c.rect.y1 <= ph, "{c:?}");
    }
    // Selection → quads: hit-test the first "quadratic", then its word.
    let i = s.find("quadratic").unwrap();
    let i = s[..i].chars().count();
    let (cx, cy) = t.chars[i + 2].rect.center();
    let hit = t.char_at(cx, cy).unwrap();
    assert_eq!(t.slice(t.word_at(hit)), "quadratic");
    let quads = t.quads(t.word_at(hit));
    assert_eq!(quads.len(), 1);
}

#[test]
fn rendering_produces_pixels() {
    require_pdfium!();
    let bytes = make_pdf(&flow(&["hello", "world"], 40, 1)).unwrap();
    let doc = PdfiumDoc::from_bytes(bytes).unwrap();
    let bm = doc.render_tile(0, 2.0, 0, 0, 512, 512).unwrap();
    assert_eq!(bm.rgba.len(), 512 * 512 * 4);
    // Some dark pixels (the text) and mostly white.
    let dark = bm.rgba.chunks(4).filter(|p| p[0] < 128).count();
    assert!(dark > 20, "dark={dark}");
    assert!(dark < 512 * 512 / 4);
}

/// Acceptance test A5: a re-downloaded paper with different bytes (new
/// front matter, reflowed from 2 columns to 1, a word changed) re-anchors
/// ≥95% of text anchors at confidence ≥0.9; none are lost.
#[test]
fn a5_reanchor_after_revision() {
    require_pdfium!();
    let w1 = words(20);
    let w1r: Vec<&str> = w1.iter().map(String::as_str).collect();
    let v1 = PdfiumDoc::from_bytes(make_pdf(&flow(&w1r, 42, 2)).unwrap()).unwrap();

    let mut w2: Vec<String> =
        "Preprint version 2. Corrections and a new preface were added to this revision."
            .split_whitespace()
            .map(str::to_owned)
            .collect();
    w2.extend(w1.iter().map(|w| if w == "bounded" { "dominated".to_owned() } else { w.clone() }));
    let w2r: Vec<&str> = w2.iter().map(String::as_str).collect();
    let v2 = PdfiumDoc::from_bytes(make_pdf(&flow(&w2r, 70, 1)).unwrap()).unwrap();
    assert_ne!(v1.page_count(), 0);

    // Capture an anchor on every 3–5 word phrase starting at every 23rd word.
    let doc_id = DocumentId::from("v1");
    let mut anchors = Vec::new();
    for page in 0..v1.page_count() {
        let t = v1.page_text(page).unwrap();
        let text = t.text();
        let starts: Vec<usize> = text
            .char_indices()
            .filter(|(i, c)| c.is_alphabetic() && (*i == 0 || text[..*i].ends_with(char::is_whitespace)))
            .map(|(i, _)| text[..i].chars().count())
            .collect();
        for (k, s) in starts.iter().enumerate().step_by(23) {
            let Some(&e) = starts.get(k + 3 + k % 3) else { continue };
            anchors.push(anchor::capture_text(&doc_id, page, &t, *s..e - 1, None));
        }
    }
    assert!(anchors.len() >= 40, "only {} anchors", anchors.len());

    let mut good = 0;
    for a in &anchors {
        let r = anchor::resolve(a, v2.page_count(), &mut |p| v2.page_text(p).ok());
        match &r {
            Resolution::Found(f) if f.confidence >= 0.9 => {
                let t = v2.page_text(f.page_index).unwrap();
                let got = soma_pdf::text::normalize_ws(&t.slice(f.char_range.clone().unwrap()));
                if got.replace(' ', "") == a.exact.replace(' ', "") {
                    good += 1;
                }
            }
            Resolution::Detached => panic!("anchor lost: {:?}", a.exact),
            _ => {}
        }
    }
    let ratio = good as f64 / anchors.len() as f64;
    assert!(ratio >= 0.95, "re-anchored {good}/{} = {ratio:.3}", anchors.len());
}

/// A tile must equal the matching crop of a full-page render (catches
/// origin/offset bugs that make every tile show the page's corner).
#[test]
fn tiles_match_full_page_crop() {
    require_pdfium!();
    let w = words(12);
    let w: Vec<&str> = w.iter().map(String::as_str).collect();
    let doc = PdfiumDoc::from_bytes(make_pdf(&flow(&w, 40, 2)).unwrap()).unwrap();
    let scale = 2.0;
    let (pw, ph) = doc.page_size(0).unwrap();
    let (fw, fh) = ((pw * scale).ceil() as u32, (ph * scale).ceil() as u32);
    let full = doc.render_tile(0, scale, 0, 0, fw, fh).unwrap();
    let (tx, ty, ts) = (300u32, 400u32, 256u32);
    let tile = doc.render_tile(0, scale, tx as i32, ty as i32, ts, ts).unwrap();
    let mut diff = 0u64;
    let mut dark = 0;
    for y in 0..ts {
        for x in 0..ts {
            let t = ((y * ts + x) * 4) as usize;
            let f = (((ty + y) * fw + tx + x) * 4) as usize;
            diff += (tile.rgba[t] as i64 - full.rgba[f] as i64).unsigned_abs();
            dark += (tile.rgba[t] < 128) as u32;
        }
    }
    assert!(dark > 50, "tile should contain text");
    let mean = diff as f64 / (ts * ts) as f64;
    assert!(mean < 4.0, "mean abs diff {mean}");
}
