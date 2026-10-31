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
    let (text, cursor) = shown(app);
    wrap(&text, cursor, inner_width(width)).0.len().clamp(1, MAX_ROWS)
}

/// The input as drawn, and the cursor in it: an API key shows as dots.
fn shown(app: &App) -> (String, usize) {
    if !app.secret() {
        return (app.input.clone(), app.cursor);
    }
    let dot = '•';
    (dot.to_string().repeat(app.input.chars().count()), app.input[..app.cursor].chars().count() * dot.len_utf8())
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
    let (text, cursor) = shown(app);
    let (rows, (col, row)) = wrap(&text, cursor, inner_width(area.width));
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
    if app.busy && app.approval.is_none() {
        comet(f.buffer_mut(), area, app.started.elapsed().as_millis() as usize);
    }
    if app.approval.is_none() {
        f.set_cursor_position(Position::new(inner.x + col as u16, inner.y + (row - top) as u16));
    }
}

/// One lap around the box, whatever its size.
const LAP_MS: usize = 2400;

/// Border cells clockwise from the top-left corner.
fn perimeter(a: Rect) -> Vec<(u16, u16)> {
    let (x1, y1) = (a.right().saturating_sub(1), a.bottom().saturating_sub(1));
    let top = (a.x..=x1).map(|x| (x, a.y));
    let right = (a.y + 1..=y1).map(|y| (x1, y));
    let bottom = (a.x..x1).rev().map(|x| (x, y1));
    let left = (a.y + 1..y1).rev().map(|y| (a.x, y));
    top.chain(right).chain(bottom).chain(left).collect()
}

/// A light running around the border while a turn is in progress: a bright
/// head and a fading tail over the dim frame.
fn comet(buf: &mut ratatui::buffer::Buffer, area: Rect, ms: usize) {
    let cells = perimeter(area);
    let n = cells.len();
    if n == 0 {
        return;
    }
    let head = ms * n / LAP_MS % n;
    let tail = (n / 6).max(4);
    for (i, &(x, y)) in cells.iter().enumerate() {
        let behind = (head + n - i) % n;
        let style = match behind {
            d if d < 2 => Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            d if d < tail / 2 => Style::new().fg(ACCENT),
            d if d < tail => Style::new().fg(ACCENT).add_modifier(Modifier::DIM),
            _ => continue,
        };
        buf[(x, y)].set_style(style);
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
    fn perimeter_visits_each_border_cell_once() {
        let cells = super::perimeter(ratatui::layout::Rect::new(2, 1, 5, 3));
        let unique: std::collections::BTreeSet<_> = cells.iter().collect();
        assert_eq!((cells.len(), unique.len()), (12, 12));
        assert_eq!((cells[0], cells[4], cells[6], cells[11]), ((2, 1), (6, 1), (6, 3), (2, 2)));
    }

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
