use anyhow::{Result, bail};

use crate::message::{Message, ToolDef, Usage};
use crate::provider::{Provider, Request};
use crate::tools::{Ctx, Tool};
use crate::ui::Ui;

/// Static on purpose: any dynamic content here would break prompt caching.
const SYSTEM: &str = "You are ai, a coding agent in the user's project directory. \
There is no shell: use the tools. Paths are relative to the project root; nothing outside it is reachable. \
Read a file before editing it. Prefer edit_file and batch edits in one call. Be concise.";

/// Hard stop against runaway tool loops.
const MAX_STEPS: usize = 50;

pub struct Agent<P> {
    provider: P,
    tools: Vec<Box<dyn Tool>>,
    defs: Vec<ToolDef>,
    messages: Vec<Message>,
}

impl<P: Provider> Agent<P> {
    pub fn new(provider: P, tools: Vec<Box<dyn Tool>>) -> Self {
        let defs = tools.iter().map(|t| t.def()).collect();
        Self { provider, tools, defs, messages: vec![] }
    }

    /// Runs one user turn until the model answers without tool calls.
    pub async fn turn(&mut self, input: String, ctx: &mut Ctx, ui: &mut dyn Ui) -> Result<Usage> {
        self.messages.push(Message::User(input));
        let mut usage = Usage::default();
        for _ in 0..MAX_STEPS {
            let req = Request { system: SYSTEM, messages: &self.messages, tools: &self.defs };
            let reply = self.provider.send(&req, &mut |t| ui.text(t)).await?;
            usage += reply.usage;
            let calls = reply.calls.clone();
            self.messages.push(Message::Assistant { text: reply.text, calls: reply.calls });
            if calls.is_empty() {
                return Ok(usage);
            }
            for call in calls {
                ui.tool_call(&call);
                let result = match self.defs.iter().position(|d| d.name == call.name) {
                    Some(i) => self.tools[i].run(ctx, ui, &call.args),
                    None => Err(format!("unknown tool {}", call.name)),
                };
                let content = match result {
                    Ok(out) => {
                        ui.tool_result(true, &out);
                        out
                    }
                    Err(e) => {
                        ui.tool_result(false, &e);
                        format!("error: {e}")
                    }
                };
                self.messages.push(Message::Tool { id: call.id, content });
            }
        }
        bail!("limite de {MAX_STEPS} étapes atteinte")
    }
}
