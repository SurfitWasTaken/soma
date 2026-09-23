//! Anchor capture and re-anchoring (PRD §4.2).
//!
//! Resolution order when a document's bytes changed since capture:
//! 1. stored char offsets on the same page, verified against `exact`;
//! 2. search for `exact` (scored by `prefix`/`suffix`) on the same page,
//!    then ±2 pages, then the whole document — exact first, fuzzy second;
//! 3. fall back to stored geometry on the same page (confidence 0.3);
//! 4. otherwise the anchor is detached. The entity is never lost.

use crate::text::{PageText, normalize_ws};
use soma_core::*;
use std::ops::Range;

pub const CONTEXT_CHARS: usize = 48;
pub const GEOMETRY_CONFIDENCE: f32 = 0.3;
const FUZZY_THRESHOLD: f64 = 0.8;
const FUZZY_MAX_LEN: usize = 160;

/// Build a text anchor from a selection. `id`/`entity` are filled in by the
/// command that files it.
pub fn capture_text(
    doc: &DocumentId,
    page_index: u32,
    text: &PageText,
    range: Range<usize>,
    color: Option<Color>,
) -> Anchor {
    let len = text.len();
    let r = range.start.min(len)..range.end.min(len);
    let prefix = text.slice(r.start.saturating_sub(CONTEXT_CHARS * 2)..r.start);
    let suffix = text.slice(r.end..(r.end + CONTEXT_CHARS * 2).min(len));
    Anchor {
        id: AnchorId::from(""),
        entity: EntityId::from(""),
        document: doc.clone(),
        page_index,
        kind: AnchorKind::Text,
        quads: text.quads(r.clone()),
        exact: normalize_ws(&text.slice(r.clone())),
        prefix: tail(&normalize_ws(&prefix), CONTEXT_CHARS),
        suffix: head(&normalize_ws(&suffix), CONTEXT_CHARS),
        char_start: Some(r.start as u32),
        char_end: Some(r.end as u32),
        confidence: 1.0,
        color,
    }
}

/// A rectangle over a figure/equation, or a clicked object. `text` is
/// whatever text lies inside, kept for display and search.
pub fn capture_region(
    doc: &DocumentId,
    page_index: u32,
    kind: AnchorKind,
    rect: crate::text::Rect,
    text: String,
    color: Option<Color>,
) -> Anchor {
    Anchor {
        id: AnchorId::from(""),
        entity: EntityId::from(""),
        document: doc.clone(),
        page_index,
        kind,
        quads: vec![rect.to_quad()],
        exact: normalize_ws(&text),
        prefix: String::new(),
        suffix: String::new(),
        char_start: None,
        char_end: None,
        confidence: 1.0,
        color,
    }
}

fn tail(s: &str, n: usize) -> String {
    let count = s.chars().count();
    s.chars().skip(count.saturating_sub(n)).collect()
}

fn head(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub page_index: u32,
    pub char_range: Option<Range<usize>>,
    pub quads: Vec<Quad>,
    pub confidence: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Resolution {
    Found(Resolved),
    /// Stored geometry reused without textual confirmation; flagged in UI.
    Geometry(Resolved),
    Detached,
}

impl Resolution {
    pub fn confidence(&self) -> f32 {
        match self {
            Resolution::Found(r) | Resolution::Geometry(r) => r.confidence,
            Resolution::Detached => 0.0,
        }
    }

    /// Apply to an anchor record (for an `UpdateAnchor` op).
    pub fn apply_to(&self, anchor: &Anchor, new_doc: &DocumentId) -> Anchor {
        let mut a = anchor.clone();
        a.document = new_doc.clone();
        match self {
            Resolution::Found(r) | Resolution::Geometry(r) => {
                a.page_index = r.page_index;
                a.quads = r.quads.clone();
                if let Some(cr) = &r.char_range {
                    a.char_start = Some(cr.start as u32);
                    a.char_end = Some(cr.end as u32);
                }
                a.confidence = r.confidence;
            }
            Resolution::Detached => a.confidence = 0.0,
        }
        a
    }
}

/// Text normalized for matching (lowercase, ligatures expanded, whitespace
/// and soft hyphens dropped) plus a map back to character offsets.
struct Normalized {
    chars: Vec<char>,
    map: Vec<usize>,
}

fn normalize(chars: impl Iterator<Item = char>) -> Normalized {
    let mut out = Normalized { chars: Vec::new(), map: Vec::new() };
    for (i, c) in chars.enumerate() {
        if c.is_whitespace() || c.is_control() || c == '\u{ad}' || c == '\u{fffe}' {
            continue;
        }
        let expanded: &[char] = match c {
            'ﬁ' => &['f', 'i'],
            'ﬂ' => &['f', 'l'],
            'ﬀ' => &['f', 'f'],
            'ﬃ' => &['f', 'f', 'i'],
            'ﬄ' => &['f', 'f', 'l'],
            'ﬅ' | 'ﬆ' => &['s', 't'],
            '‘' | '’' => &['\''],
            '“' | '”' => &['"'],
            '‐' | '‑' | '‒' | '–' | '—' => &['-'],
            _ => &[],
        };
        if expanded.is_empty() {
            for l in c.to_lowercase() {
                out.chars.push(l);
                out.map.push(i);
            }
        } else {
            for &e in expanded {
                out.chars.push(e);
                out.map.push(i);
            }
        }
    }
    out
}

fn norm_str(s: &str) -> Vec<char> {
    normalize(s.chars()).chars
}

fn find_all(hay: &[char], needle: &[char]) -> Vec<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return vec![];
    }
    (0..=hay.len() - needle.len()).filter(|&i| hay[i..i + needle.len()] == *needle).collect()
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn distance_ratio(a: &[char], b: &[char]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    levenshtein(a, b) as f64 / a.len().max(b.len()) as f64
}

