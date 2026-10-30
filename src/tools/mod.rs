mod edit;
mod file;
mod glob;
mod grep;
mod list_dir;
mod matching;
mod read;
mod write;

use std::collections::HashMap;
use std::path::PathBuf;

use serde::de::DeserializeOwned;

use crate::message::ToolDef;
use crate::sandbox::Workspace;
use crate::ui::Ui;

pub use file::truncate;

/// What the model has seen of a file: guards edits and dedups re-reads.
pub struct FileState {
    pub hash: u64,
    pub ranges: Vec<(usize, usize)>,
}

pub struct Ctx {
    pub ws: Workspace,
    pub reads: HashMap<PathBuf, FileState>,
}

impl Ctx {
    pub fn new(ws: Workspace) -> Self {
        Self { ws, reads: HashMap::new() }
    }
}

pub trait Tool {
    fn def(&self) -> ToolDef;
    /// Errors are returned to the model as short actionable messages.
    fn run(&self, ctx: &mut Ctx, ui: &mut dyn Ui, args: &str) -> Result<String, String>;
}

/// Stable order: keeps the cached prompt prefix identical across turns.
pub fn all() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(read::Read),
        Box::new(edit::Edit),
        Box::new(write::Write),
        Box::new(glob::Glob),
        Box::new(grep::Grep),
        Box::new(list_dir::ListDir),
    ]
}

fn parse<T: DeserializeOwned>(args: &str) -> Result<T, String> {
    let args = if args.trim().is_empty() { "{}" } else { args };
    serde_json::from_str(args).map_err(|e| format!("invalid arguments: {e}"))
}

/// Joins at most `cap` items, then a `+N more` marker.
fn capped(items: Vec<String>, cap: usize) -> String {
    let more = items.len().saturating_sub(cap);
    let mut out = items.into_iter().take(cap).collect::<Vec<_>>().join("\n");
    if more > 0 {
        out.push_str(&format!("\n+{more} more"));
    }
    out
}

#[cfg(test)]
pub mod tests {
    use std::path::Path;

    use super::*;
    use crate::message::ToolCall;

    /// Approves every write.
    pub struct Yes;

    impl Ui for Yes {
        fn text(&mut self, _: &str) {}
        fn tool_call(&mut self, _: &ToolCall) {}
        fn tool_result(&mut self, _: bool, _: &str) {}
        fn approve(&mut self, _: &Path, _: &str) -> bool {
            true
        }
    }

    pub fn ctx(name: &str, files: &[(&str, &str)]) -> Ctx {
        let root = crate::sandbox::tests::temp(name);
        for (path, content) in files {
            let p = root.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, content).unwrap();
        }
        Ctx::new(Workspace::open(root).unwrap())
    }

    pub fn call(ctx: &mut Ctx, tool: &str, args: serde_json::Value) -> Result<String, String> {
        let t = all().into_iter().find(|t| t.def().name == tool).unwrap();
        t.run(ctx, &mut Yes, &args.to_string())
    }
}
