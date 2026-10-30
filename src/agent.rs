use std::cell::Cell;
use std::rc::Rc;

use anyhow::{Result, bail};
use tokio::sync::{mpsc, oneshot};

use crate::exec::Review;
use crate::{exec, track, web};
use crate::message::{Message, ToolCall, ToolDef, Usage};
use crate::permission::{Kind, Mode};
use crate::provider::{Delta, Provider, Request};
use crate::tools::{self, Ctx, Outcome, Tool, change};
use crate::ui::{self, Approval, Event};

/// Static on purpose: any dynamic content here would break prompt caching.
const SYSTEM: &str = "You are ai, a coding agent in the user's project directory. \
There is no shell: use the tools; run starts allowed programs directly. \
Paths are relative to the project root; nothing outside it is reachable. \
Read a file before editing it. Prefer edit_file and batch edits in one call. \
Call independent tools together in one reply; read_file takes paths to read several files at once. \
Do exactly what was asked: no extra files, features, docs or refactors unless requested. \
When a tool refuses or cannot do something, never work around it with a script or another tool: tell the user. \
Tool results are authoritative: never re-read, re-list or re-run just to confirm a success. \
Verify changes by building or testing when possible. Reply in the user's language. \
Be concise: no preamble or narration around tool calls, just the final answer.";

/// Hard stop against runaway tool loops.
const MAX_STEPS: usize = 50;

/// Stops retry loops: a denial is final for this request.
pub const DENIED: &str = "not approved by the user or permission mode; do not retry, tell the user what you wanted to do";

pub struct Agent<P> {
    provider: P,
    tools: Vec<Box<dyn Tool>>,
    defs: Vec<ToolDef>,
    messages: Vec<Message>,
    events: mpsc::UnboundedSender<Event>,
    /// Shared with the UI so the mode can change while a turn runs.
    pub mode: Rc<Cell<Mode>>,
    /// Context for the model, prepended to the next user message.
    note: Option<String>,
}

impl<P: Provider> Agent<P> {
    pub fn new(provider: P, tools: Vec<Box<dyn Tool>>, events: mpsc::UnboundedSender<Event>, mode: Mode) -> Self {
        let defs = tools.iter().map(|t| t.def()).collect();
        Self { provider, tools, defs, messages: vec![], events, mode: Rc::new(Cell::new(mode)), note: None }
    }

