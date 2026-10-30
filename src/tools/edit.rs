use serde::Deserialize;
use serde_json::json;
use similar::{DiffTag, TextDiff};

use super::change::Change as Pending;
use super::{Ctx, Outcome, Tool, file, matching, parse};
use crate::message::ToolDef;

pub struct Edit;

#[derive(Deserialize)]
struct Args {
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
            description: "Replace exact text in a file. Edits apply in order, all or none. Batch edits in one call.",
            schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "edits": {
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
                    }
                },
                "required": ["path", "edits"]
            }),
        }
    }

    fn run(&self, ctx: &mut Ctx, args: &str) -> Result<Outcome, String> {
        let a: Args = parse(args)?;
        if a.edits.is_empty() {
            return Err("edits is empty".into());
        }
        let rel = ctx.ws.rel_write(&a.path)?;
        let current = file::read_text(ctx, &rel, &a.path)?;
        file::ensure_fresh(ctx, &rel, &current, &a.path)?;
        let updated = apply(&current, &a.edits)?;
        if updated == current {
            return Err("no change".into());
        }
        let ok = summary(&current, &updated);
        Ok(Outcome::Write(Pending { rel, old: Some(current), new: updated, ok }))
    }
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
        assert!(call(&mut c, "edit_file", edit("a", "z")).unwrap_err().contains("read the file"));
        call(&mut c, "read_file", json!({"path": "a.rs"})).unwrap();

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

        std::fs::write(c.ws.root.join("a.rs"), "changed").unwrap();
        assert!(call(&mut c, "edit_file", edit("changed", "x")).unwrap_err().contains("changed since last read"));
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
