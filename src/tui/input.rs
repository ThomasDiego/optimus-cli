use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Paragraph};

use super::app::App;
use crate::command::Suggestion;

/// The box grows with the message up to this many rows, then scrolls.
pub const MAX_ROWS: usize = 8;
pub const MAX_SUGGESTIONS: usize = 5;
const PROMPT: &str = "› ";
/// Border (2) + horizontal padding (2).
const CHROME: u16 = 4;

pub const ACCENT: Color = Color::Magenta;

/// Rows the input needs at this terminal width.
pub fn rows(app: &App, width: u16) -> usize {
    wrap(&app.input, app.cursor, inner_width(width)).0.len().clamp(1, MAX_ROWS)
}

fn inner_width(width: u16) -> usize {
    width.saturating_sub(CHROME).max(4) as usize
}

pub fn render_box(f: &mut Frame, app: &App, area: Rect) {
    let border = if app.approval.is_some() {
        Style::new().fg(Color::Yellow)
    } else if app.busy {
        Style::new().fg(Color::DarkGray)
    } else {
        Style::new().fg(ACCENT)
    };
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(border).padding(Padding::horizontal(1));
    let inner = block.inner(area);
    let (rows, (col, row)) = wrap(&app.input, app.cursor, inner_width(area.width));
    let top = row.saturating_sub(inner.height.max(1) as usize - 1);
    let text = if app.busy { Style::new().add_modifier(Modifier::DIM) } else { Style::new() };
    let lines: Vec<Line> = rows
        .into_iter()
        .enumerate()
        .skip(top)
        .map(|(i, r)| match r.strip_prefix(PROMPT).filter(|_| i == 0) {
            Some(rest) => Line::from(vec![Span::styled(PROMPT, Style::new().fg(ACCENT)), Span::styled(rest.to_string(), text)]),
            None => Line::styled(r, text),
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(block), area);
    if app.approval.is_none() {
        f.set_cursor_position(Position::new(inner.x + col as u16, inner.y + (row - top) as u16));
    }
}

pub fn render_suggestions(f: &mut Frame, sugg: &[Suggestion], selected: usize, area: Rect) {
    let width = sugg.iter().map(|s| s.text.chars().count()).max().unwrap_or(0) + 2;
    let lines: Vec<Line> = sugg
        .iter()
        .take(area.height as usize)
        .enumerate()
        .map(|(i, s)| {
            let (marker, name) = if i == selected.min(sugg.len() - 1) {
                ("› ", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))
            } else {
                ("  ", Style::new())
            };
            Line::from(vec![
                Span::styled(format!("  {marker}"), Style::new().fg(ACCENT)),
                Span::styled(format!("{:<width$}", s.text), name),
                Span::styled(s.desc, Style::new().add_modifier(Modifier::DIM)),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

/// A visual row; `indent` chars (prompt or newline indent) never wrap away.
struct Row {
    chars: Vec<char>,
    indent: usize,
}

/// Word-wraps the prompt and input into rows of `width` chars; returns rows
/// and the cursor as (col, row). Assumes single-width characters.
fn wrap(input: &str, cursor: usize, width: usize) -> (Vec<String>, (usize, usize)) {
    let mut rows = vec![Row { chars: PROMPT.chars().collect(), indent: PROMPT.chars().count() }];
    let mut cur = None;
    for (i, c) in input.char_indices() {
        if c == '\n' {
            if i == cursor {
                cur = Some((rows.last().unwrap().chars.len(), rows.len() - 1));
            }
            rows.push(Row { chars: vec![' ', ' '], indent: 2 });
            continue;
        }
        if rows.last().unwrap().chars.len() >= width {
            let row = rows.len() - 1;
            let last = rows.last_mut().unwrap();
            // Move the word being typed to the next row, if a space allows it.
            let split = last.chars.iter().rposition(|&ch| ch == ' ').filter(|&p| p >= last.indent && p + 1 < last.chars.len());
            let tail = split.map(|p| last.chars.split_off(p + 1)).unwrap_or_default();
            if let (Some(p), Some((col, r))) = (split, cur)
                && r == row
                && col > p
            {
                cur = Some((col - p - 1, row + 1));
            }
            rows.push(Row { chars: tail, indent: 0 });
        }
        if i == cursor {
            cur = Some((rows.last().unwrap().chars.len(), rows.len() - 1));
        }
        rows.last_mut().unwrap().chars.push(c);
    }
    let cur = cur.unwrap_or_else(|| {
        if rows.last().unwrap().chars.len() >= width {
            rows.push(Row { chars: vec![], indent: 0 });
        }
        (rows.last().unwrap().chars.len(), rows.len() - 1)
    });
    (rows.into_iter().map(|r| r.chars.into_iter().collect()).collect(), cur)
}

#[cfg(test)]
mod tests {
    use super::wrap;

    #[test]
    fn wraps_and_places_cursor() {
        assert_eq!(wrap("", 0, 10), (vec!["› ".into()], (2, 0)));
        assert_eq!(wrap("abcdefghij", 10, 6), (vec!["› abcd".into(), "efghij".into(), "".into()], (0, 2)));
        assert_eq!(wrap("ab\ncd", 3, 10), (vec!["› ab".into(), "  cd".into()], (2, 1)));
        assert_eq!(wrap("héllo", 1, 10).1, (3, 0));
    }

    #[test]
    fn wraps_between_words() {
        assert_eq!(wrap("hello world foo", 15, 10), (vec!["› hello ".into(), "world foo".into()], (9, 1)));
        // Cursor inside the moved word follows it.
        assert_eq!(wrap("hello world foo", 7, 10).1, (1, 1));
    }
}
