use globset::Glob;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::{BinaryDetection, SearcherBuilder};
use serde::Deserialize;
use serde_json::json;

use super::{Ctx, Outcome, Tool, file, parse};
use crate::message::ToolDef;
use crate::sandbox::is_secret;

const CAP: usize = 100;
const MAX_LINE: usize = 200;

pub struct Grep;

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum Mode {
    #[default]
    Files,
    Content,
    Count,
}

#[derive(Deserialize)]
struct Args {
    pattern: String,
    path: Option<String>,
    glob: Option<String>,
    #[serde(default)]
    mode: Mode,
    #[serde(default)]
    i: bool,
}

impl Tool for Grep {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "grep",
            description: "Regex search in files. mode: files (default), content (path:line:text), count.",
            schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"},
                    "path": {"type": "string"},
                    "glob": {"type": "string", "description": "file filter, e.g. *.rs"},
                    "mode": {"type": "string", "enum": ["files", "content", "count"]},
                    "i": {"type": "boolean", "description": "case insensitive"}
                },
                "required": ["pattern"]
            }),
        }
    }

    fn run(&self, ctx: &mut Ctx, args: &str) -> Result<Outcome, String> {
        let a: Args = parse(args)?;
        let base = ctx.ws.walk_base(a.path.as_deref())?;
        let matcher = RegexMatcherBuilder::new()
            .case_insensitive(a.i)
            .build(&a.pattern)
            .map_err(|e| format!("invalid regex: {e}"))?;
        let filter = match &a.glob {
            Some(g) => Some(Glob::new(g).map_err(|e| format!("invalid glob: {e}"))?.compile_matcher()),
            None => None,
        };
        let mut searcher = SearcherBuilder::new().binary_detection(BinaryDetection::quit(0)).line_number(true).build();
        let mut out: Vec<String> = vec![];

        for entry in ctx.ws.walker(&base).build().flatten() {
            let path = entry.path();
            if !entry.file_type().is_some_and(|t| t.is_file()) || is_secret(path) {
                continue;
            }
            if let Some(f) = &filter {
                let rel = path.strip_prefix(&base).unwrap_or(path);
                let target = if a.glob.as_deref().is_some_and(|g| g.contains('/')) { rel } else { rel.file_name().map(AsRef::as_ref).unwrap_or(rel) };
                if !f.is_match(target) {
                    continue;
                }
            }
            let shown = ctx.ws.display(path);
            match a.mode {
                Mode::Files => {
                    let mut hit = false;
                    let _ = searcher.search_path(&matcher, path, UTF8(|_, _| { hit = true; Ok(false) }));
                    if hit {
                        out.push(shown);
                    }
                }
                Mode::Count => {
                    let mut n = 0;
                    let _ = searcher.search_path(&matcher, path, UTF8(|_, _| { n += 1; Ok(true) }));
                    if n > 0 {
                        out.push(format!("{shown}:{n}"));
                    }
                }
                Mode::Content => {
                    let _ = searcher.search_path(&matcher, path, UTF8(|ln, line| {
                        out.push(format!("{shown}:{ln}:{}", file::truncate(line.trim_end(), MAX_LINE)));
                        Ok(out.len() <= CAP)
                    }));
                }
            }
            if out.len() > CAP {
                out.truncate(CAP);
                out.push("[truncated, narrow the search]".into());
                break;
            }
        }
        Ok(Outcome::Done(if out.is_empty() { "no match".into() } else { out.join("\n") }))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::tools::tests::{call, ctx};

    #[test]
    fn modes_filters_and_secrets() {
        let mut c = ctx("grep", &[("src/a.rs", "fn main() {}\nfn x() {}\n"), ("b.md", "fn doc"), (".env", "fn=SECRET")]);
        assert_eq!(call(&mut c, "grep", json!({"pattern": "fn"})).unwrap(), "b.md\nsrc/a.rs");
        assert_eq!(call(&mut c, "grep", json!({"pattern": "fn", "glob": "*.rs", "mode": "count"})).unwrap(), "src/a.rs:2");
        assert_eq!(call(&mut c, "grep", json!({"pattern": "X\\(", "i": true, "mode": "content"})).unwrap(), "src/a.rs:2:fn x() {}");
        assert_eq!(call(&mut c, "grep", json!({"pattern": "SECRET"})).unwrap(), "no match");
    }
}
