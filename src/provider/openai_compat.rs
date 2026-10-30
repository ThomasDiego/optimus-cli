use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::sse::SseLines;
use super::{Provider, Request};
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
        messages.extend(req.messages.iter().map(wire));
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
    async fn send(&self, req: &Request<'_>, on_text: &mut dyn FnMut(&str)) -> Result<Reply> {
        let mut res = self
            .http
            .post(&self.url)
            .bearer_auth(&self.key)
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&self.body(req))?)
            .send()
            .await
            .context("requête provider")?;
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
                apply(&mut reply, &serde_json::from_str(&data)?, on_text)?;
            }
        }
        Ok(reply)
    }
}

fn wire(m: &Message) -> Value {
    match m {
        Message::User(text) => json!({"role": "user", "content": text}),
        Message::Assistant { text, calls } => {
            let mut v = json!({"role": "assistant", "content": if text.is_empty() { Value::Null } else { text.as_str().into() }});
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
fn apply(reply: &mut Reply, v: &Value, on_text: &mut dyn FnMut(&str)) -> Result<()> {
    if let Some(err) = v.get("error") {
        bail!("{}", err["message"].as_str().unwrap_or("erreur provider"));
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
    if let Some(text) = delta["content"].as_str().filter(|t| !t.is_empty()) {
        on_text(text);
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
        if let Some(args) = c.pointer("/function/arguments").and_then(Value::as_str) {
            call.args.push_str(args);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fold(chunks: &[&str]) -> (Reply, String) {
        let (mut reply, mut seen) = (Reply::default(), String::new());
        for c in chunks {
            apply(&mut reply, &serde_json::from_str(c).unwrap(), &mut |t| seen.push_str(t)).unwrap();
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
        assert_eq!(seen, "Hi");
        assert_eq!(reply.calls.len(), 1);
        assert_eq!((reply.calls[0].id.as_str(), reply.calls[0].name.as_str()), ("c1", "read_file"));
        assert_eq!(reply.calls[0].args, r#"{"path":"a"}"#);
        assert_eq!((reply.usage.input, reply.usage.cached, reply.usage.cost), (10, 8, 0.5));
    }

    #[test]
    fn stream_error_is_reported() {
        let v = serde_json::json!({"error": {"message": "boom"}});
        assert!(apply(&mut Reply::default(), &v, &mut |_| {}).is_err());
    }
}
