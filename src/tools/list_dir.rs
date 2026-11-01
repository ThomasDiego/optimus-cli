use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;

use super::{Ctx, Outcome, Tool, capped, parse};
use crate::message::ToolDef;
use crate::sandbox::{TRASH, is_secret};

const CAP: usize = 200;

pub struct ListDir;

#[derive(Deserialize)]
struct Args {
    path: Option<String>,
    depth: Option<usize>,
}

impl Tool for ListDir {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "list_dir",
            description: "Directory tree, dirs end with /. .gitignored entries are marked (ignored) and not expanded; give one as path to list it. Default depth 2.",
            schema: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}, "depth": {"type": "integer"}}
            }),
        }
    }

    fn run(&self, ctx: &mut Ctx, args: &str) -> Result<Outcome, String> {
        let a: Args = parse(args)?;
        let base = ctx.ws.walk_base(a.path.as_deref())?;
        Ok(Outcome::Done(tree(ctx, &base, a.depth.unwrap_or(2).clamp(1, 10), CAP)))
    }
}

/// Indented tree; secrets are marked so the model does not try to read them.
/// Ignored entries (node_modules, dist…) are listed and marked but never
/// entered: hiding them made the model conclude they did not exist.
pub fn tree(ctx: &Ctx, base: &Path, depth: usize, cap: usize) -> String {
    let shown: Arc<HashSet<PathBuf>> = Arc::new(ctx.ws.walker(base).max_depth(Some(depth)).build().flatten().map(|e| e.into_path()).collect());
    let visible = shown.clone();
    let mut all = ctx.ws.walker(base);
    all.standard_filters(false).max_depth(Some(depth)).filter_entry(move |e| {
        let internal = e.file_name() == ".git" || e.file_name() == TRASH;
        e.depth() == 0 || (!internal && e.path().parent().is_some_and(|p| visible.contains(p)))
    });
    let items: Vec<String> = all
        .build()
        .flatten()
        .filter(|e| e.depth() > 0)
        .map(|e| {
            let slash = if e.file_type().is_some_and(|t| t.is_dir()) { "/" } else { "" };
            let outside = e.path_is_symlink() && std::fs::canonicalize(e.path()).map_or(true, |r| !r.starts_with(&ctx.ws.root));
            let mark = match (is_secret(e.path()), outside, shown.contains(e.path())) {
                (true, ..) => " (secret, unreadable)",
                (_, true, _) => " (link outside project, unreachable)",
                (_, _, false) => " (ignored)",
                _ => "",
            };
            format!("{}{}{slash}{mark}", "  ".repeat(e.depth() - 1), e.file_name().to_string_lossy())
        })
        .collect();
    if items.is_empty() { "(empty)".into() } else { capped(items, cap) }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::tools::tests::{call, ctx};

    #[test]
    fn tree_with_depth() {
        let mut c = ctx("ls", &[("src/a/deep.rs", ""), ("z.txt", ""), (".env", "")]);
        assert_eq!(call(&mut c, "list_dir", json!({})).unwrap(), ".env (secret, unreadable)\nsrc/\n  a/\nz.txt");
        assert_eq!(call(&mut c, "list_dir", json!({"path": "src", "depth": 3})).unwrap(), "a/\n  deep.rs");

        // Ignored entries exist for the model: listed once, not expanded.
        std::fs::write(c.ws.root.join(".gitignore"), "node_modules/\n*.log\n").unwrap();
        std::fs::create_dir_all(c.ws.root.join("node_modules/three")).unwrap();
        std::fs::write(c.ws.root.join("debug.log"), "").unwrap();
        let tree = call(&mut c, "list_dir", json!({})).unwrap();
        assert_eq!(tree, ".env (secret, unreadable)\n.gitignore\ndebug.log (ignored)\nnode_modules/ (ignored)\nsrc/\n  a/\nz.txt");

        // A link leading out is named as such, here and when used.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/hosts", c.ws.root.join("src/hosts")).unwrap();
            assert_eq!(call(&mut c, "list_dir", json!({"path": "src", "depth": 1})).unwrap(), "a/\nhosts (link outside project, unreachable)");
            let err = call(&mut c, "read_file", json!({"path": "src/hosts"})).unwrap_err();
            assert_eq!(err, "src/hosts: symlink to outside the project, not reachable");
        }
        assert!(call(&mut c, "read_file", json!({"path": "../x"})).unwrap_err().starts_with("../x: outside the project"));
    }
}
