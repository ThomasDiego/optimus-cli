use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::app::App;
use super::input::{self, ACCENT, MAX_SUGGESTIONS};
use crate::message::{kilo, money};
use crate::permission::Mode;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// One animation step (~16 fps): the shimmer moves one char, the spinner
/// turns every other step.
const STEP_MS: usize = 60;

/// Changes exactly when the animated row would look different.
pub fn anim_step(app: &App) -> usize {
    if app.approval.is_some() { 0 } else { app.started.elapsed().as_millis() as usize / STEP_MS }
}

fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

/// Full screen: the conversation fills the space above a footer made of the
/// live row (during a turn), the chat box, suggestions and status.
pub fn render(f: &mut Frame, app: &mut App) {
    let sugg = app.suggestions();
    let shown = sugg.len().min(MAX_SUGGESTIONS) as u16;
    let rows = input::rows(app, f.area().width) as u16;
    let [history, live, chat, list, status] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(u16::from(app.busy)),
        Constraint::Length(rows + 2),
        Constraint::Length(shown),
        Constraint::Length(1),
    ])
    .areas(f.area());
    let tail = (!app.partial.is_empty()).then(|| Line::raw(app.partial.clone()));
    app.out.render(history, f.buffer_mut(), tail.as_ref());
    f.render_widget(live_line(app), live);
    input::render_box(f, app, chat);
    if shown > 0 {
        input::render_suggestions(f, &sugg, app.selected, list);
    }
    f.render_widget(status_line(app, status.width as usize), status);
}

fn live_line(app: &App) -> Line<'static> {
    if let Some((prompt, _)) = &app.approval {
        return Line::styled(format!(" {prompt}"), Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD));
    }
    if !app.busy {
        return Line::default();
    }
    let ms = app.started.elapsed().as_millis() as usize;
    let mut spans = vec![Span::styled(format!(" {} ", SPINNER[(ms / (STEP_MS * 2)) % SPINNER.len()]), Style::new().fg(ACCENT))];
    spans.extend(shimmer(&app.activity, ms));
    spans.push(Span::styled(format!("  {}s", ms / 1000), dim()));
    Line::from(spans)
}

/// A soft highlight sweeping across the label, then a short pause.
fn shimmer(label: &str, ms: usize) -> Vec<Span<'static>> {
    let chars: Vec<char> = label.chars().take(60).collect();
    let head = (ms / STEP_MS) % (chars.len() + 12);
    chars
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let style = match head.abs_diff(i + 3) {
                0 => Style::new().fg(Color::White).add_modifier(Modifier::BOLD),
                1 | 2 => Style::new(),
                _ => dim(),
            };
            Span::styled(c.to_string(), style)
        })
        .collect()
}

/// Drops the least useful segments first when the terminal is narrow.
fn status_line(app: &App, width: usize) -> Line<'static> {
    let mode = app.mode.get();
    let mode_style = match mode {
        Mode::Ask => dim(),
        Mode::Edit => Style::new().fg(Color::Yellow),
        Mode::Auto => Style::new().fg(Color::Red),
    };
    // `ctx` is the conversation size now; ↑/↓ are billed totals, which count
    // the context again on every request.
    let u = app.usage;
    let cache = (u.cached * 100).checked_div(u.input).unwrap_or(0);
    let ctx = format!("ctx {}", kilo(app.context));
    let tokens = format!("↑{} ↓{}", kilo(u.input), kilo(u.output));
    let cost = money(u.cost);
    let candidates = [
        vec![app.model.clone(), ctx.clone(), tokens.clone(), format!("cache {cache}%"), cost.clone()],
        vec![ctx.clone(), tokens, cost.clone()],
        vec![ctx, cost.clone()],
        vec![cost],
        vec![],
    ];
    let mode_label = format!(" {}", mode.name());
    let rest = candidates
        .iter()
        .map(|c| c.iter().map(|s| format!(" · {s}")).collect::<String>())
        .find(|r| mode_label.chars().count() + r.chars().count() <= width)
        .unwrap_or_default();
    Line::from(vec![Span::styled(mode_label, mode_style), Span::styled(rest, dim())])
}

#[cfg(test)]
mod tests {
    use super::shimmer;

    #[test]
    fn shimmer_keeps_text() {
        let text: String = shimmer("Réflexion", 300).iter().map(|s| s.content.to_string()).collect();
        assert_eq!(text, "Réflexion");
    }
}
