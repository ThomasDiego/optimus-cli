use grep_regex::RegexMatcher;
use grep_searcher::SearcherBuilder;
use grep_searcher::sinks::UTF8;
use serde::Deserialize;
use serde_json::json;

use super::{Ctx, Outcome, Tool, file, parse};
use crate::message::ToolDef;

/// Declarations of a file with their line numbers: the shape of a module for
/// a fraction of the tokens of reading it.
pub struct Outline;

const MAX_PATHS: usize = 10;
const MAX_LINE: usize = 160;
const CAP: usize = 300;

/// Declaration lines per extension; indentation is kept, so nesting shows.
const LANGS: &[(&[&str], &str)] = &[
    (&["rs"], r"^\s*(pub(\([^)]*\))?\s+)?((async|const|unsafe|extern)\s+)*(fn|struct|enum|union|trait|impl|mod|type|static|macro_rules!)\b"),
    (
        &["js", "jsx", "ts", "tsx", "mjs", "cjs"],
        r"^\s*(export\s+(default\s+)?)?((async\s+)?function\b|class\b|interface\b|type\s+\w+|enum\b|(const|let)\s+\w+\s*=\s*(async\s*)?(\([^)]*\)|\w+)\s*=>)",
    ),
    (&["py"], r"^\s*(async\s+)?(def|class)\s"),
    (&["go"], r"^(func|type)\s"),
];

#[derive(Deserialize)]
struct Args {
    path: Option<String>,
    #[serde(default)]
    paths: Vec<String>,
}

impl Tool for Outline {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "outline",
            description: "Declarations (fn, struct, class, def…) of source files with line numbers, without bodies: see a file's API before reading it. rs, js/ts, py, go.",
            schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "paths": {"type": "array", "items": {"type": "string"}, "description": "several files"}
                }
            }),
        }
    }

    fn run(&self, ctx: &mut Ctx, args: &str) -> Result<Outcome, String> {
        let a: Args = parse(args)?;
        let list: Vec<String> = a.path.into_iter().chain(a.paths).collect();
        if list.is_empty() || list.len() > MAX_PATHS {
            return Err(format!("give path or 1 to {MAX_PATHS} paths"));
        }
        let several = list.len() > 1;
        let parts: Vec<String> = list
            .iter()
            .map(|p| {
                let body = one(ctx, p).unwrap_or_else(|e| format!("error: {e}"));
                if several { format!("==> {p}\n{body}") } else { body }
            })
            .collect();
        Ok(Outcome::Done(parts.join("\n")))
    }
}

fn one(ctx: &Ctx, path: &str) -> Result<String, String> {
    let rel = ctx.ws.rel(path)?;
    let ext = rel.extension().and_then(|e| e.to_str()).unwrap_or("");
    let (_, pattern) = LANGS.iter().find(|(exts, _)| exts.contains(&ext)).ok_or_else(|| format!("{path}: no outline for .{ext} files; use grep"))?;
    let matcher = RegexMatcher::new_line_matcher(pattern).map_err(|e| e.to_string())?;
    // Read through the capability handle like read_file, then search in memory.
    let text = file::read_text(ctx, &rel, path)?;
    let mut out = vec![];
    SearcherBuilder::new()
        .line_number(true)
        .build()
        .search_slice(&matcher, text.as_bytes(), UTF8(|ln, line| {
            let decl = line.trim_end().trim_end_matches('{').trim_end();
            out.push(format!("{ln}:{}", file::truncate(decl, MAX_LINE)));
            Ok(out.len() < CAP)
        }))
        .map_err(|e| file::io_err(path, e))?;
    Ok(if out.is_empty() { "(no declarations)".into() } else { out.join("\n") })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::tools::tests::{call, ctx};

    #[test]
    fn lists_declarations_per_language() {
        let rs = "pub mod utils;\n\n/// Adds.\npub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\nimpl Foo {\n    pub(crate) async fn go(&self) {}\n}\n";
        let ts = "import x from 'y';\nexport default function App() {\n}\nexport const sum = (a, b) => a + b;\nclass Store {}\n";
        let py = "import os\n\nclass A:\n    def run(self):\n        pass\n";
        let mut c = ctx("outline", &[("a.rs", rs), ("b.tsx", ts), ("c.py", py), ("d.txt", "x"), (".env", "S=1")]);
        let rs_out = "1:pub mod utils;\n4:pub fn add(a: i32, b: i32) -> i32\n7:impl Foo\n8:    pub(crate) async fn go(&self) {}";
        assert_eq!(call(&mut c, "outline", json!({"path": "a.rs"})).unwrap(), rs_out);
        let both = call(&mut c, "outline", json!({"paths": ["b.tsx", "c.py"]})).unwrap();
        assert_eq!(both, "==> b.tsx\n2:export default function App()\n4:export const sum = (a, b) => a + b;\n5:class Store {}\n==> c.py\n3:class A:\n4:    def run(self):");
        assert!(call(&mut c, "outline", json!({"path": "d.txt"})).unwrap().contains("use grep"));
        assert!(call(&mut c, "outline", json!({"path": ".env"})).unwrap().contains("secret"));
    }
}
