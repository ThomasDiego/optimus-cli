/// Minimal Server-Sent Events splitter: yields `data:` payloads, skips comments.
#[derive(Default)]
pub struct SseLines {
    buf: Vec<u8>,
}

impl SseLines {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            if let Some(data) = line.trim_end().strip_prefix("data:") {
                out.push(data.trim_start().to_string());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::SseLines;

    #[test]
    fn splits_across_chunks() {
        let mut sse = SseLines::default();
        assert!(sse.feed(b": OPENROUTER PROCESSING\n\ndata: {\"a\"").is_empty());
        assert_eq!(sse.feed(b":1}\r\n\ndata: [DONE]\n"), ["{\"a\":1}", "[DONE]"]);
    }
}