    /// Runs one user turn until the model answers without tool calls.
    pub async fn turn(&mut self, input: String, ctx: &mut Ctx) -> Result<Usage> {
        change::begin_turn(ctx);
        let mut notes: Vec<String> = self.note.take().into_iter().collect();
        // Saves the usual list_dir/glob round trips that open every task.
        if self.messages.is_empty() {
            // Capped lower than list_dir: it is paid on every request of the session.
            notes.push(format!("[Project files, depth 2]\n{}", tools::tree(ctx, &ctx.ws.root, 2, 80)));
            if !ctx.ws.dir.is_dir(".git") {
                notes.push("[Not a git repository]".into());
            }
        }
        let stale = tools::stale(ctx);
        if !stale.is_empty() {
            notes.push(format!("[Changed since you read them, re-read before editing: {}]", stale.join(", ")));
        }
        notes.push(input);
        self.messages.push(Message::User(notes.join("\n\n")));
        let mut usage = Usage::default();
        for _ in 0..MAX_STEPS {
            let req = Request { system: SYSTEM, messages: &self.messages, tools: &self.defs };
            let events = self.events.clone();
            let mut on = |d: Delta<'_>| {
                let ev = match d {
                    Delta::Text(t) => Event::Text(t.to_string()),
                    Delta::Call { name, args, added } => Event::Drafting { label: ui::draft(name, args), added },
                };
                let _ = events.send(ev);
            };
            let reply = self.provider.send(&req, &mut on).await?;
            usage += reply.usage;
            self.emit(Event::Usage(reply.usage));
            let calls = reply.calls.clone();
            self.messages.push(Message::Assistant { text: reply.text, calls: reply.calls });
            if calls.is_empty() {
                return Ok(usage);
            }
            for call in calls {
                self.emit(Event::ToolCall(call.clone()));
                let result = self.execute(ctx, &call).await;
                let (ok, output) = match result {
                    Ok(out) => (true, out),
                    Err(e) => (false, format!("error: {e}")),
                };
                self.emit(Event::ToolResult { ok, output: output.clone() });
                self.messages.push(Message::Tool { id: call.id, content: output });
            }
        }
        bail!("stopped after {MAX_STEPS} steps")
    }

    async fn execute(&self, ctx: &mut Ctx, call: &ToolCall) -> Result<String, String> {
        let i = self.defs.iter().position(|d| d.name == call.name).ok_or_else(|| format!("unknown tool {}", call.name))?;
        match self.tools[i].run(ctx, &call.args)? {
            Outcome::Done(out) => Ok(out),
            Outcome::Write(batch) => {
                let (label, diff) = (batch.label(), batch.diff());
                if self.mode.get().needs_approval(Kind::Write) {
                    if !self.approve(Approval::Write { label, diff }).await {
                        return Err(DENIED.into());
                    }
                } else {
                    self.emit(Event::Diff { diff });
                }
                change::apply(ctx, batch)
            }
            Outcome::Exec(ex) => {
                let kind = match (ex.net, ex.review) {
                    (true, _) => Some(Kind::ExecNet),
                    (false, Review::None) => None,
                    (false, Review::Write) => Some(Kind::Write),
                    (false, Review::Exec) => Some(Kind::Exec),
                };
                let ask = kind.is_some_and(|k| self.mode.get().needs_approval(k));
                if ask && !self.approve(Approval::Exec { cmd: ex.display(), net: ex.net }).await {
                    return Err(DENIED.into());
                }
                if ex.background {
                    return ctx.jobs.start(&ex, &ctx.ws.root).await;
                }
                let before = track::snapshot(&ctx.ws);
                let out = exec::run(&ex, &ctx.ws.root).await?;
                Ok(track::annotate(out, &track::changes(&before, &track::snapshot(&ctx.ws))))
            }
            Outcome::Fetch(f) => {
                if self.mode.get().needs_approval(Kind::ExecNet) && !self.approve(Approval::Fetch { url: f.url.clone() }).await {
                    return Err(DENIED.into());
                }
                web::fetch(&f).await
            }
            Outcome::Wait { id, secs } => ctx.jobs.wait(id, secs).await,
        }
    }

    async fn approve(&self, req: Approval) -> bool {
        let (reply, answer) = oneshot::channel();
        self.emit(Event::Approve { req, reply });
        answer.await.unwrap_or(false)
    }

    fn emit(&self, ev: Event) {
        let _ = self.events.send(ev);
    }

    /// After a cancelled turn, answers dangling tool calls so the history stays valid.
    pub fn heal(&mut self) {
        let Some(pos) = self.messages.iter().rposition(|m| matches!(m, Message::Assistant { .. })) else { return };
        let Message::Assistant { calls, .. } = &self.messages[pos] else { return };
        let answered: Vec<&str> = self.messages[pos + 1..]
            .iter()
            .filter_map(|m| match m {
                Message::Tool { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        let missing: Vec<String> = calls.iter().filter(|c| !answered.contains(&c.id.as_str())).map(|c| c.id.clone()).collect();
        for id in missing {
            self.messages.push(Message::Tool { id, content: "interrupted by user".into() });
        }
    }

    pub fn note(&mut self, note: String) {
        self.note = Some(note);
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.note = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Reply;

    struct Silent;

    impl Provider for Silent {
        async fn send(&self, _: &Request<'_>, _: &mut dyn FnMut(Delta<'_>)) -> Result<Reply> {
            Ok(Reply::default())
        }
    }

    #[test]
    fn heal_answers_dangling_calls_only() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut agent = Agent::new(Silent, vec![], tx, Mode::Ask);
        let call = |id: &str| ToolCall { id: id.into(), name: "read_file".into(), args: "{}".into() };
        agent.messages.push(Message::Assistant { text: String::new(), calls: vec![call("a"), call("b")] });
        agent.messages.push(Message::Tool { id: "a".into(), content: "ok".into() });
        agent.heal();
        agent.heal();
        let tools: Vec<&str> = agent
            .messages
            .iter()
            .filter_map(|m| match m {
                Message::Tool { id, content } => Some(if content == "ok" { id.as_str() } else { "healed" }),
                _ => None,
            })
            .collect();
        assert_eq!(tools, ["a", "healed"]);
    }
}
