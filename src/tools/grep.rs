use globset::Glob;
use grep_regex::RegexMatcherBuilder;
use std::io;

use grep_searcher::sinks::UTF8;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use serde::Deserialize;
use serde_json::json;

use super::{Ctx, FileState, Outcome, Tool, file, parse};
use crate::message::ToolDef;
use crate::sandbox::is_secret;

const CAP: usize = 100;
/// A directory search without a mode shows the matching lines when they fit
/// in this many output lines, file names otherwise: saves the read that
/// follows a short file list.
const AUTO_LINES: usize = 60;
const MAX_LINE: usize = 200;
const MAX_CONTEXT: usize = 10;

pub struct Grep;

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Files,
    Content,
    Count,
}

#[derive(Deserialize)]
struct Args {
    pattern: String,
    path: Option<String>,
    glob: Option<String>,
    mode: Option<Mode>,
    #[serde(default)]
    i: bool,
    #[serde(default)]
    context: usize,
}

/// Content-mode output: the path once per file, then `line:text` for matches
/// and `line-text` for context, `--` between separate groups.
struct Lines<'a> {
    out: &'a mut Vec<String>,
}

impl Lines<'_> {
    fn push(&mut self, ln: Option<u64>, bytes: &[u8], sep: char) {
        let text = String::from_utf8_lossy(bytes);
        self.out.push(format!("{}{sep}{}", ln.unwrap_or(0), file::truncate(text.trim_end(), MAX_LINE)));
    }
}

impl Sink for Lines<'_> {
    type Error = io::Error;

    fn matched(&mut self, _: &Searcher, m: &SinkMatch<'_>) -> Result<bool, io::Error> {
        self.push(m.line_number(), m.bytes(), ':');
        Ok(self.out.len() <= CAP)
    }

    fn context(&mut self, _: &Searcher, c: &SinkContext<'_>) -> Result<bool, io::Error> {
        self.push(c.line_number(), c.bytes(), '-');
        Ok(true)
    }

    fn context_break(&mut self, _: &Searcher) -> Result<bool, io::Error> {
        self.out.push("--".into());
        Ok(true)
    }
}

impl Tool for Grep {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "grep",
            description: "Regex search in files. mode: content (path, then line:text per match; context adds surrounding line-text, saving a read_file), files, count (per file; tally values with python3 -c). Default: content when it is short, else files.",
            schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"},
                    "path": {"type": "string"},
                    "glob": {"type": "string", "description": "file filter, e.g. *.rs"},
                    "mode": {"type": "string", "enum": ["files", "content", "count"]},
                    "i": {"type": "boolean", "description": "case insensitive"},
                    "context": {"type": "integer", "description": "content: lines around each match, max 10"}
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
        // No mode: content, falling back to file names past AUTO_LINES (a single
        // file always shows its lines: listing its name answers nothing).
        let auto = a.mode.is_none() && !base.is_file();
        let mode = a.mode.unwrap_or(Mode::Content);
        let (mut names, mut shown_files, mut overflow, mut sample) = (vec![], vec![], false, vec![]);
        let around = a.context.min(MAX_CONTEXT);
        let mut searcher = SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(0))
            .line_number(true)
            .before_context(around)
            .after_context(around)
            .build();
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
            match mode {
                Mode::Files | Mode::Content if overflow => {
                    let mut hit = false;
                    let _ = searcher.search_path(&matcher, path, UTF8(|_, _| { hit = true; Ok(false) }));
                    if hit {
                        names.push(shown);
                    }
                }
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
                    let mut lines = vec![];
                    let _ = searcher.search_path(&matcher, path, Lines { out: &mut lines });
                    if !lines.is_empty() {
                        names.push(shown.clone());
                        shown_files.push(path.to_path_buf());
                        out.push(shown);
                        out.append(&mut lines);
                    }
                }
            }
            // Too long for auto content: keep walking, for the names only.
            if auto && !overflow && out.len() > AUTO_LINES {
                overflow = true;
                // A few first matches show the pattern, often enough to skip a read.
                sample = out.drain(..).take(6).collect();
                shown_files.clear();
                continue;
            }
            if out.len() > CAP {
                out.truncate(CAP);
                out.push("[truncated: narrow the search, or tally with python3 -c]".into());
                break;
            }
        }
        if overflow {
            return Ok(Outcome::Done(format!("{}\n[first matches]\n{}", super::capped(names, CAP), sample.join("\n"))));
        }
        // Only lines actually returned count as seen.
        for path in shown_files {
            seen(ctx, &path);
        }
        Ok(Outcome::Done(if out.is_empty() { "no match".into() } else { out.join("\n") }))
    }
}

