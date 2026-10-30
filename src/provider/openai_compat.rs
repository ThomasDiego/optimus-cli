use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::sse::SseLines;
use super::{Delta, Provider, Request};
use crate::message::{Message, Reply, ToolCall, Usage};

/// OpenAI Chat Completions wire format: OpenRouter, Mistral, OpenAI, Ollama…
pub struct OpenAiCompat {
    http: reqwest::Client,
    url: String,
    key: String,
    model: String,
    only: Vec<String>,
}

impl OpenAiCompat {
    pub fn new(base_url: &str, key: String, model: String, only: Vec<String>) -> Self {
        let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
        Self { http: reqwest::Client::new(), url, key, model, only }
    }

    fn body(&self, req: &Request) -> Value {
        let mut messages = vec![json!({"role": "system", "content": req.system})];
        // Reasoning goes back only inside the running tool loop, where
        // DeepSeek requires it; older turns' thinking would only cost tokens.
        let loop_start = req.messages.iter().rposition(|m| matches!(m, Message::User(_))).unwrap_or(0);
        messages.extend(req.messages.iter().enumerate().map(|(i, m)| wire(m, i > loop_start)));
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.schema}}))
            .collect();
        let mut body = json!({
            "model": self.model,
            "messages": messages,
            "tools": tools,
            "stream": true,
            "stream_options": {"include_usage": true},
        });
        // Anthropic models need an explicit marker; others cache prefixes automatically.
        if self.model.trim_start_matches('~').starts_with("anthropic/") {
            body["cache_control"] = json!({"type": "ephemeral"});
        }
        if !self.only.is_empty() {
            body["provider"] = json!({"only": self.only, "allow_fallbacks": false});
        }
        body
    }
}

impl Provider for OpenAiCompat {
    async fn send(&self, req: &Request<'_>, on: &mut dyn FnMut(Delta<'_>)) -> Result<Reply> {
        let mut res = self
            .http
            .post(&self.url)
            .bearer_auth(&self.key)
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&self.body(req))?)
            .send()
            .await
            .context("provider request")?;
        if !res.status().is_success() {
            let status = res.status();
            bail!("{status}: {}", res.text().await.unwrap_or_default());
        }
        let mut reply = Reply::default();
        let mut sse = SseLines::default();
        while let Some(chunk) = res.chunk().await? {
            for data in sse.feed(&chunk) {
                if data == "[DONE]" {
                    return Ok(reply);
                }
                apply(&mut reply, &serde_json::from_str(&data)?, on)?;
            }
        }
        Ok(reply)
    }
}

fn wire(m: &Message, current: bool) -> Value {
    match m {
        Message::User(text) => json!({"role": "user", "content": text}),
        Message::Assistant { text, calls, reasoning } => {
            let mut v = json!({"role": "assistant", "content": if text.is_empty() { Value::Null } else { text.as_str().into() }});
            // DeepSeek's name; OpenRouter takes it as an alias of `reasoning`.
            if current && !reasoning.is_empty() {
                v["reasoning_content"] = reasoning.as_str().into();
            }
            if !calls.is_empty() {
                v["tool_calls"] = calls
                    .iter()
                    .map(|c| json!({"id": c.id, "type": "function", "function": {"name": c.name, "arguments": c.args}}))
                    .collect();
            }
            v
        }
        Message::Tool { id, content } => json!({"role": "tool", "tool_call_id": id, "content": content}),
    }
}

