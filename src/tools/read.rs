use std::fmt::Write as _;

use serde::Deserialize;
use serde_json::json;

use super::{Ctx, FileState, Outcome, Tool, capped, file, outline, parse};
use crate::message::ToolDef;

const LIMIT: usize = 400;
/// Lines shown at each end of a skimmed file.
const SKIM: usize = 100;
const OUTLINE_CAP: usize = 100;
const MAX_LIMIT: usize = 2000;
const MAX_LINE: usize = 300;
/// Files per batched call: one round trip instead of many.
const MAX_PATHS: usize = 10;

pub struct Read;

#[derive(Deserialize)]
struct Args {
    path: Option<String>,
    #[serde(default)]
    paths: Vec<String>,
    offset: Option<usize>,
    limit: Option<usize>,
}

impl Tool for Read {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "read_file",
            description: "Read text files. Output: line<TAB>text, default 400 lines; without offset/limit, a longer file gives its outline (code) or both ends. Use paths to read several at once.",
            schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "paths": {"type": "array", "items": {"type": "string"}, "description": "several files"},
                    "offset": {"type": "integer", "description": "1-based start line"},
                    "limit": {"type": "integer"}
                }
            }),
        }
    }

    fn run(&self, ctx: &mut Ctx, args: &str) -> Result<Outcome, String> {
        let a: Args = parse(args)?;
        if let Some(path) = &a.path {
            return read_one(ctx, path, a.offset, a.limit, true).map(Outcome::Done);
        }
        if a.paths.is_empty() {
            return Err("path or paths required".into());
        }
        if a.paths.len() > MAX_PATHS {
            return Err(format!("at most {MAX_PATHS} paths per call"));
        }
        // One failing file does not hide the others.
        let parts: Vec<String> = a
            .paths
            .iter()
            .map(|p| format!("==> {p}\n{}", read_one(ctx, p, a.offset, a.limit, true).unwrap_or_else(|e| format!("error: {e}"))))
            .collect();
        Ok(Outcome::Done(parts.join("\n")))
    }
}

/// `skim`: a large file read without a range shows its outline (code) or its
/// two ends (logs end with the crash) instead of its first lines.
pub(super) fn read_one(ctx: &mut Ctx, path: &str, offset: Option<usize>, limit: Option<usize>, skim: bool) -> Result<String, String> {
    let rel = ctx.ws.rel(path)?;
    let text = file::read_text(ctx, &rel, path)?;
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    let skim = skim && offset.is_none() && limit.is_none() && total > LIMIT;
    if skim && let Some(decls) = outline::decls(&rel, &text).filter(|d| !d.is_empty()) {
        // Not recorded as a read: no content was seen.
        return Ok(format!("{}\n[{total} lines, outline only: read the lines you need with offset/limit (max {MAX_LIMIT}), or grep]", capped(decls, OUTLINE_CAP)));
    }
    let start = offset.unwrap_or(1).max(1);
    if total > 0 && start > total {
        return Err(format!("offset {start} beyond end ({total} lines)"));
    }
    let end = (start - 1 + limit.unwrap_or(LIMIT).clamp(1, MAX_LIMIT)).min(total);
    // (0, total) stands for the skimmed view in the dedup ranges.
    let range = if skim { (0, total) } else { (start, end) };

    let hash = file::hash(text.as_bytes());
    let state = ctx.reads.entry(rel).or_insert(FileState { hash, ranges: vec![] });
    if state.hash != hash {
        *state = FileState { hash, ranges: vec![] };
    } else if state.ranges.contains(&range) {
        return Ok("unchanged since last read".into());
    }
    state.ranges.push(range);

    if total == 0 {
        return Ok("(empty file)".into());
    }
    if skim {
        let tail = total - SKIM + 1;
        let how = if total > MAX_LIMIT { "grep it, or compute over it with python3 -c" } else { "read them with offset/limit, or grep" };
        return Ok(format!("{}[{}-{} of {total} lines skipped; {how}]\n{}", numbered(&lines, 1, SKIM), SKIM + 1, tail - 1, numbered(&lines, tail, total)));
    }
    Ok(render(&lines, start, end, total))
}

