use serde::Deserialize;
use serde_json::json;

use super::change::Change;
use super::{Batch, Ctx, Outcome, Tool, file, parse};
use crate::message::ToolDef;

/// Moves or renames a file; the destination must not exist.
pub struct Move;

/// Deletes a file. Both are undoable: the old content is snapshotted.
pub struct Delete;

#[derive(Deserialize)]
struct MoveArgs {
    from: String,
    to: String,
}

#[derive(Deserialize)]
struct DeleteArgs {
    path: String,
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
            Change { rel: to, old: None, new: Some(bytes.clone()) },
            Change { rel: from, old: Some(bytes), new: None },
        ];
        // The model did not author this content: it must read before editing.
        Ok(Outcome::Write(Batch { changes, ok: format!("moved {} → {}", a.from, a.to), known: false }))
    }
}

impl Tool for Delete {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "delete_file",
            description: "Delete a file (not a directory). Undoable by the user.",
            schema: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
        }
    }

    fn run(&self, ctx: &mut Ctx, args: &str) -> Result<Outcome, String> {
        let a: DeleteArgs = parse(args)?;
        let rel = ctx.ws.rel_write(&a.path)?;
        let bytes = file::read_bytes(ctx, &rel, &a.path)?;
        Ok(Outcome::Write(Batch::single(rel, Some(bytes), None, format!("deleted {}", a.path))))
    }
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
        assert!(call(&mut c, "edit_file", edit).unwrap_err().contains("read the file"));

        change::begin_turn(&mut c);
        change::undo(&mut c).unwrap();
        assert!(root.join("src/a.rs").exists() && !root.join("src/sub/b.rs").exists());
        assert_eq!(std::fs::read(root.join("img.png")).unwrap(), b"\x00\x01bin");
    }

    #[test]
    fn guards_destination_dirs_secrets_and_git() {
        let mut c = ctx("fsops-guard", &[("a.txt", "a"), ("b.txt", "b"), ("d/x.txt", "x"), (".env", "S=1")]);
        assert!(call(&mut c, "move_file", json!({"from": "a.txt", "to": "b.txt"})).unwrap_err().contains("exists"));
        assert!(call(&mut c, "delete_file", json!({"path": "d"})).unwrap_err().contains("directory"));
        assert!(call(&mut c, "move_file", json!({"from": ".env", "to": "leak.txt"})).is_err());
        assert!(call(&mut c, "move_file", json!({"from": "a.txt", "to": ".git/hooks/pre-commit"})).is_err());
        assert!(call(&mut c, "delete_file", json!({"path": "../outside"})).is_err());
    }
}