fn sim(a: &[char], b: &[char]) -> f64 {
    1.0 - distance_ratio(a, b)
}

/// How well the text around a normalized match agrees with stored context.
fn context_score(n: &Normalized, start: usize, end: usize, prefix: &[char], suffix: &[char]) -> f64 {
    let before = &n.chars[start.saturating_sub(prefix.len())..start];
    let after = &n.chars[end..(end + suffix.len()).min(n.chars.len())];
    let p = if prefix.is_empty() { 1.0 } else { sim(prefix, before) };
    let s = if suffix.is_empty() { 1.0 } else { sim(suffix, after) };
    (p + s) / 2.0
}

fn to_range(n: &Normalized, start: usize, end: usize) -> Range<usize> {
    n.map[start]..n.map[end - 1] + 1
}

fn search_page(
    text: &PageText,
    exact: &[char],
    prefix: &[char],
    suffix: &[char],
    fuzzy: bool,
) -> Option<(Range<usize>, f32)> {
    let n = normalize(text.chars.iter().map(|c| c.ch));
    let best_exact = find_all(&n.chars, exact)
        .into_iter()
        .map(|i| (i, context_score(&n, i, i + exact.len(), prefix, suffix)))
        .max_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((i, ctx)) = best_exact {
        return Some((to_range(&n, i, i + exact.len()), (0.9 + 0.1 * ctx) as f32));
    }
    if !fuzzy || exact.is_empty() {
        return None;
    }
    // Fuzzy: slide a window of the needle's length (capped) over the page.
    let needle = &exact[..exact.len().min(FUZZY_MAX_LEN)];
    let w = needle.len();
    if w > n.chars.len() {
        return None;
    }
    let mut best: Option<(usize, f64)> = None;
    for i in 0..=n.chars.len() - w {
        // Cheap pre-filter: first or last char must agree.
        if n.chars[i] != needle[0] && n.chars[i + w - 1] != needle[w - 1] {
            continue;
        }
        let s = sim(needle, &n.chars[i..i + w]);
        if s >= FUZZY_THRESHOLD && best.is_none_or(|(_, b)| s > b) {
            best = Some((i, s));
        }
    }
    let (i, s) = best?;
    let end = (i + exact.len()).min(n.chars.len());
    let ctx = context_score(&n, i, end, prefix, suffix);
    Some((to_range(&n, i, end), (s * (0.85 + 0.05 * ctx)) as f32))
}

