pub mod change;
mod edit;
mod file;
mod fs_ops;
mod git;
mod glob;
mod grep;
mod list_dir;
mod matching;
mod read;
mod run;
mod trash;
mod web_fetch;
mod write;

use std::collections::HashMap;
use std::path::PathBuf;

use serde::de::DeserializeOwned;

use crate::exec::Exec;
use crate::message::ToolDef;
use crate::sandbox::Workspace;
use crate::web::Fetch;

pub use change::Batch;
pub use file::stale;
pub use list_dir::tree;
pub use file::truncate;

/// What the model has seen of a file: guards edits and dedups re-reads.
pub struct FileState {
    pub hash: u64,
    pub ranges: Vec<(usize, usize)>,
}

pub struct Ctx {
    pub ws: Workspace,
    pub reads: HashMap<PathBuf, FileState>,
    pub undo: Vec<Vec<change::Snapshot>>,
    /// Programs the `run` tool may start.
    pub run_allow: Vec<String>,
}

impl Ctx {
    pub fn new(ws: Workspace, run_allow: Vec<String>) -> Self {
        trash::purge(&ws.dir, true);
        Self { ws, reads: HashMap::new(), undo: vec![], run_allow }
    }
}

/// Undo dies with the session, so its parked directories go too.
impl Drop for Ctx {
    fn drop(&mut self) {
        trash::purge(&self.ws.dir, false);
    }
}

/// Tools never write or execute themselves: side effects are returned to the
/// agent, which applies its permission mode first.
pub enum Outcome {
    Done(String),
    Write(Batch),
    Exec(Exec),
    Fetch(Fetch),
}

pub trait Tool {
    fn def(&self) -> ToolDef;
    /// Errors are returned to the model as short actionable messages.
    fn run(&self, ctx: &mut Ctx, args: &str) -> Result<Outcome, String>;
}

/// Stable order: keeps the cached prompt prefix identical across turns.
pub fn all() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(read::Read),
        Box::new(edit::Edit),
        Box::new(write::Write),
        Box::new(fs_ops::Move),
        Box::new(fs_ops::Delete),
        Box::new(glob::Glob),
        Box::new(grep::Grep),
        Box::new(list_dir::ListDir),
        Box::new(run::Run),
        Box::new(git::Git),
        Box::new(web_fetch::WebFetch),
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
    use super::*;

    pub fn ctx(name: &str, files: &[(&str, &str)]) -> Ctx {
        let root = crate::sandbox::tests::temp(name);
        for (path, content) in files {
            let p = root.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, content).unwrap();
        }
        Ctx::new(Workspace::open(root).unwrap(), vec!["cargo".into()])
    }

    /// Runs a tool and applies its write as if approved.
    pub fn call(ctx: &mut Ctx, tool: &str, args: serde_json::Value) -> Result<String, String> {
        let t = all().into_iter().find(|t| t.def().name == tool).unwrap();
        match t.run(ctx, &args.to_string())? {
            Outcome::Done(s) => Ok(s),
            Outcome::Write(batch) => change::apply(ctx, batch),
            Outcome::Exec(ex) => Ok(format!("exec {} {:?}", ex.program, ex.args)),
            Outcome::Fetch(f) => Ok(format!("fetch {}", f.url)),
        }
    }
}
