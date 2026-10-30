use std::cell::Cell;
use std::rc::Rc;

use anyhow::{Result, bail};
use tokio::sync::{mpsc, oneshot};

use crate::exec;
use crate::message::{Message, ToolCall, ToolDef, Usage};
use crate::permission::{Kind, Mode};
use crate::provider::{Provider, Request};
use crate::tools::{Ctx, Outcome, Tool, change};
use crate::ui::{Approval, Event};

/// Static on purpose: any dynamic content here would break prompt caching.
const SYSTEM: &str = "You are ai, a coding agent in the user's project directory. \
There is no shell: use the tools; run starts allowed programs directly. \
Paths are relative to the project root; nothing outside it is reachable. \
Read a file before editing it. Prefer edit_file and batch edits in one call. \
Verify changes by building or testing when possible. Be concise.";

/// Hard stop against runaway tool loops.
const MAX_STEPS: usize = 50;

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
        let input = match self.note.take() {
            Some(note) => format!("{note}\n\n{input}"),
            None => input,
        };
        self.messages.push(Message::User(input));
        let mut usage = Usage::default();
        for _ in 0..MAX_STEPS {
            let req = Request { system: SYSTEM, messages: &self.messages, tools: &self.defs };
            let events = self.events.clone();
            let reply = self.provider.send(&req, &mut |t| drop(events.send(Event::Text(t.to_string())))).await?;
            usage += reply.usage;
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
        bail!("limite de {MAX_STEPS} étapes atteinte")
    }

    async fn execute(&self, ctx: &mut Ctx, call: &ToolCall) -> Result<String, String> {
        let i = self.defs.iter().position(|d| d.name == call.name).ok_or_else(|| format!("unknown tool {}", call.name))?;
        match self.tools[i].run(ctx, &call.args)? {
            Outcome::Done(out) => Ok(out),
            Outcome::Write(ch) => {
                let (path, diff) = (ch.rel.clone(), ch.diff());
                if self.mode.get().needs_approval(Kind::Write) {
                    if !self.approve(Approval::Write { path, diff }).await {
                        return Err("rejected by user".into());
                    }
                } else {
                    self.emit(Event::Diff { path, diff });
                }
                change::apply(ctx, ch)
            }
            Outcome::Exec(ex) => {
                if self.mode.get().needs_approval(Kind::Exec) && !self.approve(Approval::Exec { cmd: ex.display() }).await {
                    return Err("rejected by user".into());
                }
                exec::run(&ex, &ctx.ws.root).await
            }
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

    pub fn note(&mut self, note: String) {
        self.note = Some(note);
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.note = None;
    }
}
