use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Line-by-line markdown styling for streamed text: only the code-fence
/// state carries over between lines, so each line is rendered exactly once.
#[derive(Default)]
pub struct Markdown {
    code: bool,
}

impl Markdown {
    pub fn line(&mut self, raw: &str) -> Line<'static> {
        let t = raw.trim_start();
        if t.starts_with("```") {
            self.code = !self.code;
            return Line::styled(raw.to_string(), Style::new().add_modifier(Modifier::DIM));
        }
        if self.code {
            return Line::styled(raw.to_string(), code());
        }
        if let Some(h) = heading(t) {
            return Line::styled(h.to_string(), Style::new().add_modifier(Modifier::BOLD));
        }
        Line::from(inline(raw))
    }

    pub fn reset(&mut self) {
        self.code = false;
    }
}

fn code() -> Style {
    Style::new().fg(Color::Cyan)
}

fn heading(t: &str) -> Option<&str> {
    let rest = t.trim_start_matches('#');
    (rest.len() < t.len() && rest.starts_with(' ')).then(|| rest.trim_start())
}

/// Splits `code` and **bold** spans; unmatched markers stay literal.
fn inline(s: &str) -> Vec<Span<'static>> {
    let mut spans = vec![];
    let mut rest = s;
    while !rest.is_empty() {
        let next = [rest.find('`').map(|i| (i, "`")), rest.find("**").map(|i| (i, "**"))]
            .into_iter()
            .flatten()
            .min_by_key(|(i, _)| *i);
        let Some((i, mark)) = next else { break };
        let after = &rest[i + mark.len()..];
        let Some(j) = after.find(mark) else { break };
        if i > 0 {
            spans.push(Span::raw(rest[..i].to_string()));
        }
        let style = if mark == "`" { code() } else { Style::new().add_modifier(Modifier::BOLD) };
        spans.push(Span::styled(after[..j].to_string(), style));
        rest = &after[j + mark.len()..];
    }
    if !rest.is_empty() {
        spans.push(Span::raw(rest.to_string()));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(line: &Line) -> Vec<String> {
        line.spans.iter().map(|s| s.content.to_string()).collect()
    }

    #[test]
    fn styles_inline_headings_and_fences() {
        let mut md = Markdown::default();
        assert_eq!(texts(&md.line("a `b` **c** d")), ["a ", "b", " ", "c", " d"]);
        assert_eq!(texts(&md.line("x ** y")), ["x ** y"]);
        assert_eq!(texts(&md.line("## Title")), ["Title"]);
        md.line("```rust");
        assert_eq!(md.line("let **a** = 1;").style.fg, Some(Color::Cyan));
        md.line("```");
        assert_eq!(texts(&md.line("**b**")), ["b"]);
    }
}
