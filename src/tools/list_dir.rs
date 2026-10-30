use serde::Deserialize;
use serde_json::json;

use super::{Ctx, Outcome, Tool, capped, parse};
use crate::message::ToolDef;

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
            description: "Directory tree, dirs end with /. Respects .gitignore. Default depth 2.",
            schema: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}, "depth": {"type": "integer"}}
            }),
        }
    }

    fn run(&self, ctx: &mut Ctx, args: &str) -> Result<Outcome, String> {
        let a: Args = parse(args)?;
        let base = ctx.ws.walk_base(a.path.as_deref())?;
        let depth = a.depth.unwrap_or(2).clamp(1, 10);
        let items: Vec<String> = ctx
            .ws
            .walker(&base)
            .max_depth(Some(depth))
            .build()
            .flatten()
            .filter(|e| e.depth() > 0)
            .map(|e| {
                let slash = if e.file_type().is_some_and(|t| t.is_dir()) { "/" } else { "" };
                format!("{}{}{slash}", "  ".repeat(e.depth() - 1), e.file_name().to_string_lossy())
            })
            .collect();
        Ok(Outcome::Done(if items.is_empty() { "(empty)".into() } else { capped(items, CAP) }))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::tools::tests::{call, ctx};

    #[test]
    fn tree_with_depth() {
        let mut c = ctx("ls", &[("src/a/deep.rs", ""), ("z.txt", "")]);
        assert_eq!(call(&mut c, "list_dir", json!({})).unwrap(), "src/\n  a/\nz.txt");
        assert_eq!(call(&mut c, "list_dir", json!({"path": "src", "depth": 3})).unwrap(), "a/\n  deep.rs");
    }
}
