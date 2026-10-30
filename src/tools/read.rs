use std::fmt::Write as _;

use serde::Deserialize;
use serde_json::json;

use super::{Ctx, FileState, Outcome, Tool, file, parse};
use crate::message::ToolDef;

const LIMIT: usize = 400;
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
            description: "Read text files. Output: line<TAB>text, default 400 lines. Use paths to read several at once.",
            schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "paths": {"type": "array", "items": {"type": "string"}, "description": "several files, whole"},
                    "offset": {"type": "integer", "description": "1-based start line"},
                    "limit": {"type": "integer"}
                }
            }),
        }
    }

    fn run(&self, ctx: &mut Ctx, args: &str) -> Result<Outcome, String> {
        let a: Args = parse(args)?;
        if let Some(path) = &a.path {
            return read_one(ctx, path, a.offset, a.limit).map(Outcome::Done);
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
            .map(|p| format!("==> {p}\n{}", read_one(ctx, p, None, None).unwrap_or_else(|e| format!("error: {e}"))))
            .collect();
        Ok(Outcome::Done(parts.join("\n")))
    }
}

fn read_one(ctx: &mut Ctx, path: &str, offset: Option<usize>, limit: Option<usize>) -> Result<String, String> {
    let rel = ctx.ws.rel(path)?;
    let text = file::read_text(ctx, &rel, path)?;
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    let start = offset.unwrap_or(1).max(1);
    if total > 0 && start > total {
        return Err(format!("offset {start} beyond end ({total} lines)"));
    }
    let end = (start - 1 + limit.unwrap_or(LIMIT).clamp(1, MAX_LIMIT)).min(total);

    let hash = file::hash(text.as_bytes());
    let state = ctx.reads.entry(rel).or_insert(FileState { hash, ranges: vec![] });
    if state.hash != hash {
        *state = FileState { hash, ranges: vec![] };
    } else if state.ranges.contains(&(start, end)) {
        return Ok("unchanged since last read".into());
    }
    state.ranges.push((start, end));

    if total == 0 {
        return Ok("(empty file)".into());
    }
    Ok(render(&lines, start, end, total))
}

fn render(lines: &[&str], start: usize, end: usize, total: usize) -> String {
    let mut out = String::new();
    for (i, line) in lines[start - 1..end].iter().enumerate() {
        let _ = writeln!(out, "{}\t{}", start + i, file::truncate(line, MAX_LINE));
    }
    if start > 1 || end < total {
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
        assert!(out.ends_with("400\tl400\n[1-400/500]"));
        assert_eq!(call(&mut c, "read_file", json!({"path": "./a.txt"})).unwrap(), "unchanged since last read");
        let tail = call(&mut c, "read_file", json!({"path": "a.txt", "offset": 499})).unwrap();
        assert_eq!(tail, "499\tl499\n500\tl500\n[499-500/500]");
        assert!(call(&mut c, "read_file", json!({"path": "bin"})).unwrap_err().contains("binary"));
        assert!(call(&mut c, "read_file", json!({"path": "."})).unwrap_err().contains("list_dir"));
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