/// Folds one stream chunk into the reply. The trailing usage chunk repeats
/// `finish_reason` on OpenRouter, so it is treated as accounting only.
fn apply(reply: &mut Reply, v: &Value, on: &mut dyn FnMut(Delta<'_>)) -> Result<()> {
    if let Some(err) = v.get("error") {
        bail!("{}", err["message"].as_str().unwrap_or("provider error"));
    }
    if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
        let n = |p: &str| u.pointer(p).and_then(Value::as_u64).unwrap_or(0);
        reply.usage = Usage {
            input: n("/prompt_tokens"),
            output: n("/completion_tokens"),
            cached: n("/prompt_tokens_details/cached_tokens"),
            cache_write: n("/prompt_tokens_details/cache_write_tokens"),
            cost: u["cost"].as_f64().unwrap_or(0.0),
        };
    }
    let Some(delta) = v.pointer("/choices/0/delta") else { return Ok(()) };
    // OpenRouter streams `reasoning`, DeepSeek's own API `reasoning_content`.
    if let Some(r) = delta["reasoning"].as_str().or_else(|| delta["reasoning_content"].as_str()) {
        reply.reasoning.push_str(r);
    }
    if let Some(text) = delta["content"].as_str().filter(|t| !t.is_empty()) {
        on(Delta::Text(text));
        reply.text.push_str(text);
    }
    for c in delta["tool_calls"].as_array().into_iter().flatten() {
        let i = c["index"].as_u64().unwrap_or(0) as usize;
        if reply.calls.len() <= i {
            reply.calls.resize_with(i + 1, ToolCall::default);
        }
        let call = &mut reply.calls[i];
        if let Some(id) = c["id"].as_str().filter(|s| !s.is_empty()) {
            call.id = id.to_string();
        }
        if let Some(name) = c.pointer("/function/name").and_then(Value::as_str).filter(|_| call.name.is_empty()) {
            call.name = name.to_string();
        }
        let args = c.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("");
        call.args.push_str(args);
        on(Delta::Call { name: &call.name, args: &call.args, added: args.len() });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fold(chunks: &[&str]) -> (Reply, String) {
        let (mut reply, mut seen) = (Reply::default(), String::new());
        for c in chunks {
            let mut on = |d: Delta<'_>| match d {
                Delta::Text(t) => seen.push_str(t),
                Delta::Call { added, .. } => seen.push_str(&format!("<{added}>")),
            };
            apply(&mut reply, &serde_json::from_str(c).unwrap(), &mut on).unwrap();
        }
        (reply, seen)
    }

    #[test]
    fn accumulates_text_tool_calls_and_usage() {
        let (reply, seen) = fold(&[
            r#"{"choices":[{"delta":{"content":"Hi"}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"read_file","arguments":"{\"pa"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"a\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            r#"{"choices":[{"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":3,"cost":0.5,"prompt_tokens_details":{"cached_tokens":8}}}"#,
        ]);
        assert_eq!(seen, "Hi<4><8>");
        assert_eq!(reply.calls.len(), 1);
        assert_eq!((reply.calls[0].id.as_str(), reply.calls[0].name.as_str()), ("c1", "read_file"));
        assert_eq!(reply.calls[0].args, r#"{"path":"a"}"#);
        assert_eq!((reply.usage.input, reply.usage.cached, reply.usage.cost), (10, 8, 0.5));
    }

    #[test]
    fn reasoning_goes_back_only_inside_the_running_tool_loop() {
        let (reply, seen) = fold(&[
            r#"{"choices":[{"delta":{"reasoning":"think "}}]}"#,
            r#"{"choices":[{"delta":{"reasoning_content":"more"}}]}"#,
        ]);
        assert_eq!((reply.reasoning.as_str(), seen.as_str()), ("think more", ""));

        let said = |r: &str| Message::Assistant { text: String::new(), calls: vec![], reasoning: r.into() };
        let messages = [Message::User("a".into()), said("old"), Message::User("b".into()), said("new")];
        let p = OpenAiCompat::new("http://x", String::new(), "m".into(), vec![]);
        let body = p.body(&Request { system: "s", messages: &messages, tools: &[] });
        let sent: Vec<&Value> = body["messages"].as_array().unwrap().iter().map(|m| &m["reasoning_content"]).collect();
        assert_eq!(sent, [&Value::Null, &Value::Null, &Value::Null, &Value::Null, &json!("new")]);
    }

    #[test]
    fn stream_error_is_reported() {
        let v = serde_json::json!({"error": {"message": "boom"}});
        assert!(apply(&mut Reply::default(), &v, &mut |_| {}).is_err());
    }
}
