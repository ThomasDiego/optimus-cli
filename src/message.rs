use std::ops::AddAssign;

use serde_json::Value;

/// Provider-neutral conversation message.
#[derive(Clone, Debug)]
pub enum Message {
    User(String),
    Assistant { text: String, calls: Vec<ToolCall> },
    Tool { id: String, content: String },
}

#[derive(Clone, Debug, Default)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Raw JSON arguments as streamed by the model.
    pub args: String,
}

pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: Value,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub cache_write: u64,
    pub cost: f64,
}

impl AddAssign for Usage {
    fn add_assign(&mut self, o: Self) {
        self.input += o.input;
        self.output += o.output;
        self.cached += o.cached;
        self.cache_write += o.cache_write;
        self.cost += o.cost;
    }
}

#[derive(Debug, Default)]
pub struct Reply {
    pub text: String,
    pub calls: Vec<ToolCall>,
    pub usage: Usage,
}
