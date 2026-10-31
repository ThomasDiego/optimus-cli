use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use tokio::sync::oneshot;

use super::input::ACCENT;
use super::markdown::Markdown;
use super::select::{Selection, inside};
use super::transcript::Transcript;
use crate::command::{self, Suggestion};
use crate::message::Usage;
use crate::permission::Mode;
use crate::agent::DENIED;
use crate::setup::Setup;
use crate::tools::truncate;
use crate::ui::{Approval, Event, brief, stat, summary};

const DIFF_LINES: usize = 60;
/// A second Ctrl+C within this window quits; one alone never does.
const QUIT_WINDOW: Duration = Duration::from_millis(1500);
const THINKING: &str = "Thinking";
/// Gutter width: text of every speaker starts in the same column.
const INDENT: &str = "  ";

pub enum Action {
    None,
    Submit(String),
    Cancel,
    Quit,
}

pub struct App {
    pub input: String,
    /// Byte index into `input`, always on a char boundary.
    pub cursor: usize,
    history: Vec<String>,
    browsing: Option<usize>,
    /// Highlighted `/command` suggestion.
    pub selected: usize,
    /// Received text not yet revealed: smooths bursty token delivery.
    pending: String,
    /// Revealed text after the last newline, shown live until complete.
    pub partial: String,
    md: Markdown,
    /// Finished lines of the conversation.
    pub out: Transcript,
    pub busy: bool,
    /// What the agent is doing now, animated in the live row.
    pub activity: String,
    /// Lines added/removed by an unprompted write, shown on its result line.
    stat: Option<(usize, usize)>,
    /// Characters received in the current request, until its usage arrives.
    pub streamed: usize,
    /// The model's current text block already has its bullet.
    speaking: bool,
    pub started: Instant,
    pub approval: Option<(String, oneshot::Sender<bool>)>,
    pub usage: Usage,
    /// Tokens of the latest request: the current conversation size.
    pub context: u64,
    pub mode: Rc<Cell<Mode>>,
    pub model: String,
    /// Background jobs still running, refreshed between turns.
    pub jobs: usize,
    /// Set by Ctrl+C: another one before this instant quits.
    pub quit_until: Option<Instant>,
    /// Where the history was last drawn: the only place a selection starts.
    pub view_area: Rect,
    /// Text selected with the mouse; kept highlighted after the copy.
    pub selection: Option<Selection>,
    /// The button is still down: the selection follows the pointer.
    held: bool,
    /// Released: the next render reads the selection into `copied`.
    pub copy_pending: bool,
    /// Text for the clipboard, taken by the draw loop.
    pub copied: Option<String>,
    /// Guided configuration in progress: the input answers it.
    pub setup: Option<Setup>,
}

fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

impl App {
    pub fn new(model: String, mode: Rc<Cell<Mode>>) -> Self {
        Self {
            input: String::new(),
            cursor: 0,
            history: vec![],
            browsing: None,
            selected: 0,
            pending: String::new(),
            partial: String::new(),
            md: Markdown::default(),
            out: Transcript::default(),
            busy: false,
            activity: String::new(),
            stat: None,
            streamed: 0,
            speaking: false,
            started: Instant::now(),
            approval: None,
            usage: Usage::default(),
            context: 0,
            mode,
            model,
            jobs: 0,
            quit_until: None,
            view_area: Rect::default(),
            selection: None,
            held: false,
            copy_pending: false,
            copied: None,
            setup: None,
        }
    }

    /// The input is an API key: shown masked, kept out of the history.
    pub fn secret(&self) -> bool {
        self.setup.as_ref().is_some_and(Setup::secret)
    }

