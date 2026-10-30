use std::time::{SystemTime, UNIX_EPOCH};

use globset::GlobBuilder;
use serde::Deserialize;
use serde_json::json;

use super::{Ctx, Tool, capped, parse};
use crate::message::ToolDef;
use crate::ui::Ui;

const CAP: usize = 100;

pub struct Glob;

#[derive(Deserialize)]
struct Args {
    pattern: String,
    path: Option<String>,
}

impl Tool for Glob {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "glob",
            description: "Find files by glob (e.g. **/*.rs), newest first. Respects .gitignore.",
            schema: json!({
                "type": "object",
                "properties": {"pattern": {"type": "string"}, "path": {"type": "string"}},
                "required": ["pattern"]
            }),
        }
    }

    fn run(&self, ctx: &mut Ctx, _: &mut dyn Ui, args: &str) -> Result<String, String> {
        let a: Args = parse(args)?;
        let base = ctx.ws.walk_base(a.path.as_deref())?;
        let glob = GlobBuilder::new(&a.pattern)
            .literal_separator(true)
            .build()
            .map_err(|e| format!("invalid pattern: {e}"))?
            .compile_matcher();
        let mut hits: Vec<(SystemTime, String)> = ctx
            .ws
            .walker(&base)
            .build()
            .flatten()
            .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
            .filter(|e| e.path().strip_prefix(&base).is_ok_and(|p| glob.is_match(p)))
            .map(|e| {
                let mtime = e.metadata().ok().and_then(|m| m.modified().ok()).unwrap_or(UNIX_EPOCH);
                (mtime, ctx.ws.display(e.path()))
            })
            .collect();
        if hits.is_empty() {
            return Ok("no match".into());
        }
        hits.sort_by_key(|h| std::cmp::Reverse(h.0));
        Ok(capped(hits.into_iter().map(|h| h.1).collect(), CAP))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::tools::tests::{call, ctx};

    #[test]
    fn matches_and_respects_gitignore() {
        let mut c = ctx("glob", &[(".gitignore", "target/\n"), ("src/a.rs", ""), ("target/b.rs", ""), ("c.md", "")]);
        assert_eq!(call(&mut c, "glob", json!({"pattern": "**/*.rs"})).unwrap(), "src/a.rs");
        assert_eq!(call(&mut c, "glob", json!({"pattern": "*.zz"})).unwrap(), "no match");
        assert!(call(&mut c, "glob", json!({"pattern": "*", "path": ".."})).is_err());
    }
}
