mod openai_compat;
mod sse;

use anyhow::Result;

use crate::message::{Message, Reply, ToolDef};

pub use openai_compat::{CheckError, OpenAiCompat};

pub struct Request<'a> {
    pub system: &'a str,
    pub messages: &'a [Message],
    pub tools: &'a [ToolDef],
}

/// What a stream chunk added, for live display.
pub enum Delta<'a> {
    Text(&'a str),
    /// A tool call being generated: its name and arguments so far, and how
    /// many characters this chunk added.
    Call { name: &'a str, args: &'a str, added: usize },
}

/// One implementation per wire format; the agent only sees neutral types.
pub trait Provider {
    /// Streams one completion, forwarding each delta to `on`.
    async fn send(&self, req: &Request<'_>, on: &mut dyn FnMut(Delta<'_>)) -> Result<Reply>;
}
