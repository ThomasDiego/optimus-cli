use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::json;

use super::change::{Change, Kind};
use super::{Batch, Ctx, Outcome, Tool, file, parse, trash};
use crate::message::ToolDef;

/// Moves or renames a file; the destination must not exist.
pub struct Move;

/// Deletes files and directories, all undoable: file content is snapshotted,
/// directories with contents are parked in the trash.
pub struct Delete;

/// Paths per delete call, and entries scanned before a directory counts as
/// non-empty (and goes to the trash rather than being removed entry by entry).
const MAX_PATHS: usize = 50;
const MAX_SCAN: usize = 1000;

#[derive(Deserialize)]
struct MoveArgs {
    from: String,
    to: String,
}

#[derive(Deserialize)]
struct DeleteArgs {
    path: Option<String>,
    #[serde(default)]
    paths: Vec<String>,
}

impl Tool for Move {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "move_file",
            description: "Move or rename a file (parent dirs are created). Fails if the destination exists.",
            schema: json!({
                "type": "object",
                "properties": {"from": {"type": "string"}, "to": {"type": "string"}},
                "required": ["from", "to"]
            }),
        }
    }

    fn run(&self, ctx: &mut Ctx, args: &str) -> Result<Outcome, String> {
        let a: MoveArgs = parse(args)?;
        let (from, to) = (ctx.ws.rel_write(&a.from)?, ctx.ws.rel_write(&a.to)?);
        if ctx.ws.dir.try_exists(&to).map_err(|e| file::io_err(&a.to, e))? {
            return Err(format!("{}: exists; delete it first or pick another name", a.to));
        }
        let bytes = file::read_bytes(ctx, &from, &a.from)?;
        let changes = vec![
            Change { rel: to, old: None, new: Some(bytes.clone()), kind: Kind::File },
            Change { rel: from, old: Some(bytes), new: None, kind: Kind::File },
        ];
        // The model did not author this content: it must read before editing.
        Ok(Outcome::Write(Batch { changes, ok: format!("moved {} → {}", a.from, a.to), known: false }))
    }
}

impl Tool for Delete {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "delete_file",
            description: "Delete files or directories, contents included (e.g. node_modules). All or nothing; undoable by the user.",
            schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "paths": {"type": "array", "items": {"type": "string"}, "description": "several at once"}
                }
            }),
        }
    }

    fn run(&self, ctx: &mut Ctx, args: &str) -> Result<Outcome, String> {
        let a: DeleteArgs = parse(args)?;
        let mut list: Vec<String> = a.path.into_iter().chain(a.paths).collect();
        list.sort();
        list.dedup();
        if list.is_empty() || list.len() > MAX_PATHS {
            return Err(format!("give path or 1 to {MAX_PATHS} paths"));
        }
        let (mut files, mut dirs, mut done, mut skipped) = (vec![], vec![], vec![], vec![]);
        for p in &list {
            // Protected paths are skipped, not fatal: "delete everything" then
            // takes one call instead of one retry per refusal.
            let rel = match ctx.ws.rel_write(p) {
                Ok(rel) if rel == Path::new(".") => {
                    skipped.push("the project root cannot be deleted".to_string());
                    continue;
                }
                Ok(rel) => rel,
                Err(e) if e.contains("secret file") || e.contains("read-only") => {
                    skipped.push(e);
                    continue;
                }
                Err(e) => return Err(e),
            };
            done.push(p.as_str());
            match ctx.ws.dir.symlink_metadata(&rel).map_err(|e| file::io_err(p, e))?.is_dir() {
                true => dirs.push(rel),
                false => files.push(Change { old: Some(file::read_bytes(ctx, &rel, p)?), rel, new: None, kind: Kind::File }),
            }
        }
        // Files first, so a directory emptied by this same call is removed
        // plainly; children before parents, so nothing is parked twice.
        let pending: HashSet<PathBuf> = files.iter().map(|c| c.rel.clone()).collect();
        let mut budget = MAX_SCAN;
        for rel in dirs.into_iter().rev() {
            let mut empty = vec![];
            match empty_dirs(ctx, &rel, &pending, &mut empty, &mut budget) {
                Ok(()) => {
                    for e in empty {
                        if !files.iter().any(|c| c.rel == e.rel) {
                            files.push(e);
                        }
                    }
                }
                Err(_) => {
                    let mut n = 0;
                    trash::count(&ctx.ws.dir, &rel, &mut n).map_err(|e| file::io_err(&rel.display().to_string(), e))?;
                    let kind = Kind::Tree { slot: trash::slot(files.len()), files: n };
                    files.push(Change { rel, old: Some(vec![]), new: None, kind });
                }
            }
        }
        if done.is_empty() {
            return Err(skipped.join("; "));
        }
        let mut ok = format!("deleted {}", done.join(", "));
        if !skipped.is_empty() {
            ok.push_str(&format!("; skipped {}", skipped.join("; ")));
        }
        Ok(Outcome::Write(Batch { changes: files, ok, known: true }))
    }
}