/// Re-anchor against a (possibly different) document.
///
/// `page_text(i)` returns the extracted text of page `i`; it is called
/// lazily so resolving an anchor on page 84 of a 400-page book usually
/// touches only page 84.
pub fn resolve(
    anchor: &Anchor,
    page_count: u32,
    page_text: &mut dyn FnMut(u32) -> Option<PageText>,
) -> Resolution {
    let same_page_exists = anchor.page_index < page_count;
    if anchor.kind == AnchorKind::Text && !anchor.exact.is_empty() {
        let exact = norm_str(&anchor.exact);
        let prefix = norm_str(&anchor.prefix);
        let suffix = norm_str(&anchor.suffix);
        let mut cache: std::collections::HashMap<u32, Option<PageText>> = Default::default();
        let mut get = |i: u32| cache.entry(i).or_insert_with(|| page_text(i)).clone();

        // 1. Stored offsets on the same page.
        if same_page_exists
            && let (Some(s), Some(e)) = (anchor.char_start, anchor.char_end)
            && let Some(t) = get(anchor.page_index)
            && (e as usize) <= t.len()
            && norm_str(&t.slice(s as usize..e as usize)) == exact
        {
            let r = s as usize..e as usize;
            return Resolution::Found(Resolved {
                page_index: anchor.page_index,
                quads: t.quads(r.clone()),
                char_range: Some(r),
                confidence: 1.0,
            });
        }

        // 2. Search in widening tiers.
        let p = anchor.page_index as i64;
        let near: Vec<u32> = [p - 1, p + 1, p - 2, p + 2]
            .into_iter()
            .filter(|&i| i >= 0 && i < page_count as i64)
            .map(|i| i as u32)
            .collect();
        let rest: Vec<u32> = (0..page_count).filter(|i| (*i as i64 - p).abs() > 2).collect();
        let same: Vec<u32> = if same_page_exists { vec![anchor.page_index] } else { vec![] };
        for tier in [same, near, rest] {
            for fuzzy in [false, true] {
                let mut best: Option<(u32, Range<usize>, f32, PageText)> = None;
                for &page in &tier {
                    let Some(t) = get(page) else { continue };
                    if let Some((r, c)) = search_page(&t, &exact, &prefix, &suffix, fuzzy)
                        && best.as_ref().is_none_or(|b| c > b.2)
                    {
                        best = Some((page, r, c, t));
                    }
                }
                if let Some((page, r, c, t)) = best {
                    return Resolution::Found(Resolved {
                        page_index: page,
                        quads: t.quads(r.clone()),
                        char_range: Some(r),
                        confidence: c,
                    });
                }
            }
        }
    }

    // 3. Geometry on the same page. Region/object anchors come straight here.
    if same_page_exists && !anchor.quads.is_empty() {
        let confidence = if anchor.kind == AnchorKind::Text { GEOMETRY_CONFIDENCE } else { 0.5 };
        return Resolution::Geometry(Resolved {
            page_index: anchor.page_index,
            char_range: None,
            quads: anchor.quads.clone(),
            confidence,
        });
    }
    Resolution::Detached
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::tests::fake_page;

    const V1: &str = "Let X be a continuous semimartingale.\nThe quadratic variation of X is defined as the limit\nof sums of squared increments over partitions.";

    fn cap(page: &PageText, needle: &str) -> Anchor {
        let s = page.text().find(needle).unwrap();
        let s = page.text()[..s].chars().count();
        capture_text(&DocumentId::from("v1"), 0, page, s..s + needle.chars().count(), None)
    }

    #[test]
    fn same_offsets_resolve_at_full_confidence() {
        let p = fake_page(V1);
        let a = cap(&p, "quadratic variation");
        assert_eq!(a.exact, "quadratic variation");
        assert!(a.prefix.ends_with("The"));
        let r = resolve(&a, 1, &mut |_| Some(p.clone()));
        assert_eq!(r.confidence(), 1.0);
    }

    #[test]
    fn shifted_text_and_moved_page_still_resolve_highly() {
        let a = cap(&fake_page(V1), "quadratic variation");
        // Revised edition: text inserted before, reflowed, moved to page 2,
        // and a ligature introduced.
        let v2 = format!("Preface.\n\nChapter 3.\n{}", V1.replace("defined", "deﬁned").replace('\n', " "));
        let pages = [fake_page("front matter"), fake_page("more"), fake_page(&v2)];
        let r = resolve(&a, 3, &mut |i| pages.get(i as usize).cloned());
        let Resolution::Found(found) = r else { panic!("{r:?}") };
        assert_eq!(found.page_index, 2);
        assert!(found.confidence >= 0.9, "{}", found.confidence);
        assert_eq!(pages[2].slice(found.char_range.unwrap()), "quadratic variation");
    }

    #[test]
    fn typo_fix_resolves_fuzzily() {
        let a = cap(&fake_page(V1), "sums of squared increments over partitions");
        let v2 = V1.replace("squared increments", "squared incremnts");
        let r = resolve(&a, 1, &mut |_| Some(fake_page(&v2)));
        assert!(matches!(r, Resolution::Found(_)), "{r:?}");
        assert!(r.confidence() > 0.7 && r.confidence() < 0.9);
    }

    #[test]
    fn missing_text_falls_back_to_geometry_then_detaches() {
        let a = cap(&fake_page(V1), "quadratic variation");
        let r = resolve(&a, 1, &mut |_| Some(fake_page("entirely different content")));
        assert!(matches!(r, Resolution::Geometry(_)));
        assert_eq!(r.confidence(), GEOMETRY_CONFIDENCE);
        let mut far = a.clone();
        far.page_index = 7;
        assert_eq!(resolve(&far, 1, &mut |_| Some(fake_page("nothing"))), Resolution::Detached);
    }

    #[test]
    fn disambiguates_repeated_text_by_context() {
        let text = "the limit exists here. but the limit exists there too.";
        let p = fake_page(text);
        let second = text.rfind("the limit").unwrap();
        let a = capture_text(&DocumentId::from("d"), 0, &p, second..second + 9, None);
        let shifted = fake_page(&format!("xx {text}"));
        let r = resolve(&a, 1, &mut |_| Some(shifted.clone()));
        let Resolution::Found(f) = r else { panic!() };
        assert_eq!(f.char_range.unwrap().start, second + 3);
    }
}
