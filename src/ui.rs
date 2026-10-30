use std::path::PathBuf;

use serde_json::Value;
use tokio::sync::oneshot;

use crate::message::ToolCall;
use crate::tools::truncate;

/// Everything the agent shows or asks, consumed by the REPL or the TUI.
pub enum Event {
    Text(String),
    ToolCall(ToolCall),
    ToolResult { ok: bool, output: String },
    /// A file was written without confirmation; shown to the user only.
    Diff { path: PathBuf, diff: String },
    Approve { req: Approval, reply: oneshot::Sender<bool> },
}

pub enum Approval {
    Write { path: PathBuf, diff: String },
    Exec { cmd: String },
}

/// One-line human summary of a tool call: `read_file src/main.rs`.
pub fn brief(call: &ToolCall) -> String {
    let args: Value = serde_json::from_str(&call.args).unwrap_or(Value::Null);
    let s = |k: &str| args[k].as_str().map(str::to_string);
    let detail = match call.name.as_str() {
        "run" => s("cmd").map(|c| {
            let rest = args["args"].as_array().into_iter().flatten().filter_map(Value::as_str);
            std::iter::once(c.as_str()).chain(rest).collect::<Vec<_>>().join(" ")
        }),
        "glob" | "grep" => s("pattern").map(|p| match s("path") {
            Some(path) => format!("{p} in {path}"),
            None => p,
        }),
        _ => s("path"),
    };
    let detail = detail.unwrap_or_else(|| call.args.clone());
    format!("{} {}", call.name, truncate(&detail, 100))
}
