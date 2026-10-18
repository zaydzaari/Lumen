//! Right-to-left text (Arabic, Hebrew, Persian…) in egui.
//!
//! egui shapes text per font run and lays runs out left to right. A space
//! between two Arabic words comes from the Latin font, so every Arabic word
//! becomes its own run and multi-word Arabic titles appear in reverse word
//! order; mixed Arabic/Latin titles get scrambled the same way. Before layout,
//! this module applies the Unicode bidi algorithm per line and rewrites each
//! right-to-left run with its words (and spaces) in visual order. Characters
//! inside a word stay in logical order: the shaper draws each word right to
//! left and joins its letters itself.

use std::borrow::Cow;

use unicode_bidi::BidiInfo;

/// Quick check so pure left-to-right text costs nothing.
pub fn has_rtl(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(c as u32,
            0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF)
    })
}

fn mirror(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '«' => '»',
        '»' => '«',
        _ => c,
    }
}

/// Reorder one line of text into visual order (see module docs).
pub fn visual_line(s: &str) -> Cow<'_, str> {
    if !has_rtl(s) {
        return Cow::Borrowed(s);
    }
    let info = BidiInfo::new(s, None);
    let Some(para) = info.paragraphs.first() else {
        return Cow::Borrowed(s);
    };
    let line = para.range.clone();
    let (levels, runs) = info.visual_runs(para, line);
    let mut out = String::with_capacity(s.len());
    for run in runs {
        let text = &s[run.clone()];
        if levels[run.start].is_rtl() {
            // Tokens = words and whitespace; reverse their order.
            let mut tokens: Vec<String> = Vec::new();
            let mut cur = String::new();
            let mut cur_space: Option<bool> = None;
            for c in text.chars() {
                let sp = c.is_whitespace();
                if cur_space.is_some_and(|w| w != sp) {
                    tokens.push(std::mem::take(&mut cur));
                }
                cur_space = Some(sp);
                cur.push(mirror(c));
            }
            if !cur.is_empty() {
                tokens.push(cur);
            }
            for t in tokens.iter().rev() {
                out.push_str(t);
            }
        } else {
            out.push_str(text);
        }
    }
    Cow::Owned(out)
}

/// Break `s` into lines no wider than `max_w` (measured by `width`), in
/// logical order, then put each line into visual order.
pub fn wrap_visual(s: &str, max_w: f32, width: impl Fn(&str) -> f32) -> String {
    let mut lines: Vec<String> = Vec::new();
    for para in s.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let candidate = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if !line.is_empty() && width(&visual_line(&candidate)) > max_w {
                lines.push(std::mem::take(&mut line));
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        lines.push(line);
    }
    lines
        .iter()
        .map(|l| visual_line(l).into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin_untouched() {
        assert!(matches!(visual_line("Hello world"), Cow::Borrowed(_)));
    }

    #[test]
    fn arabic_words_reversed_letters_kept() {
        // "Cheb Hasni" in Arabic: two words; visual order puts the second first.
        let s = "الشاب حسني";
        assert_eq!(visual_line(s), "حسني الشاب");
    }

    #[test]
    fn mixed_title_orders_runs() {
        // Latin paragraph with an Arabic phrase: the phrase's words flip,
        // the Latin parts keep their place.
        let s = "Baida - بيضاء مون امور (Live)";
        let v = visual_line(s);
        assert!(v.starts_with("Baida - "));
        assert!(v.contains("امور مون بيضاء"));
        assert!(v.ends_with("(Live)"));
    }

    #[test]
    fn wraps_before_reordering() {
        let s = "واحد اثنان ثلاثة اربعة";
        let out = wrap_visual(s, 10.0, |t| t.chars().count() as f32);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "اثنان واحد");
    }
}
