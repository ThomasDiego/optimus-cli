use serde::Deserialize;
use serde_json::json;

use super::{Change, Ctx, Outcome, Tool, file, parse};
use crate::message::ToolDef;

pub struct Write;

#[derive(Deserialize)]
struct Args {
    path: String,
    content: String,
}

impl Tool for Write {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "write_file",
            description: "Create or overwrite a file. Prefer edit_file for changes.",
            schema: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}, "content": {"type": "string"}},
                "required": ["path", "content"]
            }),
        }
    }

    fn run(&self, ctx: &mut Ctx, args: &str) -> Result<Outcome, String> {
        let a: Args = parse(args)?;
        let rel = ctx.ws.rel_write(&a.path)?;
        let exists = ctx.ws.dir.try_exists(&rel).map_err(|e| file::io_err(&a.path, e))?;
        let old = if exists {
            let current = file::read_text(ctx, &rel, &a.path)?;
            file::ensure_fresh(ctx, &rel, &current, &a.path)?;
            Some(current)
        } else {
            None
        };
        let ok = format!("ok {}L", a.content.lines().count());
        Ok(Outcome::Write(Change { rel, old, new: a.content, ok }))
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
}
