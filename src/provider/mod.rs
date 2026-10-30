mod openai_compat;
mod sse;

use anyhow::Result;

use crate::message::{Message, Reply, ToolDef};

pub use openai_compat::OpenAiCompat;

pub struct Request<'a> {
    pub system: &'a str,
    pub messages: &'a [Message],
    pub tools: &'a [ToolDef],
}

/// One implementation per wire format; the agent only sees neutral types.
pub trait Provider {
    /// Streams one completion, forwarding text deltas to `on_text`.
    async fn send(&self, req: &Request<'_>, on_text: &mut dyn FnMut(&str)) -> Result<Reply>;
}
