use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use tokio::sync::oneshot;

use super::markdown::Markdown;
use super::transcript::Transcript;
use crate::command::{self, Suggestion};
use crate::message::Usage;
use crate::permission::Mode;
use crate::tools::truncate;
use crate::ui::{Approval, Event, brief};

const DIFF_LINES: usize = 60;

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
    pub started: Instant,
    pub approval: Option<(String, oneshot::Sender<bool>)>,
    pub usage: Usage,
    /// Tokens of the latest request: the current conversation size.
    pub context: u64,
    pub mode: Rc<Cell<Mode>>,
    pub model: String,
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
            started: Instant::now(),
            approval: None,
            usage: Usage::default(),
            context: 0,
            mode,
            model,
        }
    }

    pub fn on_agent(&mut self, ev: Event) {
        match ev {
            Event::Text(t) => return self.pending.push_str(&t),
            Event::Usage(u) => {
                self.context = u.input + u.output;
                return self.usage += u;
            }
            // Anything else must appear after the text that preceded it.
            _ => self.reveal_all(),
        }
        match ev {
            Event::Text(_) | Event::Usage(_) => {}
            Event::ToolCall(call) => {
                self.flush_partial();
                self.activity = brief(&call);
                self.out.push(Line::styled(format!("● {}", self.activity), dim()));
            }
            Event::ToolResult { ok, output } => {
                self.activity = "Réflexion".into();
                let first = truncate(output.lines().next().unwrap_or(""), 120).replace('\t', "  ");
                let style = if ok { dim() } else { Style::new().fg(Color::Red) };
                self.out.push(Line::styled(format!("  ⎿ {first}"), style));
            }
            Event::Diff { path, diff } => self.push_diff(&path.display().to_string(), &diff),
            Event::Approve { req, reply } => {
                self.flush_partial();
                let prompt = match req {
                    Approval::Write { path, diff } => {
                        self.push_diff(&path.display().to_string(), &diff);
                        "Appliquer ? (o/n)".to_string()
                    }
                    Approval::Exec { cmd, .. } => format!("Exécuter « {cmd} » ? (o/n)"),
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
                match m.kind {
                    MouseEventKind::ScrollUp => self.out.scroll(3),
                    MouseEventKind::ScrollDown => self.out.scroll(-3),
                    _ => {}
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if self.approval.is_some() {
            match k.code {
                KeyCode::Char('c') if ctrl => return Action::Cancel,
                KeyCode::Char('o' | 'O' | 'y' | 'Y') | KeyCode::Enter => self.answer(true),
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
        if self.busy || self.approval.is_some() { vec![] } else { command::suggest(&self.input) }
    }

    fn submit(&mut self) -> Action {
        let text = std::mem::take(&mut self.input);
        self.cursor = 0;
        self.browsing = None;
        self.selected = 0;
        self.history.push(text.clone());
        self.out.follow();
        Action::Submit(text.trim().to_string())
    }

    fn edit(&mut self, k: KeyEvent, ctrl: bool) -> Action {
        match k.code {
            KeyCode::Char('c') if ctrl => {
                if self.busy {
                    return Action::Cancel;
                }
                if self.input.is_empty() {
                    return Action::Quit;
                }
                self.set_input(String::new());
            }
            KeyCode::Char('d') if ctrl && self.input.is_empty() && !self.busy => return Action::Quit,
            KeyCode::Esc if self.busy => return Action::Cancel,
            KeyCode::BackTab => self.mode.set(self.mode.get().next()),
            KeyCode::Char('j') if ctrl => self.insert("\n"),
            KeyCode::Enter if k.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) => self.insert("\n"),
            KeyCode::Enter if !self.busy && !self.input.trim().is_empty() => return self.submit(),
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

    fn answer(&mut self, yes: bool) {
        if let Some((_, reply)) = self.approval.take() {
            let _ = reply.send(yes);
            self.out.push(Line::styled(if yes { "  ✓ autorisé" } else { "  ✗ refusé" }, dim()));
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

    fn push_diff(&mut self, path: &str, diff: &str) {
        self.out.push(Line::styled(path.to_string(), Style::new().add_modifier(Modifier::BOLD)));
        let lines: Vec<&str> = diff.lines().collect();
        for line in lines.iter().take(DIFF_LINES) {
            let style = match line.as_bytes().first() {
                Some(b'+') => Style::new().fg(Color::Green),
                Some(b'-') => Style::new().fg(Color::Red),
                Some(b'@') => dim(),
                _ => Style::new(),
            };
            self.out.push(Line::styled(line.to_string(), style));
        }
        if lines.len() > DIFF_LINES {
            self.out.push(Line::styled(format!("… {} lignes de plus", lines.len() - DIFF_LINES), dim()));
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
            self.out.push(line);
            self.partial.drain(..=i);
        }
    }

    pub fn start_turn(&mut self) {
        self.busy = true;
        self.started = Instant::now();
        self.activity = "Réflexion".into();
    }

    pub fn flush_partial(&mut self) {
        if !self.partial.is_empty() {
            let line = self.md.line(&std::mem::take(&mut self.partial));
            self.out.push(line);
        }
    }

    /// Closes the visual block of a turn.
    pub fn end_turn(&mut self) {
        self.reveal_all();
        self.flush_partial();
        self.md.reset();
        self.busy = false;
        self.approval = None;
    }

    pub fn echo(&mut self, input: &str) {
        self.out.push(Line::default());
        for (i, l) in input.lines().enumerate() {
            let prefix = if i == 0 { "› " } else { "  " };
            self.out.push(Line::styled(format!("{prefix}{l}"), Style::new().add_modifier(Modifier::BOLD)));
        }
    }

    pub fn say(&mut self, msg: &str, error: bool) {
        let style = if error { Style::new().fg(Color::Red) } else { dim() };
        self.out.push(Line::styled(msg.to_string(), style));
    }
}