/// Collects `rel` and its subdirectories, deepest first. Fails on the first
/// file not deleted by this call, or past the scan budget.
fn empty_dirs(ctx: &Ctx, rel: &Path, pending: &HashSet<PathBuf>, out: &mut Vec<Change>, budget: &mut usize) -> Result<(), String> {
    let shown = rel.display().to_string();
    for entry in ctx.ws.dir.read_dir(rel).map_err(|e| file::io_err(&shown, e))? {
        let entry = entry.map_err(|e| file::io_err(&shown, e))?;
        *budget = budget.checked_sub(1).ok_or_else(|| format!("{shown}: too many entries to check"))?;
        let child = rel.join(entry.file_name());
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            empty_dirs(ctx, &child, pending, out, budget)?;
        } else if !pending.contains(&child) {
            return Err(format!("{shown}: not empty, contains {}", child.display()));
        }
    }
    if !out.iter().any(|c| c.rel == rel) {
        out.push(Change { rel: rel.to_path_buf(), old: Some(vec![]), new: None, kind: Kind::Dir });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::tools::change;
    use crate::tools::tests::{call, ctx};

    #[test]
    fn moves_deletes_and_undoes_including_binaries() {
        let mut c = ctx("fsops", &[("src/a.rs", "fn a() {}\n"), ("img.png", "\u{0}\u{1}bin"), ("keep.txt", "k")]);
        change::begin_turn(&mut c);
        assert_eq!(call(&mut c, "move_file", json!({"from": "src/a.rs", "to": "src/sub/b.rs"})).unwrap(), "moved src/a.rs → src/sub/b.rs");
        assert_eq!(call(&mut c, "delete_file", json!({"path": "img.png"})).unwrap(), "deleted img.png");
        let root = c.ws.root.clone();
        assert!(!root.join("src/a.rs").exists() && root.join("src/sub/b.rs").exists() && !root.join("img.png").exists());
        // Moved content is not "known": editing requires a read first.
        let edit = json!({"path": "src/sub/b.rs", "edits": [{"old": "a", "new": "b"}]});
        assert!(call(&mut c, "edit_file", edit).unwrap_err().contains("not read yet"));

        change::begin_turn(&mut c);
        change::undo(&mut c).unwrap();
        assert!(root.join("src/a.rs").exists() && !root.join("src/sub/b.rs").exists());
        assert_eq!(std::fs::read(root.join("img.png")).unwrap(), b"\x00\x01bin");
    }

    #[test]
    fn guards_destination_dirs_secrets_and_git() {
        let mut c = ctx("fsops-guard", &[("a.txt", "a"), ("b.txt", "b"), ("d/x.txt", "x"), (".env", "S=1")]);
        assert!(call(&mut c, "move_file", json!({"from": "a.txt", "to": "b.txt"})).unwrap_err().contains("exists"));
        assert!(call(&mut c, "delete_file", json!({"path": ".ai-trash/1"})).unwrap_err().contains("read-only"));
        assert!(call(&mut c, "move_file", json!({"from": "a.txt", "to": ".ai-trash/x"})).is_err());
        assert!(call(&mut c, "move_file", json!({"from": ".env", "to": "leak.txt"})).is_err());
        assert!(call(&mut c, "move_file", json!({"from": "a.txt", "to": ".git/hooks/pre-commit"})).is_err());
        assert!(call(&mut c, "delete_file", json!({"path": "../outside"})).is_err());
        assert!(call(&mut c, "delete_file", json!({"path": "."})).unwrap_err().contains("root"));
        assert!(call(&mut c, "delete_file", json!({"paths": [".env"]})).is_err());
        // Mixed with deletable paths, protected ones are skipped and reported.
        let ok = call(&mut c, "delete_file", json!({"paths": [".", ".env", ".git/HEAD", "b.txt"]})).unwrap();
        assert!(ok.starts_with("deleted b.txt; skipped ") && ok.contains("secret") && ok.contains("read-only"), "{ok}");
        assert!(!c.ws.root.join("b.txt").exists() && c.ws.root.join(".env").exists());
    }

    #[test]
    fn deletes_many_and_empty_dirs_atomically() {
        let mut c = ctx("fsops-many", &[("a.txt", "a"), ("b.txt", "b"), ("d/x.txt", "x"), ("full/keep.txt", "k")]);
        let root = c.ws.root.clone();
        std::fs::create_dir_all(root.join("empty/nested/deeper")).unwrap();
        change::begin_turn(&mut c);

        // A directory emptied by the same call can be removed with its files.
        let ok = call(&mut c, "delete_file", json!({"paths": ["a.txt", "b.txt", "d/x.txt", "d", "empty"]})).unwrap();
        assert_eq!(ok, "deleted a.txt, b.txt, d, d/x.txt, empty");
        assert!(!root.join("a.txt").exists() && !root.join("d").exists() && !root.join("empty").exists());

        change::begin_turn(&mut c);
        change::undo(&mut c).unwrap();
        assert_eq!(std::fs::read_to_string(root.join("d/x.txt")).unwrap(), "x");
        assert!(root.join("a.txt").exists() && root.join("empty/nested/deeper").is_dir());
    }

    #[test]
    fn parks_full_trees_in_a_hidden_trash_and_undoes() {
        let mut c = ctx("fsops-tree", &[("full/keep.txt", "k"), ("full/sub/x.txt", "x"), ("c.txt", "c")]);
        let root = c.ws.root.clone();
        // Past the scan budget, like a real node_modules.
        for i in 0..1200 {
            std::fs::create_dir_all(root.join(format!("node_modules/p{}", i % 7))).unwrap();
            std::fs::write(root.join(format!("node_modules/p{}/f{i}.js", i % 7)), "x").unwrap();
        }
        change::begin_turn(&mut c);
        let ok = call(&mut c, "delete_file", json!({"paths": ["c.txt", "full", "full/sub", "node_modules"]})).unwrap();
        assert_eq!(ok, "deleted c.txt, full, full/sub, node_modules");
        assert!(!root.join("full").exists() && !root.join("node_modules").exists() && !root.join("c.txt").exists());
        // Invisible to the model and to git.
        assert_eq!(call(&mut c, "list_dir", json!({})).unwrap(), "(empty)");
        assert_eq!(std::fs::read_to_string(root.join(".ai-trash/.gitignore")).unwrap(), "*\n");

        change::begin_turn(&mut c);
        change::undo(&mut c).unwrap();
        assert_eq!(std::fs::read_to_string(root.join("full/sub/x.txt")).unwrap(), "x");
        assert!(root.join("node_modules/p3/f3.js").exists() && root.join("c.txt").exists());

        // Deleted again, then the session ends: the trash goes with it.
        call(&mut c, "delete_file", json!({"path": "node_modules"})).unwrap();
        drop(c);
        assert!(!root.join(".ai-trash").exists() && !root.join("node_modules").exists());
    }
}
