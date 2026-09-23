//! Generate simple text PDFs with PDFium — for tests, benchmarks and demos.

use crate::{Result, pdfium};
use pdfium_render::prelude::*;

/// One positioned line of text: `(x, y_from_top, text)` in points.
pub type Line = (f32, f32, String);

pub const FONT_SIZE: f32 = 10.0;
pub const LINE_HEIGHT: f32 = 13.0;

/// Build a US-letter PDF where each page is a list of positioned lines.
pub fn make_pdf(pages: &[Vec<Line>]) -> Result<Vec<u8>> {
    let mut doc = pdfium()?.create_new_pdf()?;
    let font = doc.fonts_mut().times_roman();
    for lines in pages {
        let mut page = doc
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::new_portrait(PdfPagePaperStandardSize::USLetterAnsiA))?;
        let h = page.height().value;
        for (x, y, text) in lines {
            page.objects_mut().create_text_object(
                PdfPoints::new(*x),
                PdfPoints::new(h - *y - FONT_SIZE),
                text,
                font,
                PdfPoints::new(FONT_SIZE),
            )?;
        }
    }
    Ok(doc.save_to_bytes()?)
}

/// Flow `words` into columns of `chars_per_line` characters, `columns` per
/// page, 50 lines per column. Returns pages of positioned lines.
pub fn flow(words: &[&str], chars_per_line: usize, columns: usize) -> Vec<Vec<Line>> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for w in words {
        if !cur.is_empty() && cur.len() + 1 + w.len() > chars_per_line {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    let per_col = 50;
    let col_width = 468.0 / columns as f32;
    lines
        .chunks(per_col * columns)
        .map(|page| {
            page.iter()
                .enumerate()
                .map(|(i, l)| {
                    let col = i / per_col;
                    let row = i % per_col;
                    (72.0 + col as f32 * col_width, 72.0 + row as f32 * LINE_HEIGHT, l.clone())
                })
                .collect()
        })
        .collect()
}
