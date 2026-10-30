use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget, Wrap};

/// Chat history with virtualized rendering: each line's wrapped height is
/// cached per width, and a frame only lays out the lines on screen, so the
/// cost of a frame does not grow with the conversation.
#[derive(Default)]
pub struct Transcript {
    lines: Vec<Line<'static>>,
    heights: Vec<u16>,
    width: u16,
    total: usize,
    /// Rows scrolled up from the bottom; 0 follows new content.
    offset: usize,
    /// Height of the last rendered area, for page scrolling.
    view: u16,
}

fn height_of(line: &Line<'static>, width: u16) -> u16 {
    if width == 0 {
        return 1;
    }
    Paragraph::new(line.clone()).wrap(Wrap { trim: false }).line_count(width).clamp(1, u16::MAX as usize) as u16
}

impl Transcript {
    pub fn push(&mut self, line: Line<'static>) {
        let h = height_of(&line, self.width);
        // While the user reads older lines, new ones must not move the view.
        if self.offset > 0 {
            self.offset += h as usize;
        }
        self.total += h as usize;
        self.heights.push(h);
        self.lines.push(line);
    }

    /// Positive scrolls towards older lines.
    pub fn scroll(&mut self, rows: isize) {
        self.offset = self.offset.saturating_add_signed(rows).min(self.total);
    }

    pub fn page(&mut self, up: bool) {
        let rows = (self.view / 2).max(1) as isize;
        self.scroll(if up { rows } else { -rows });
    }

    pub fn follow(&mut self) {
        self.offset = 0;
    }

    /// Re-measures only when the width changes (terminal resize).
    fn set_width(&mut self, width: u16) {
        if width == self.width {
            return;
        }
        self.width = width;
        self.heights = self.lines.iter().map(|l| height_of(l, width)).collect();
        self.total = self.heights.iter().map(|&h| h as usize).sum();
    }

    /// Renders the visible window; `tail` (the line being streamed) is drawn
    /// after the stored lines without being stored.
    pub fn render(&mut self, area: Rect, buf: &mut Buffer, tail: Option<&Line<'static>>) {
        self.set_width(area.width);
        self.view = area.height;
        let tail = tail.map(|l| (l, height_of(l, area.width)));
        let total = self.total + tail.map_or(0, |(_, h)| h as usize);
        let view = area.height as usize;
        self.offset = self.offset.min(total.saturating_sub(view));
        let bottom = total - self.offset;
        let top = bottom.saturating_sub(view);

        let stored = self.lines.iter().zip(self.heights.iter().copied()).rev();
        let mut end = total;
        for (line, h) in tail.into_iter().chain(stored) {
            let start = end - h as usize;
            if end <= top {
                break;
            }
            if start < bottom {
                let (from, to) = (start.max(top), end.min(bottom));
                let rect = Rect { x: area.x, y: area.y + (from - top) as u16, width: area.width, height: (to - from) as u16 };
                Paragraph::new(line.clone())
                    .wrap(Wrap { trim: false })
                    .scroll(((from - start) as u16, 0))
                    .render(rect, buf);
            }
            end = start;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(t: &mut Transcript, w: u16, h: u16, tail: Option<&Line<'static>>) -> Vec<String> {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        t.render(area, &mut buf, tail);
        (0..h).map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>().trim_end().to_string()).collect()
    }

    #[test]
    fn follows_bottom_scrolls_and_wraps() {
        let mut t = Transcript::default();
        for i in 1..=10 {
            t.push(Line::raw(format!("line {i}")));
        }
        assert_eq!(screen(&mut t, 10, 3, None), ["line 8", "line 9", "line 10"]);
        t.scroll(2);
        assert_eq!(screen(&mut t, 10, 3, None), ["line 6", "line 7", "line 8"]);
        // New content keeps the scrolled view still.
        t.push(Line::raw("line 11"));
        assert_eq!(screen(&mut t, 10, 3, None), ["line 6", "line 7", "line 8"]);
        t.follow();
        let tail = Line::raw("streaming");
        assert_eq!(screen(&mut t, 10, 3, Some(&tail)), ["line 10", "line 11", "streaming"]);
        // A wrapped line is partially visible at the top edge.
        t.push(Line::raw("aaaa bbbb cccc"));
        assert_eq!(screen(&mut t, 8, 2, None), ["bbbb", "cccc"]);
        t.scroll(1000);
        assert_eq!(screen(&mut t, 8, 2, None), ["line 1", "line 2"]);
    }
}
