use std::time::{SystemTime, UNIX_EPOCH};

use globset::GlobBuilder;
use serde::Deserialize;
use serde_json::json;

use super::{Ctx, Outcome, Tool, capped, parse};
use crate::message::ToolDef;
use crate::sandbox::is_secret;

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
            description: "Find files by glob (e.g. **/*.rs), newest first. Skips .gitignored paths unless the pattern starts in one (node_modules/x/*).",
            schema: json!({
                "type": "object",
                "properties": {"pattern": {"type": "string"}, "path": {"type": "string"}},
                "required": ["pattern"]
            }),
        }
    }

    fn run(&self, ctx: &mut Ctx, args: &str) -> Result<Outcome, String> {
        let a: Args = parse(args)?;
        let base = ctx.ws.walk_base(a.path.as_deref())?;
        // A literal leading dir ("node_modules/three/*.json") becomes the walk
        // base: an ignored dir is then searched when explicitly targeted.
        let parts: Vec<&str> = a.pattern.split('/').collect();
        let literal = parts[..parts.len() - 1].iter().take_while(|p| !p.contains(['*', '?', '[', '{'])).count();
        let (base, pattern) = match ctx.ws.walk_base(Some(&base.join(parts[..literal].join("/")).to_string_lossy())) {
            Ok(dir) if literal > 0 && dir.is_dir() => (dir, parts[literal..].join("/")),
            _ => (base, a.pattern.clone()),
        };
        let glob = GlobBuilder::new(&pattern)
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
                let secret = if is_secret(e.path()) { " (secret, unreadable)" } else { "" };
                (mtime, format!("{}{secret}", ctx.ws.display(e.path())))
            })
            .collect();
        if hits.is_empty() {
            return Ok(Outcome::Done("no match".into()));
        }
        hits.sort_by_key(|h| std::cmp::Reverse(h.0));
        Ok(Outcome::Done(capped(hits.into_iter().map(|h| h.1).collect(), CAP)))
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
        std::fs::write(c.ws.root.join(".env"), "S=1").unwrap();
        assert_eq!(call(&mut c, "glob", json!({"pattern": ".env*"})).unwrap(), ".env (secret, unreadable)");

        // Ignored dirs stay out of broad searches but can be targeted.
        std::fs::write(c.ws.root.join(".gitignore"), "target/\nnode_modules/\n").unwrap();
        std::fs::create_dir_all(c.ws.root.join("node_modules/three")).unwrap();
        std::fs::write(c.ws.root.join("node_modules/three/package.json"), "{}").unwrap();
        assert_eq!(call(&mut c, "glob", json!({"pattern": "**/package.json"})).unwrap(), "no match");
        let hit = "node_modules/three/package.json";
        assert_eq!(call(&mut c, "glob", json!({"pattern": hit})).unwrap(), hit);
        assert_eq!(call(&mut c, "glob", json!({"pattern": "node_modules/*/package.json"})).unwrap(), hit);
        assert_eq!(call(&mut c, "list_dir", json!({"path": "node_modules"})).unwrap(), "three/\n  package.json");
        // A prefix escaping the project is not a base: plain match, nothing found.
        assert_eq!(call(&mut c, "glob", json!({"pattern": "../*/x"})).unwrap(), "no match");
        assert!(call(&mut c, "glob", json!({"pattern": "*", "path": ".."})).is_err());
    }
}
