use std::ops::Range;

use similar::TextDiff;

pub fn line_of(text: &str, byte: usize) -> usize {
    text[..byte].matches('\n').count() + 1
}

/// Fallback when exact matching fails: compares whole lines ignoring trailing
/// whitespace. Returns byte ranges in `text`.
pub fn loose_find(text: &str, old: &str) -> Vec<Range<usize>> {
    let want: Vec<&str> = old.lines().map(str::trim_end).collect();
    if want.is_empty() {
        return vec![];
    }
    let keep_eol = old.ends_with('\n');
    let spans = line_spans(text);
    spans
        .windows(want.len())
        .filter(|w| w.iter().zip(&want).all(|(l, o)| text[l.start..l.body_end].trim_end() == *o))
        .map(|w| w[0].start..if keep_eol { w[w.len() - 1].end } else { w[w.len() - 1].body_end })
        .collect()
}

struct Span {
    start: usize,
    body_end: usize,
    end: usize,
}

fn line_spans(text: &str) -> Vec<Span> {
    let mut out = vec![];
    let mut start = 0;
    for seg in text.split_inclusive('\n') {
        let end = start + seg.len();
        let body = seg.trim_end_matches(['\n', '\r']).len();
        out.push(Span { start, body_end: start + body, end });
        start = end;
    }
    out
}

/// Most similar region to `old`, so the model can fix its edit without re-reading.
pub fn closest(text: &str, old: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let n = old.lines().count().max(1);
    if lines.is_empty() || lines.len() > 5000 || old.len() > 4000 {
        return String::new();
    }
    let target = old.trim_end();
    let (mut best, mut at) = (0.0f32, 0);
    for i in 0..=lines.len().saturating_sub(n) {
        let window = lines[i..(i + n).min(lines.len())].join("\n");
        let r = TextDiff::from_chars(window.as_str(), target).ratio();
        if r > best {
            (best, at) = (r, i);
        }
    }
    if best < 0.5 {
        return String::new();
    }
    let shown: String = lines[at..(at + n.min(10)).min(lines.len())]
        .iter()
        .enumerate()
        .map(|(k, l)| format!("{}\t{l}\n", at + k + 1))
        .collect();
    format!("; closest:\n{shown}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loose_ignores_trailing_spaces() {
        let text = "a  \nb\t\nc\n";
        assert_eq!(loose_find(text, "a\nb"), vec![0..6]);
        assert_eq!(loose_find(text, "a\nb\n"), vec![0..7]);
        assert!(loose_find(text, "x").is_empty());
    }

    #[test]
    fn closest_points_to_near_miss() {
        let text = "fn main() {\n    let x = 1;\n}\n";
        assert!(closest(text, "    let x = 2;").contains("2\t    let x = 1;"));
        assert_eq!(closest(text, "zzzzzzzzzzzzzzzzzzzz"), "");
    }
}
