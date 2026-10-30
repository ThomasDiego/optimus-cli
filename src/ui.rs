use serde_json::Value;
use tokio::sync::oneshot;

use crate::message::{ToolCall, Usage};
use crate::tools::truncate;

/// Everything the agent shows or asks, consumed by the REPL or the TUI.
pub enum Event {
    Text(String),
    /// Tokens and cost of one model request, for live accounting.
    Usage(Usage),
    ToolCall(ToolCall),
    ToolResult { ok: bool, output: String },
    /// Files were written without confirmation; shown to the user only.
    Diff { label: String, diff: String },
    Approve { req: Approval, reply: oneshot::Sender<bool> },
}

pub enum Approval {
    /// `label` names the files: `src/a.rs` or `a.rs → b.rs`.
    Write { label: String, diff: String },
    Exec { cmd: String, net: bool },
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
        "move_file" => s("from").zip(s("to")).map(|(from, to)| format!("{from} → {to}")),
        _ => s("path").or_else(|| {
            let paths = args["paths"].as_array()?.iter().filter_map(Value::as_str).collect::<Vec<_>>();
            Some(paths.join(", "))
        }),
    };
    match detail {
        Some(d) => format!("{} {}", call.name, truncate(&d, 100)),
        None => call.name.clone(),
    }
}
