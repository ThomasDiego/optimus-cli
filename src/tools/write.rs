use serde::Deserialize;
use serde_json::json;

use super::change::{Change, Kind};
use super::{Batch, Ctx, Outcome, Tool, file, parse};
use crate::message::ToolDef;

pub struct Write;

const MAX_FILES: usize = 20;

#[derive(Deserialize)]
struct Args {
    path: Option<String>,
    content: Option<String>,
    #[serde(default)]
    files: Vec<File>,
}

#[derive(Deserialize)]
struct File {
    path: String,
    content: String,
}

impl Tool for Write {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "write_file",
            description: "Create or overwrite files, all or none. Prefer edit_file for changes.",
            schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"},
                    "files": {
                        "type": "array",
                        "description": "several at once",
                        "items": {"type": "object", "properties": {"path": {"type": "string"}, "content": {"type": "string"}}, "required": ["path", "content"]}
                    }
                }
            }),
        }
    }

    fn run(&self, ctx: &mut Ctx, args: &str) -> Result<Outcome, String> {
        let a: Args = parse(args)?;
        let single = a.path.zip(a.content).map(|(path, content)| File { path, content });
        let files: Vec<File> = single.into_iter().chain(a.files).collect();
        if files.is_empty() || files.len() > MAX_FILES {
            return Err(format!("give path and content, or 1 to {MAX_FILES} files"));
        }
        let (mut changes, mut done) = (vec![], vec![]);
        for f in files {
            let rel = ctx.ws.rel_write(&f.path)?;
            if changes.iter().any(|c: &Change| c.rel == rel) {
                return Err(format!("{}: given twice", f.path));
            }
            let exists = ctx.ws.dir.try_exists(&rel).map_err(|e| file::io_err(&f.path, e))?;
            let old = if exists {
                let current = file::read_text(ctx, &rel, &f.path)?;
                file::ensure_fresh(ctx, &rel, &current, &f.path)?;
                Some(current.into_bytes())
            } else {
                None
            };
            done.push((f.path, f.content.lines().count()));
            changes.push(Change { rel, old, new: Some(f.content.into_bytes()), kind: Kind::File });
        }
        let ok = match done.as_slice() {
            [(_, n)] => format!("ok {n}L"),
            many => format!("ok {}", many.iter().map(|(p, n)| format!("{p} {n}L")).collect::<Vec<_>>().join(", ")),
        };
        Ok(Outcome::Write(Batch { changes, ok, known: true }))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::tools::tests::{call, ctx};

    #[test]
    fn creates_nested_and_guards_overwrite() {
        let mut c = ctx("write", &[("old.txt", "x")]);
        assert_eq!(call(&mut c, "write_file", json!({"path": "src/new.rs", "content": "a\nb\n"})).unwrap(), "ok 2L");
        assert_eq!(std::fs::read_to_string(c.ws.root.join("src/new.rs")).unwrap(), "a\nb\n");
        assert!(call(&mut c, "write_file", json!({"path": "old.txt", "content": "y"})).is_err());
        assert!(call(&mut c, "write_file", json!({"path": "../evil", "content": "y"})).is_err());
        assert!(call(&mut c, "write_file", json!({"path": ".git/hooks/pre-commit", "content": "y"})).is_err());
    }

    #[test]
    fn writes_several_all_or_none() {
        let mut c = ctx("write-many", &[("old.txt", "x")]);
        let files = json!({"files": [{"path": "a/Cargo.toml", "content": "[a]\n"}, {"path": "b/src/lib.rs", "content": "x\ny\n"}]});
        assert_eq!(call(&mut c, "write_file", files).unwrap(), "ok a/Cargo.toml 1L, b/src/lib.rs 2L");
        assert!(c.ws.root.join("b/src/lib.rs").exists());
        // One bad path (unread file) refuses the whole batch.
        let bad = json!({"files": [{"path": "c.rs", "content": "c"}, {"path": "old.txt", "content": "y"}]});
        assert!(call(&mut c, "write_file", bad).is_err());
        assert!(!c.ws.root.join("c.rs").exists());
        let twice = json!({"files": [{"path": "d.rs", "content": "1"}, {"path": "./d.rs", "content": "2"}]});
        assert!(call(&mut c, "write_file", twice).unwrap_err().contains("twice"));
    }
}
