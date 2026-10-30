use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

/// Columns of the gutter (`› `, `● `, blanks) left out of selections.
const GUTTER: u16 = 2;

/// A mouse selection over the history, in screen cells. The text is read
/// back from the rendered buffer, so copies are exactly what is on screen.
#[derive(Clone, Copy)]
pub struct Selection {
    anchor: (u16, u16),
    cursor: (u16, u16),
}

pub fn inside(area: Rect, x: u16, y: u16) -> bool {
    x >= area.x && x < area.right() && y >= area.y && y < area.bottom()
}

impl Selection {
    pub fn new(x: u16, y: u16) -> Self {
        Self { anchor: (x, y), cursor: (x, y) }
    }

    pub fn drag(&mut self, x: u16, y: u16) {
        self.cursor = (x, y);
    }

    /// A click without a drag selects nothing.
    pub fn is_empty(&self) -> bool {
        self.anchor == self.cursor
    }

    /// Rows covered inside `area` as `(y, first x, last x)`, in reading order.
    /// Pointers past the area snap to its edge, so dragging below or above the
    /// history keeps extending the selection.
    fn rows(&self, area: Rect) -> Vec<(u16, u16, u16)> {
        if area.height == 0 || area.width <= GUTTER {
            return vec![];
        }
        let (left, right, bottom) = (area.x + GUTTER, area.right() - 1, area.bottom() - 1);
        let snap = |(x, y): (u16, u16)| match y {
            y if y < area.y => (left, area.y),
            y if y > bottom => (right, bottom),
            y => (x.clamp(left, right), y),
        };
        let (a, b) = (snap(self.anchor), snap(self.cursor));
        let ((sx, sy), (ex, ey)) = if (a.1, a.0) <= (b.1, b.0) { (a, b) } else { (b, a) };
        (sy..=ey).map(|y| (y, if y == sy { sx } else { left }, if y == ey { ex } else { right })).collect()
    }

    pub fn highlight(&self, buf: &mut Buffer, area: Rect) {
        for (y, from, to) in self.rows(area) {
            buf.set_style(Rect::new(from, y, to - from + 1, 1), Style::new().add_modifier(Modifier::REVERSED));
        }
    }

    pub fn text(&self, buf: &Buffer, area: Rect) -> String {
        let rows: Vec<String> = self
            .rows(area)
            .into_iter()
            .map(|(y, from, to)| {
                let mut row = String::new();
                let mut skip = 0;
                for x in from..=to {
                    // The cell after a wide character is a filler, not a space.
                    if skip > 0 {
                        skip -= 1;
                        continue;
                    }
                    let symbol = buf[(x, y)].symbol();
                    skip = ratatui::text::Span::raw(symbol).width().saturating_sub(1);
                    row.push_str(symbol);
                }
                row.trim_end().to_string()
            })
            .collect();
        rows.join("\n").trim_matches('\n').to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(rows: &[&str]) -> (Buffer, Rect) {
        let area = Rect::new(0, 0, 20, rows.len() as u16);
        let mut buf = Buffer::empty(area);
        for (y, row) in rows.iter().enumerate() {
            buf.set_string(0, y as u16, row, Style::new());
        }
        (buf, area)
    }

    #[test]
    fn copies_the_text_between_two_points_without_the_gutter() {
        let (buf, area) = screen(&["● hello world", "  second line", "  third line"]);
        let mut sel = Selection::new(8, 0);
        assert!(sel.is_empty());
        sel.drag(8, 2);
        assert!(!sel.is_empty());
        assert_eq!(sel.text(&buf, area), "world\nsecond line\nthird l");
        // Dragged backwards, and into the gutter: same reading order, gutter skipped.
        let mut back = Selection::new(4, 1);
        back.drag(0, 0);
        assert_eq!(back.text(&buf, area), "hello world\nsec");
    }

    #[test]
    fn dragging_past_the_history_snaps_to_its_edges() {
        let (buf, area) = screen(&["  a b", "  c d"]);
        let mut sel = Selection::new(4, 0);
        sel.drag(40, 9);
        assert_eq!(sel.text(&buf, area), "b\nc d");
        sel.drag(0, 0);
        sel.anchor = (5, 1);
        assert_eq!(sel.text(&buf, area), "a b\nc d");
    }

    #[test]
    fn wide_characters_are_copied_once_and_highlight_reverses_cells() {
        let (mut buf, area) = screen(&["  日本 ok"]);
        let mut sel = Selection::new(2, 0);
        sel.drag(11, 0);
        assert_eq!(sel.text(&buf, area), "日本 ok");
        sel.highlight(&mut buf, area);
        assert!(buf[(2, 0)].modifier.contains(Modifier::REVERSED) && !buf[(0, 0)].modifier.contains(Modifier::REVERSED));
        assert!(inside(area, 19, 0) && !inside(area, 20, 0) && !inside(area, 0, 1));
    }
}
