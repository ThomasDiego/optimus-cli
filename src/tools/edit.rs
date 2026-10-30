use serde::Deserialize;
use serde_json::json;
use similar::{DiffTag, TextDiff};

use super::change::{self, Kind};
use super::{Batch, Ctx, Outcome, Tool, file, matching, parse};
use crate::message::ToolDef;

pub struct Edit;

const MAX_FILES: usize = 20;

#[derive(Deserialize)]
struct Args {
    path: Option<String>,
    #[serde(default)]
    edits: Vec<Change>,
    #[serde(default)]
    files: Vec<FileEdits>,
}

#[derive(Deserialize)]
struct FileEdits {
    path: String,
    edits: Vec<Change>,
}

#[derive(Deserialize)]
struct Change {
    old: String,
    new: String,
    #[serde(default)]
    all: bool,
}

impl Tool for Edit {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "edit_file",
            description: "Replace exact text in files. Edits apply in order, all or none. Batch edits in one call, several files with files.",
            schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "edits": edits_schema(),
                    "files": {
                        "type": "array",
                        "description": "several files at once",
                        "items": {"type": "object", "properties": {"path": {"type": "string"}, "edits": edits_schema()}, "required": ["path", "edits"]}
                    }
                }
            }),
        }
    }

    fn run(&self, ctx: &mut Ctx, args: &str) -> Result<Outcome, String> {
        let a: Args = parse(args)?;
        let single = a.path.map(|path| FileEdits { path, edits: a.edits });
        let files: Vec<FileEdits> = single.into_iter().chain(a.files).collect();
        if files.is_empty() || files.len() > MAX_FILES {
            return Err(format!("give path and edits, or 1 to {MAX_FILES} files"));
        }
        let many = files.len() > 1;
        let (mut changes, mut done) = (vec![], vec![]);
        for f in files {
            // Errors name the file once several are edited together.
            let tag = |e: String| if many { format!("{}: {e}", f.path) } else { e };
            if f.edits.is_empty() {
                return Err(tag("edits is empty".into()));
            }
            let rel = ctx.ws.rel_write(&f.path)?;
            if changes.iter().any(|c: &change::Change| c.rel == rel) {
                return Err(format!("{}: given twice; put all its edits in one entry", f.path));
            }
            let current = file::read_text(ctx, &rel, &f.path)?;
            file::ensure_fresh(ctx, &rel, &current, &f.path)?;
            let updated = apply(&current, &f.edits).map_err(tag)?;
            if updated == current {
                return Err(tag("no change".into()));
            }
            done.push(if many { format!("{} {}", f.path, summary(&current, &updated)) } else { summary(&current, &updated) });
            changes.push(change::Change { rel, old: Some(current.into_bytes()), new: Some(updated.into_bytes()), kind: Kind::File });
        }
        Ok(Outcome::Write(Batch { changes, ok: done.join("; "), known: true }))
    }
}

fn edits_schema() -> serde_json::Value {
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "old": {"type": "string", "description": "exact current text"},
                "new": {"type": "string"},
                "all": {"type": "boolean", "description": "replace every occurrence"}
            },
            "required": ["old", "new"]
        }
    })
}

fn apply(content: &str, edits: &[Change]) -> Result<String, String> {
    let crlf = content.contains("\r\n");
    let mut text = content.to_string();
    for (k, e) in edits.iter().enumerate() {
        let (old, new) = if crlf { (to_crlf(&e.old), to_crlf(&e.new)) } else { (e.old.clone(), e.new.clone()) };
        let tag = if edits.len() > 1 { format!("edit {}: ", k + 1) } else { String::new() };
        if old.is_empty() {
            return Err(format!("{tag}old is empty"));
        }
        let hits: Vec<usize> = text.match_indices(&old).map(|(i, _)| i).collect();
        text = match hits.len() {
            0 => match matching::loose_find(&text, &old).as_slice() {
                [r] => format!("{}{new}{}", &text[..r.start], &text[r.end..]),
                [] => return Err(format!("{tag}old not found{}", matching::closest(&text, &old))),
                many => return Err(ambiguous(&tag, &text, many.iter().map(|r| r.start))),
            },
            1 => text.replacen(&old, &new, 1),
            _ if e.all => text.replace(&old, &new),
            _ => return Err(ambiguous(&tag, &text, hits.into_iter())),
        };
    }
    Ok(text)
}

fn ambiguous(tag: &str, text: &str, starts: impl Iterator<Item = usize>) -> String {
    let at: Vec<String> = starts.map(|i| format!("L{}", matching::line_of(text, i))).collect();
    format!("{tag}{} matches at {}: add context or set all=true", at.len(), at.join(","))
}

