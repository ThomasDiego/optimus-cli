use std::path::Path;

use crate::message::ToolCall;

/// Everything the agent shows or asks. The REPL implements it today, the TUI later.
pub trait Ui {
    fn text(&mut self, chunk: &str);
    fn tool_call(&mut self, call: &ToolCall);
    fn tool_result(&mut self, ok: bool, output: &str);
    /// Confirms a file write; the diff is shown to the user only.
    fn approve(&mut self, path: &Path, diff: &str) -> bool;
}