fn numbered(lines: &[&str], start: usize, end: usize) -> String {
    let mut out = String::new();
    for (i, line) in lines[start - 1..end].iter().enumerate() {
        let _ = writeln!(out, "{}\t{}", start + i, file::truncate(line, MAX_LINE));
    }
    out
}

fn render(lines: &[&str], start: usize, end: usize, total: usize) -> String {
    let mut out = numbered(lines, start, end);
    if total > MAX_LIMIT && end < total {
        let _ = write!(out, "[{start}-{end}/{total}; too long to page: grep it, or compute over it with python3 -c]");
    } else if start > 1 || end < total {
        let _ = write!(out, "[{start}-{end}/{total}]");
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::tools::tests::{call, ctx};

    #[test]
    fn numbers_paginates_and_dedups() {
        let body: String = (1..=500).map(|i| format!("l{i}\n")).collect();
        let mut c = ctx("read", &[("a.txt", &body), ("bin", "a\0b")]);
        let out = call(&mut c, "read_file", json!({"path": "a.txt"})).unwrap();
        assert!(out.starts_with("1\tl1\n2\tl2\n"));
        assert!(out.contains("100\tl100\n[101-400 of 500 lines skipped; read them with offset/limit, or grep]\n401\tl401\n"));
        assert!(out.ends_with("500\tl500\n"));
        assert_eq!(call(&mut c, "read_file", json!({"path": "./a.txt"})).unwrap(), "unchanged since last read");
        let page = call(&mut c, "read_file", json!({"path": "a.txt", "limit": 400})).unwrap();
        assert!(page.ends_with("400\tl400\n[1-400/500]"));
        let tail = call(&mut c, "read_file", json!({"path": "a.txt", "offset": 499})).unwrap();
        assert_eq!(tail, "499\tl499\n500\tl500\n[499-500/500]");
        assert_eq!(call(&mut c, "read_file", json!({"path": "bin"})).unwrap_err(), "bin: binary, unknown format (3 bytes)");
        std::fs::write(c.ws.root.join("logo.png"), b"\x89PNG\r\n\x1a\n\0\0").unwrap();
        assert_eq!(call(&mut c, "read_file", json!({"path": "logo.png"})).unwrap_err(), "logo.png: binary, PNG image (10 bytes)");
        assert!(call(&mut c, "read_file", json!({"path": "."})).unwrap_err().contains("list_dir"));
    }

    #[test]
    fn large_code_file_gives_its_outline() {
        let body: String = (1..=150).map(|i| format!("fn f{i}() {{\n    {i}\n}}\n")).collect();
        let mut c = ctx("read-outline", &[("big.rs", &body)]);
        let out = call(&mut c, "read_file", json!({"path": "big.rs"})).unwrap();
        assert!(out.starts_with("1:fn f1()\n4:fn f2()\n"));
        assert!(out.ends_with("298:fn f100()\n+50 more\n[450 lines, outline only: read the lines you need with offset/limit (max 2000), or grep]"));
        assert!(!c.reads.contains_key(std::path::Path::new("big.rs")));
        let range = call(&mut c, "read_file", json!({"paths": ["big.rs"], "offset": 449})).unwrap();
        assert_eq!(range, "==> big.rs\n449\t    150\n450\t}\n[449-450/450]");
    }

    #[test]
    fn reads_several_files_in_one_call() {
        let mut c = ctx("read-many", &[("a.rs", "a\n"), ("b.rs", "b\n")]);
        let out = call(&mut c, "read_file", json!({"paths": ["a.rs", "missing.rs", "b.rs"]})).unwrap();
        assert_eq!(out, "==> a.rs\n1\ta\n\n==> missing.rs\nerror: missing.rs: not found\n==> b.rs\n1\tb\n");
        assert!(c.reads.contains_key(std::path::Path::new("b.rs")));
        assert!(call(&mut c, "read_file", json!({})).unwrap_err().contains("path or paths"));
    }
}