fn to_crlf(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// `ok L12-15 L40 (+3/-1)`: changed line ranges in the new file.
fn summary(old: &str, new: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let (mut add, mut del, mut spans) = (0, 0, vec![]);
    for op in diff.ops().iter().filter(|op| op.tag() != DiffTag::Equal) {
        let (o, n) = (op.old_range(), op.new_range());
        del += o.len();
        add += n.len();
        spans.push(if n.len() > 1 { format!("L{}-{}", n.start + 1, n.end) } else { format!("L{}", n.start + 1) });
    }
    spans.truncate(10);
    format!("ok {} (+{add}/-{del})", spans.join(" "))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::tools::tests::{call, ctx};

    #[test]
    fn edit_flow() {
        let mut c = ctx("edit", &[("a.rs", "let a = 1;\nlet b = 1;\nlet c = 1;\n")]);
        let edit = |old: &str, new: &str| json!({"path": "a.rs", "edits": [{"old": old, "new": new}]});
        let err = call(&mut c, "edit_file", edit("a", "z")).unwrap_err();
        assert!(err.starts_with("a.rs: not read yet") && err.contains("1\tlet a = 1;"), "{err}");
        assert_eq!(call(&mut c, "read_file", json!({"path": "a.rs"})).unwrap(), "unchanged since last read");

        let err = call(&mut c, "edit_file", edit("= 1", "= 2")).unwrap_err();
        assert_eq!(err, "3 matches at L1,L2,L3: add context or set all=true");
        assert_eq!(call(&mut c, "edit_file", edit("let b = 1;", "let b = 2;\nlet d = 4;")).unwrap(), "ok L2-3 (+2/-1)");
        // State was refreshed by the edit: a second edit needs no re-read.
        let multi = json!({"path": "a.rs", "edits": [{"old": "= 1", "new": "= 9", "all": true}, {"old": "d", "new": "e"}]});
        assert!(call(&mut c, "edit_file", multi).unwrap().starts_with("ok"));
        let body = std::fs::read_to_string(c.ws.root.join("a.rs")).unwrap();
        assert_eq!(body, "let a = 9;\nlet b = 2;\nlet e = 4;\nlet c = 9;\n");

        let bad = json!({"path": "a.rs", "edits": [{"old": "let a", "new": "x"}, {"old": "nope", "new": "y"}]});
        assert!(call(&mut c, "edit_file", bad).unwrap_err().starts_with("edit 2: old not found"));
        assert_eq!(std::fs::read_to_string(c.ws.root.join("a.rs")).unwrap(), body, "not atomic");

        // Changed behind the model's back: refused, with the current content
        // attached so the retry can succeed without a read_file.
        std::fs::write(c.ws.root.join("a.rs"), "changed").unwrap();
        let err = call(&mut c, "edit_file", edit("changed", "x")).unwrap_err();
        assert!(err.contains("changed since your last read") && err.ends_with("1\tchanged\n"), "{err}");
        assert!(call(&mut c, "edit_file", edit("changed", "x")).unwrap().starts_with("ok"));
    }

    #[test]
    fn edits_several_files_all_or_none() {
        let mut c = ctx("edit-many", &[("lib.rs", "pub mod utils;\n"), ("main.rs", "use utils;\n"), ("x.rs", "x")]);
        call(&mut c, "read_file", json!({"paths": ["lib.rs", "main.rs", "x.rs"]})).unwrap();
        let files = |second: &str| json!({"files": [
            {"path": "lib.rs", "edits": [{"old": "utils", "new": "text"}]},
            {"path": "main.rs", "edits": [{"old": second, "new": "text"}]}
        ]});
        // One miss refuses both, and says which file.
        let err = call(&mut c, "edit_file", files("nope")).unwrap_err();
        assert!(err.starts_with("main.rs: old not found"), "{err}");
        assert_eq!(std::fs::read_to_string(c.ws.root.join("lib.rs")).unwrap(), "pub mod utils;\n");
        assert_eq!(call(&mut c, "edit_file", files("utils")).unwrap(), "lib.rs ok L1 (+1/-1); main.rs ok L1 (+1/-1)");
        assert_eq!(std::fs::read_to_string(c.ws.root.join("main.rs")).unwrap(), "use text;\n");
    }

    #[test]
    fn reports_stale_reads_once() {
        let mut c = ctx("stale", &[("a.rs", "a"), ("b.rs", "b"), ("c.rs", "c")]);
        call(&mut c, "read_file", json!({"paths": ["a.rs", "b.rs", "c.rs"]})).unwrap();
        std::fs::write(c.ws.root.join("a.rs"), "edited by the user").unwrap();
        std::fs::remove_file(c.ws.root.join("c.rs")).unwrap();
        assert_eq!(crate::tools::stale(&mut c), ["a.rs", "c.rs"]);
        assert!(crate::tools::stale(&mut c).is_empty());
    }

    #[test]
    fn preserves_crlf_and_tolerates_trailing_spaces() {
        let mut c = ctx("crlf", &[("w.txt", "one  \r\ntwo\r\n")]);
        call(&mut c, "read_file", json!({"path": "w.txt"})).unwrap();
        let args = json!({"path": "w.txt", "edits": [{"old": "one\ntwo", "new": "1\n2"}]});
        call(&mut c, "edit_file", args).unwrap();
        assert_eq!(std::fs::read_to_string(c.ws.root.join("w.txt")).unwrap(), "1\r\n2\r\n");
    }
}
