use serde_json::Value;
use tokio::sync::oneshot;

use crate::message::{ToolCall, Usage};
use crate::tools::truncate;

/// Everything the agent shows or asks, consumed by the REPL or the TUI.
pub enum Event {
    Text(String),
    /// Tokens and cost of one model request, for live accounting.
    Usage(Usage),
    ToolCall(ToolCall),
    /// A tool call still being generated: what it targets so far, and how
    /// many characters just arrived (for the live token count).
    Drafting { label: String, added: usize },
    ToolResult { ok: bool, output: String },
    /// Files were written without confirmation; shown to the user only.
    Diff { diff: String },
    Approve { req: Approval, reply: oneshot::Sender<bool> },
}

/// `write_file src/a.rs` from a call whose JSON arguments are still
/// streaming. Only the head is scanned: the target comes before the content.
pub fn draft(name: &str, args: &str) -> String {
    let head = &args[..args.floor_char_boundary(300)];
    let target = ["path", "cmd", "url", "from", "pattern", "op"].iter().find_map(|k| {
        let rest = head.split_once(&format!("\"{k}\":"))?.1.trim_start().strip_prefix('"')?;
        Some(rest.split_once('"').map_or(rest, |(v, _)| v))
    });
    match target {
        Some(t) => format!("{name} {t}"),
        None => name.to_string(),
    }
}

/// One display line for a tool result, and whether it failed. A command that
/// exited non-zero gets its first error line: "exit 128" alone says nothing.
pub fn summary(ok: bool, output: &str) -> (String, bool) {
    let first = output.lines().next().unwrap_or("").replace('\t', "  ");
    let failed = !ok || (first.starts_with("exit ") && !first.starts_with("exit 0 ")) || first.starts_with("killed");
    if !ok || !failed {
        return (first, failed);
    }
    let body = output.split_once("[stderr]\n").or_else(|| output.split_once("[stdout]\n")).map_or("", |(_, b)| b);
    match body.lines().map(str::trim).find(|l| !l.is_empty()) {
        Some(reason) => (format!("{first} · {reason}"), true),
        None => (first, true),
    }
}

/// Added and removed lines of a unified diff, for one-line write summaries.
pub fn stat(diff: &str) -> (usize, usize) {
    let count = |sign: &str, header: &str| diff.lines().filter(|l| l.starts_with(sign) && !l.starts_with(header)).count();
    (count("+", "+++"), count("-", "---"))
}

pub enum Approval {
    /// `label` names the files: `src/a.rs` or `a.rs → b.rs`.
    Write { label: String, diff: String },
    Exec { cmd: String, net: bool },
    /// A web page read; always asked since the URL itself leaves the machine.
    Fetch { url: String },
}

/// One-line human summary of a tool call: `read_file src/main.rs`.
pub fn brief(call: &ToolCall) -> String {
    let args: Value = serde_json::from_str(&call.args).unwrap_or(Value::Null);
    let s = |k: &str| args[k].as_str().map(str::to_string);
    let detail = match call.name.as_str() {
        "run" => s("cmd").map(|c| {
            let rest = args["args"].as_array().into_iter().flatten().filter_map(Value::as_str);
            let cmd = std::iter::once(c.as_str()).chain(rest).collect::<Vec<_>>().join(" ");
            if args["background"] == true { format!("{cmd} (background)") } else { cmd }
        }),
        "process" => s("op").map(|op| args["id"].as_u64().map_or(op.clone(), |id| format!("{op} {id}"))),
        "glob" | "grep" => s("pattern").map(|p| match s("path") {
            Some(path) => format!("{p} in {path}"),
            None => p,
        }),
        "move_file" => s("from").zip(s("to")).map(|(from, to)| format!("{from} → {to}")),
        "replace" => s("old").zip(s("new")).map(|(old, new)| format!("{old} → {new}")),
        "web_fetch" => s("url"),
        "http_local" => args["port"].as_u64().map(|p| format!(":{p}{}", s("path").unwrap_or_default())),
        "git" => s("op").map(|op| {
            let detail = s("message").or_else(|| s("name")).or_else(|| s("rev"));
            [Some(op), detail].into_iter().flatten().collect::<Vec<_>>().join(" ")
        }),
        _ => s("path").or_else(|| {
            let paths = args["paths"].as_array()?.iter().filter_map(Value::as_str).collect::<Vec<_>>();
            Some(paths.join(", "))
        }),
    };
    match detail {
        Some(d) => format!("{} {}", call.name, truncate(&d, 100)),
        None => call.name.clone(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn drafts_name_the_target_while_streaming() {
        assert_eq!(super::draft("write_file", r#"{"path": "src/a.rs", "content": "fn"#), "write_file src/a.rs");
        assert_eq!(super::draft("write_file", r#"{"path":"src/a"#), "write_file src/a");
        assert_eq!(super::draft("run", r#"{"cmd":"npm","args":["run"#), "run npm");
        assert_eq!(super::draft("edit_file", r#"{"pa"#), "edit_file");
    }

    #[test]
    fn failed_commands_show_their_reason() {
        let out = "exit 128 0.0s\n[stderr]\nfatal: not a git repository\nmore";
        assert_eq!(super::summary(true, out), ("exit 128 0.0s · fatal: not a git repository".into(), true));
        assert_eq!(super::summary(true, "exit 0 0.1s\n[stdout]\nok"), ("exit 0 0.1s".into(), false));
        assert_eq!(super::summary(false, "error: nope\nx"), ("error: nope".into(), true));
    }
}
