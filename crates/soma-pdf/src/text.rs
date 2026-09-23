//! Extracted page text with per-character geometry. Pure data: selection,
//! hit-testing and quad generation work without PDFium, so they are tested
//! directly.
//!
//! Coordinates are page space in PDF points with the origin at the top-left
//! and y pointing down (the reader's convention). Character offsets are
//! indices into [`PageText::chars`], which is also how anchors store
//! `char_start..char_end`.

use soma_core::Quad;
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Rect { x0: x0.min(x1), y0: y0.min(y1), x1: x0.max(x1), y1: y0.max(y1) }
    }
    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }
    pub fn center(&self) -> (f32, f32) {
        ((self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0)
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }
    pub fn intersects(&self, o: &Rect) -> bool {
        self.x0 < o.x1 && o.x0 < self.x1 && self.y0 < o.y1 && o.y0 < self.y1
    }
    pub fn union(&self, o: &Rect) -> Rect {
        Rect { x0: self.x0.min(o.x0), y0: self.y0.min(o.y0), x1: self.x1.max(o.x1), y1: self.y1.max(o.y1) }
    }
    pub fn area(&self) -> f32 {
        self.width().max(0.0) * self.height().max(0.0)
    }
    pub fn distance_sq(&self, x: f32, y: f32) -> f32 {
        let dx = (self.x0 - x).max(0.0).max(x - self.x1);
        let dy = (self.y0 - y).max(0.0).max(y - self.y1);
        dx * dx + dy * dy
    }
    pub fn to_quad(&self) -> Quad {
        Quad::from_rect(self.x0, self.y0, self.x1, self.y1)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextChar {
    pub ch: char,
    /// Loose bounds (full line height), page space. Zero-area for
    /// PDFium-generated characters such as synthesized spaces and newlines.
    pub rect: Rect,
    /// Synthesized by the text extractor (not painted on the page).
    pub generated: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageText {
    pub chars: Vec<TextChar>,
}

impl PageText {
    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    pub fn text(&self) -> String {
        self.chars.iter().map(|c| c.ch).collect()
    }

    pub fn slice(&self, r: Range<usize>) -> String {
        let r = r.start.min(self.len())..r.end.min(self.len());
        self.chars[r].iter().map(|c| c.ch).collect()
    }

    fn has_box(c: &TextChar) -> bool {
        c.rect.area() > 0.0
    }

    /// The character whose box contains the point.
    pub fn char_at(&self, x: f32, y: f32) -> Option<usize> {
        self.chars.iter().position(|c| Self::has_box(c) && c.rect.contains(x, y))
    }

    /// Nearest character within `tolerance` points — used while dragging a
    /// selection through gaps between glyphs.
    pub fn char_near(&self, x: f32, y: f32, tolerance: f32) -> Option<usize> {
        if let Some(i) = self.char_at(x, y) {
            return Some(i);
        }
        let t2 = tolerance * tolerance;
        self.chars
            .iter()
            .enumerate()
            .filter(|(_, c)| Self::has_box(c))
            .map(|(i, c)| (i, c.rect.distance_sq(x, y)))
            .filter(|(_, d)| *d <= t2)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// Selection caret position for a point: the character index where a
    /// selection starting/ending here should begin or end. Picks the side of
    /// the nearest glyph the point falls on.
    pub fn caret_at(&self, x: f32, y: f32) -> Option<usize> {
        let i = self.char_near(x, y, 40.0)?;
        let r = self.chars[i].rect;
        Some(if x > r.center().0 { i + 1 } else { i })
    }

    fn is_word_char(c: char) -> bool {
        c.is_alphanumeric() || c == '-' || c == '_' || c == '\'' || c == '’'
    }

    /// Double-click: the word containing `i`.
    pub fn word_at(&self, i: usize) -> Range<usize> {
        if i >= self.len() || !Self::is_word_char(self.chars[i].ch) {
            return i..(i + 1).min(self.len());
        }
        let mut s = i;
        while s > 0 && Self::is_word_char(self.chars[s - 1].ch) {
            s -= 1;
        }
        let mut e = i;
        while e < self.len() && Self::is_word_char(self.chars[e].ch) {
            e += 1;
        }
        s..e
    }

    /// Triple-click: the visual line containing `i` (up to line breaks).
    pub fn line_at(&self, i: usize) -> Range<usize> {
        if i >= self.len() {
            return i..i;
        }
        let is_break = |c: &TextChar| c.ch == '\n' || c.ch == '\r';
        let mut s = i;
        while s > 0 && !is_break(&self.chars[s - 1]) {
            s -= 1;
        }
        let mut e = i;
        while e < self.len() && !is_break(&self.chars[e]) {
            e += 1;
        }
        s..e
    }

    /// The sentence around a range — captured as a new node's body (F-CAP-2).
    pub fn sentence_around(&self, r: Range<usize>) -> String {
        let terminal = |c: char| matches!(c, '.' | '?' | '!');
        let mut s = r.start.min(self.len());
        while s > 0 && !terminal(self.chars[s - 1].ch) && r.start - s < 400 {
            s -= 1;
        }
        let mut e = r.end.min(self.len());
        while e < self.len() && !terminal(self.chars[e].ch) && e - r.end < 400 {
            e += 1;
        }
        if e < self.len() {
            e += 1;
        }
        normalize_ws(&self.slice(s..e))
    }

    /// Characters whose centre lies in a rectangle (region selection).
    pub fn range_in_rect(&self, rect: &Rect) -> Option<Range<usize>> {
        let inside: Vec<usize> = self
            .chars
            .iter()
            .enumerate()
            .filter(|(_, c)| Self::has_box(c) && rect.contains(c.rect.center().0, c.rect.center().1))
            .map(|(i, _)| i)
            .collect();
        Some(*inside.first()?..*inside.last()? + 1)
    }

    /// Merge the boxes of a character range into one rectangle per visual
    /// line — what the reader paints as a highlight.
    pub fn rects(&self, r: Range<usize>) -> Vec<Rect> {
        let mut out: Vec<Rect> = Vec::new();
        for c in
            self.chars[r.start.min(self.len())..r.end.min(self.len())].iter().filter(|c| Self::has_box(c))
        {
            if let Some(last) = out.last_mut() {
                let overlap = (last.y1.min(c.rect.y1) - last.y0.max(c.rect.y0)).max(0.0);
                let same_line = overlap > 0.5 * last.height().min(c.rect.height());
                let adjacent = c.rect.x0 >= last.x0 - 1.0 && c.rect.x0 - last.x1 < 3.0 * c.rect.height();
                if same_line && adjacent {
                    *last = last.union(&c.rect);
                    continue;
                }
            }
            out.push(c.rect);
        }
        out
    }

    pub fn quads(&self, r: Range<usize>) -> Vec<Quad> {
        self.rects(r).iter().map(Rect::to_quad).collect()
    }
}

/// Collapse runs of whitespace (including extractor line breaks) to one space.
pub fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Lay text out as a fake page: 6pt-wide glyphs, 12pt lines, `\n`
    /// generated between lines.
    pub fn fake_page(text: &str) -> PageText {
        let mut chars = Vec::new();
        let (mut x, mut y) = (72.0f32, 72.0f32);
        for ch in text.chars() {
            if ch == '\n' {
                chars.push(TextChar { ch, rect: Rect::new(x, y, x, y), generated: true });
                x = 72.0;
                y += 12.0;
                continue;
            }
            chars.push(TextChar { ch, rect: Rect::new(x, y, x + 6.0, y + 12.0), generated: false });
            x += 6.0;
        }
        PageText { chars }
    }

    #[test]
    fn word_line_and_rects() {
        let p = fake_page("the quadratic variation of\nthe process is defined.");
        let i = p.text().find("variation").unwrap();
        assert_eq!(p.slice(p.word_at(i + 2)), "variation");
        assert_eq!(p.slice(p.line_at(i)), "the quadratic variation of");
        // A selection spanning a line break yields one rect per line.
        let s = p.text().find("of").unwrap();
        let e = p.text().find("process").unwrap() + 7;
        assert_eq!(p.rects(s..e).len(), 2);
        assert_eq!(p.sentence_around(i..i + 9), "the quadratic variation of the process is defined.");
    }

    #[test]
    fn hit_testing() {
        let p = fake_page("abc def");
        assert_eq!(p.char_at(72.0 + 6.0 * 4.0 + 1.0, 80.0), Some(4));
        assert_eq!(p.caret_at(72.0 + 6.0 * 4.0 + 5.0, 80.0), Some(5));
        assert_eq!(p.char_at(10.0, 10.0), None);
        let r = p.range_in_rect(&Rect::new(72.0, 70.0, 72.0 + 18.0, 90.0)).unwrap();
        assert_eq!(p.slice(r), "abc");
    }
}