    /// Wheel scrolls; dragging with the left button selects, and releasing
    /// copies. Scrolling drops the selection: the text moves under it.
    fn on_mouse(&mut self, m: MouseEvent) {
        match m.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let rows = if m.kind == MouseEventKind::ScrollUp { 3 } else { -3 };
                self.out.scroll(rows);
                (self.selection, self.held) = (None, false);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.held = inside(self.view_area, m.column, m.row);
                self.selection = self.held.then(|| Selection::new(m.column, m.row));
            }
            MouseEventKind::Drag(MouseButton::Left) if self.held => {
                if let Some(sel) = &mut self.selection {
                    sel.drag(m.column, m.row);
                }
            }
            MouseEventKind::Up(MouseButton::Left) if self.held => {
                self.held = false;
                match self.selection.filter(|s| !s.is_empty()) {
                    Some(_) => self.copy_pending = true,
                    None => self.selection = None,
                }
            }
            _ => {}
        }
    }

    /// True on the second Ctrl+C within QUIT_WINDOW; the first one arms it.
    fn ctrl_c(&mut self) -> bool {
        let now = Instant::now();
        if self.quit_until.is_some_and(|t| now < t) {
            return true;
        }
        self.quit_until = Some(now + QUIT_WINDOW);
        false
    }

    /// The window is open: the status line says a second Ctrl+C quits.
    pub fn quit_armed(&self) -> bool {
        self.quit_until.is_some_and(|t| Instant::now() < t)
    }

    pub fn on_agent(&mut self, ev: Event) {
        match ev {
            Event::Text(t) => {
                self.streamed += t.len();
                return self.pending.push_str(&t);
            }
            // Shows what the model is writing before the call completes.
            Event::Drafting { label, added } => {
                self.streamed += added;
                return self.activity = label;
            }
            Event::Usage(u) => {
                self.context = u.input + u.output;
                self.streamed = 0;
                return self.usage += u;
            }
            // Anything else must appear after the text that preceded it.
            _ => self.reveal_all(),
        }
        match ev {
            Event::Text(_) | Event::Drafting { .. } | Event::Usage(_) => {}
            Event::ToolCall(call) => {
                self.flush_partial();
                self.speaking = false;
                self.activity = brief(&call);
                self.out.push_in(Span::styled("● ", dim()), Line::styled(self.activity.clone(), dim()));
            }
            Event::ToolResult { ok, output } => {
                self.activity = THINKING.into();
                if let Some((add, del)) = self.stat.take().filter(|&s| ok && s != (0, 0)) {
                    let (add, del) = (format!("+{add}"), format!("-{del}"));
                    let spans = vec![Span::styled("⎿ ", dim()), Span::styled(add, Color::Green), Span::raw(" "), Span::styled(del, Color::Red)];
                    return self.out.push_in(Span::raw(INDENT), Line::from(spans));
                }
                // The model-facing refusal text is long; the user only needs the fact.
                let (first, failed) = if output.strip_prefix("error: ") == Some(DENIED) { ("denied".into(), true) } else { summary(ok, &output) };
                let style = if failed { Style::new().fg(Color::Red) } else { dim() };
                self.out.push_in(Span::raw(INDENT), Line::styled(format!("⎿ {}", truncate(&first, 120)), style));
            }
            // The user did not ask to review it: a line count is enough.
            Event::Diff { diff, .. } => self.stat = Some(stat(&diff)),
            Event::Approve { req, reply } => {
                self.flush_partial();
                let prompt = match req {
                    Approval::Write { diff, .. } => {
                        self.push_diff(&diff);
                        "Apply? (y/n)".to_string()
                    }
                    Approval::Exec { cmd, .. } => format!("Run `{cmd}`? (y/n)"),
                    Approval::Fetch { url } => format!("Fetch {url}? (y/n)"),
                };
                self.approval = Some((prompt, reply));
            }
        }
    }

    pub fn on_term(&mut self, ev: TermEvent) -> Action {
        match ev {
            TermEvent::Key(k) if k.kind == KeyEventKind::Press => self.on_key(k),
            TermEvent::Paste(s) => {
                self.insert(&s.replace("\r\n", "\n").replace('\r', "\n"));
                Action::None
            }
            TermEvent::Mouse(m) => {
                self.on_mouse(m);
                Action::None
            }
            _ => Action::None,
        }
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        self.selection = None;
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if self.approval.is_some() {
            match k.code {
                KeyCode::Char('c') if ctrl => return if self.ctrl_c() { Action::Quit } else { Action::Cancel },
                KeyCode::Char('y' | 'Y') | KeyCode::Enter => self.answer(true),
                KeyCode::Char('n' | 'N') | KeyCode::Esc => self.answer(false),
                _ => {}
            }
            return Action::None;
        }
        let sugg = self.suggestions();
        if !sugg.is_empty() {
            let (n, sel) = (sugg.len(), self.selected.min(sugg.len() - 1));
            match k.code {
                KeyCode::Up => self.selected = (sel + n - 1) % n,
                KeyCode::Down => self.selected = (sel + 1) % n,
                KeyCode::Tab => {
                    let s = &sugg[sel];
                    self.set_input(if s.takes_arg { format!("{} ", s.text) } else { s.text.clone() });
                }
                KeyCode::Esc => self.set_input(String::new()),
                KeyCode::Enter if k.modifiers.is_empty() => {
                    self.set_input(sugg[sel].text.clone());
                    return self.submit();
                }
                _ => return self.edit(k, ctrl),
            }
            return Action::None;
        }
        self.edit(k, ctrl)
    }

    /// Visible `/command` completions for the current input.
    pub fn suggestions(&self) -> Vec<Suggestion> {
        if self.busy || self.approval.is_some() || self.setup.is_some() { vec![] } else { command::suggest(&self.input) }
    }

    fn submit(&mut self) -> Action {
        let text = std::mem::take(&mut self.input);
        self.cursor = 0;
        self.browsing = None;
        self.selected = 0;
        if !self.secret() && !text.trim().is_empty() {
            self.history.push(text.clone());
        }
        self.out.follow();
        Action::Submit(text.trim().to_string())
    }

    fn edit(&mut self, k: KeyEvent, ctrl: bool) -> Action {
        match k.code {
            // Each press still does its job (stop the turn, clear the input);
            // only a second one in quick succession quits.
            KeyCode::Char('c') if ctrl => {
                if self.ctrl_c() {
                    return Action::Quit;
                }
                if self.busy {
                    return Action::Cancel;
                }
                self.set_input(String::new());
            }
            KeyCode::Char('d') if ctrl && self.input.is_empty() && !self.busy => return Action::Quit,
            KeyCode::Esc if self.busy => return Action::Cancel,
            KeyCode::Esc if self.setup.as_ref().is_some_and(Setup::closable) => return Action::Cancel,
            KeyCode::BackTab => self.mode.set(self.mode.get().next()),
            KeyCode::Char('j') if ctrl => self.insert("\n"),
            KeyCode::Enter if k.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) => self.insert("\n"),
            // Setup answers may be empty: Enter keeps the current value.
            KeyCode::Enter if !self.busy && (self.setup.is_some() || !self.input.trim().is_empty()) => return self.submit(),
            KeyCode::Char('u') if ctrl => self.set_input(String::new()),
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.input.len(),
            KeyCode::Char(c) if !ctrl => self.insert(c.encode_utf8(&mut [0; 4])),
            KeyCode::Backspace => {
                if let Some(prev) = self.prev_boundary() {
                    self.input.replace_range(prev..self.cursor, "");
                    self.cursor = prev;
                    self.selected = 0;
                }
            }
            KeyCode::Delete => {
                if let Some(next) = self.next_boundary() {
                    self.input.replace_range(self.cursor..next, "");
                    self.selected = 0;
                }
            }
            KeyCode::Left => self.cursor = self.prev_boundary().unwrap_or(self.cursor),
            KeyCode::Right => self.cursor = self.next_boundary().unwrap_or(self.cursor),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.len(),
            KeyCode::Up => self.browse(true),
            KeyCode::Down => self.browse(false),
            KeyCode::PageUp => self.out.page(true),
            KeyCode::PageDown => self.out.page(false),
            _ => {}
        }
        Action::None
    }

    /// The tool's result line follows and tells the outcome ("denied" or its output).
    fn answer(&mut self, yes: bool) {
        if let Some((_, reply)) = self.approval.take() {
            let _ = reply.send(yes);
        }
    }

    fn browse(&mut self, older: bool) {
        let pos = match (self.browsing, older) {
            (None, true) => self.history.len().checked_sub(1),
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i + 1 < self.history.len() => Some(i + 1),
            _ => None,
        };
        self.browsing = pos;
        self.set_input(pos.map(|i| self.history[i].clone()).unwrap_or_default());
    }

    fn set_input(&mut self, text: String) {
        self.cursor = text.len();
        self.input = text;
        self.selected = 0;
    }

    fn insert(&mut self, s: &str) {
        self.selected = 0;
        self.input.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    fn prev_boundary(&self) -> Option<usize> {
        self.input[..self.cursor].char_indices().next_back().map(|(i, _)| i)
    }

    fn next_boundary(&self) -> Option<usize> {
        self.input[self.cursor..].chars().next().map(|c| self.cursor + c.len_utf8())
    }

    /// Indented under the tool call it belongs to, which already names the files.
    fn push_diff(&mut self, diff: &str) {
        let lines: Vec<&str> = diff.lines().collect();
        for line in lines.iter().take(DIFF_LINES) {
            let style = match line.as_bytes().first() {
                Some(b'+') => Style::new().fg(Color::Green),
                Some(b'-') => Style::new().fg(Color::Red),
                Some(b'@') => dim(),
                _ => Style::new(),
            };
            self.out.push_in(Span::raw("    "), Line::styled(line.to_string(), style));
        }
        if lines.len() > DIFF_LINES {
            self.out.push_in(Span::raw("    "), Line::styled(format!("… {} more lines", lines.len() - DIFF_LINES), dim()));
        }
    }

    /// Reveals a frame's worth of pending text; the rate grows with the
    /// backlog so the display never lags far behind the stream.
    pub fn reveal(&mut self) -> bool {
        let backlog = self.pending.chars().count();
        if backlog == 0 {
            return false;
        }
        let n = (backlog / 6).max(3);
        let cut = self.pending.char_indices().nth(n).map_or(self.pending.len(), |(i, _)| i);
        let chunk: String = self.pending.drain(..cut).collect();
        self.push_text(&chunk);
        true
    }

    fn reveal_all(&mut self) {
        let all = std::mem::take(&mut self.pending);
        self.push_text(&all);
    }

    fn push_text(&mut self, t: &str) {
        self.partial.push_str(t);
        while let Some(i) = self.partial.find('\n') {
            let line = self.md.line(&self.partial[..i]);
            self.push_ai(line);
            self.partial.drain(..=i);
        }
    }

    /// The model's text: a bullet opens each block, its other lines align after it.
    pub fn ai_gutter(&self) -> Span<'static> {
        if self.speaking { Span::raw(INDENT) } else { Span::styled("● ", Style::new().fg(ACCENT)) }
    }

    fn push_ai(&mut self, line: Line<'static>) {
        let gutter = self.ai_gutter();
        self.speaking = true;
        self.out.push_in(gutter, line);
    }

    pub fn start_turn(&mut self) {
        self.busy = true;
        self.started = Instant::now();
        self.activity = THINKING.into();
    }

    pub fn flush_partial(&mut self) {
        if !self.partial.is_empty() {
            let line = self.md.line(&std::mem::take(&mut self.partial));
            self.push_ai(line);
        }
    }

    /// Closes the visual block of a turn.
    pub fn end_turn(&mut self) {
        self.reveal_all();
        self.flush_partial();
        self.md.reset();
        self.busy = false;
        self.speaking = false;
        self.approval = None;
    }

    pub fn echo(&mut self, input: &str) {
        self.out.push(Line::default());
        let bold = Style::new().add_modifier(Modifier::BOLD);
        for (i, l) in input.lines().enumerate() {
            let gutter = if i == 0 { "› " } else { INDENT };
            self.out.push_in(Span::styled(gutter, bold), Line::styled(l.to_string(), bold));
        }
    }

    /// Blank screen for a fresh conversation; session cost totals stay.
    pub fn clear(&mut self) {
        self.out = Transcript::default();
        self.partial.clear();
        self.pending.clear();
        self.md.reset();
        self.context = 0;
        self.stat = None;
    }

    /// A setup question: its first line stands out, choices follow.
    pub fn ask(&mut self, prompt: &str) {
        self.out.push(Line::default());
        for (i, l) in prompt.lines().enumerate() {
            let (gutter, style) = match i {
                0 => (Span::styled("? ", Style::new().fg(ACCENT)), Style::new().add_modifier(Modifier::BOLD)),
                _ => (Span::raw(INDENT), Style::new()),
            };
            self.out.push_in(gutter, Line::styled(l.to_string(), style));
        }
    }

    pub fn say(&mut self, msg: &str, error: bool) {
        let style = if error { Style::new().fg(Color::Red) } else { dim() };
        self.out.push_in(Span::raw(INDENT), Line::styled(msg.to_string(), style));
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{Action, App};
    use crate::permission::Mode;

    #[test]
    fn dragging_selects_and_releasing_copies_what_is_on_screen() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::crossterm::event::{Event, MouseButton, MouseEvent, MouseEventKind};
        use ratatui::text::{Line, Span};

        let mut app = App::new("m".into(), Rc::new(Cell::new(Mode::Ask)));
        app.out.push_in(Span::raw("● "), Line::raw("hello world"));
        let mut term = Terminal::new(TestBackend::new(30, 10)).unwrap();
        let mut frame = |app: &mut App| drop(term.draw(|f| crate::tui::view::render(f, app)).unwrap());
        let mouse = |kind, column, row| Event::Mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE });
        let left = MouseButton::Left;
        frame(&mut app);
        // A short conversation sits at the top of the history.
        let row = app.view_area.y;

        // A plain click selects and copies nothing; neither does one outside the history.
        app.on_term(mouse(MouseEventKind::Down(left), 3, row));
        app.on_term(mouse(MouseEventKind::Up(left), 3, row));
        frame(&mut app);
        assert!(app.selection.is_none() && app.copied.is_none());
        app.on_term(mouse(MouseEventKind::Down(left), 3, 9));
        assert!(app.selection.is_none());

        app.on_term(mouse(MouseEventKind::Down(left), 2, row));
        app.on_term(mouse(MouseEventKind::Drag(left), 6, row));
        frame(&mut app);
        assert!(app.copied.is_none(), "nothing is copied while the button is down");
        app.on_term(mouse(MouseEventKind::Up(left), 6, row));
        frame(&mut app);
        assert_eq!(app.copied.take().as_deref(), Some("hello"));
        // Still highlighted, until the view moves or a key is pressed.
        assert!(app.selection.is_some());
        app.on_term(mouse(MouseEventKind::ScrollUp, 6, row));
        assert!(app.selection.is_none());
    }

    #[test]
    fn quits_only_on_a_second_ctrl_c_in_time() {
        let mut app = App::new("m".into(), Rc::new(Cell::new(Mode::Ask)));
        let mut ctrl_c = |app: &mut App| app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(matches!(ctrl_c(&mut app), Action::None) && app.quit_armed());
        assert!(matches!(ctrl_c(&mut app), Action::Quit));

        // Too late: the second press only arms again.
        app.quit_until = Some(Instant::now() - Duration::from_millis(1));
        assert!(matches!(ctrl_c(&mut app), Action::None));

        // During a turn the first press stops it, the second quits.
        let mut app = App::new("m".into(), Rc::new(Cell::new(Mode::Ask)));
        app.busy = true;
        assert!(matches!(ctrl_c(&mut app), Action::Cancel));
        assert!(matches!(ctrl_c(&mut app), Action::Quit));

        // With text typed, the first press clears it.
        let mut app = App::new("m".into(), Rc::new(Cell::new(Mode::Ask)));
        app.input = "draft".into();
        app.cursor = 5;
        assert!(matches!(ctrl_c(&mut app), Action::None) && app.input.is_empty());
    }

    #[test]
    fn counts_diff_lines_without_headers() {
        assert_eq!(crate::ui::stat("--- a\n+++ b\n@@ -1 +1,2 @@\n-x\n+y\n+z\n ctx\n"), (2, 1));
        assert_eq!(crate::ui::stat("(binary)"), (0, 0));
    }
}