/// Matched lines count as seen: edits on them need no read_file first, while
/// a later outside change is still caught by the hash.
fn seen(ctx: &mut Ctx, path: &std::path::Path) {
    let Ok(rel) = path.strip_prefix(&ctx.ws.root).map(std::path::Path::to_path_buf) else { return };
    if ctx.reads.contains_key(&rel) {
        return;
    }
    if let Ok(text) = file::read_text(ctx, &rel, "") {
        ctx.reads.insert(rel, FileState { hash: file::hash(text.as_bytes()), ranges: vec![] });
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::tools::tests::{call, ctx};

    #[test]
    fn modes_filters_and_secrets() {
        let mut c = ctx("grep", &[("src/a.rs", "fn main() {}\nfn x() {}\n"), ("b.md", "fn doc"), (".env", "fn=SECRET")]);
        assert_eq!(call(&mut c, "grep", json!({"pattern": "fn"})).unwrap(), "b.md\n1:fn doc\nsrc/a.rs\n1:fn main() {}\n2:fn x() {}");
        assert_eq!(call(&mut c, "grep", json!({"pattern": "fn", "mode": "files"})).unwrap(), "b.md\nsrc/a.rs");
        assert_eq!(call(&mut c, "grep", json!({"pattern": "fn", "glob": "*.rs", "mode": "count"})).unwrap(), "src/a.rs:2");
        assert_eq!(call(&mut c, "grep", json!({"pattern": "X\\(", "i": true, "mode": "content"})).unwrap(), "src/a.rs\n2:fn x() {}");
        assert_eq!(call(&mut c, "grep", json!({"pattern": "SECRET"})).unwrap(), "no match");
        assert_eq!(call(&mut c, "grep", json!({"pattern": "x", "path": "src/a.rs"})).unwrap(), "src/a.rs\n2:fn x() {}");
        assert_eq!(call(&mut c, "grep", json!({"pattern": "x", "path": "src/a.rs", "mode": "files"})).unwrap(), "src/a.rs");
    }

    #[test]
    fn context_shows_surrounding_lines_once() {
        let body = "a\nb\nTARGET\nc\nd\ne\nf\ng\nTARGET\nh\n";
        let mut c = ctx("grep-ctx", &[("f.txt", body)]);
        let out = call(&mut c, "grep", json!({"pattern": "TARGET", "mode": "content", "context": 1})).unwrap();
        assert_eq!(out, "f.txt\n2-b\n3:TARGET\n4-c\n--\n8-g\n9:TARGET\n10-h");
    }

    #[test]
    fn long_default_search_falls_back_to_names_unseen() {
        let files: Vec<(String, String)> = (1..=40).map(|i| (format!("h{i:02}.rs"), format!("shout(\"h{i}\")\n"))).collect();
        let refs: Vec<(&str, &str)> = files.iter().map(|(p, b)| (p.as_str(), b.as_str())).collect();
        let mut c = ctx("grep-auto", &refs);
        let out = call(&mut c, "grep", json!({"pattern": "shout"})).unwrap();
        let (names, sample) = out.split_once("\n[first matches]\n").unwrap();
        assert_eq!(names.lines().count(), 40, "{out}");
        assert!(names.starts_with("h01.rs\nh02.rs\n") && !names.contains(':'), "{out}");
        assert_eq!(sample, "h01.rs\n1:shout(\"h1\")\nh02.rs\n1:shout(\"h2\")\nh03.rs\n1:shout(\"h3\")");
        assert!(c.reads.is_empty(), "a sample is not a read: editing still needs one");
    }

    #[test]
    fn matched_files_can_be_edited_without_a_read() {
        let mut c = ctx("grep-seen", &[("a.rs", "old()\n"), ("b.rs", "old()\n")]);
        call(&mut c, "grep", json!({"pattern": "old", "glob": "a.rs", "mode": "content"})).unwrap();
        let edit = |p: &str| json!({"path": p, "edits": [{"old": "old()", "new": "new()"}]});
        assert!(call(&mut c, "edit_file", edit("a.rs")).is_ok());
        assert!(call(&mut c, "edit_file", edit("b.rs")).unwrap_err().contains("not read yet"));
    }
}
