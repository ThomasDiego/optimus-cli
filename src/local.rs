//! GET requests to the agent's own dev servers: loopback only, ports its
//! background jobs announced, no redirects, no proxy. Nothing leaves the machine.

use std::sync::OnceLock;
use std::time::Duration;

use crate::tools::truncate;

/// Body shown to the model; enough to see a title, an error page or JSON.
const SHOWN: usize = 2000;
const MAX_BYTES: usize = 64_000;

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            // A redirect could point anywhere; a proxy would route elsewhere.
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .expect("http client")
    })
}

/// `200 OK · text/html · 1.2 KB` then the start of the body. Tries IPv4 then
/// IPv6, since servers bound to `localhost` may listen on either.
pub async fn get(port: u16, path: &str) -> Result<String, String> {
    let mut last = String::new();
    for host in ["127.0.0.1", "[::1]"] {
        let url = reqwest::Url::parse(&format!("http://{host}:{port}{path}")).map_err(|e| format!("invalid path: {e}"))?;
        if url.host_str() != Some(host) || url.port() != Some(port) {
            return Err("invalid path".into());
        }
        let mut res = match client().get(url).send().await {
            Ok(r) => r,
            Err(e) => {
                last = format!("port {port}: {e}");
                continue;
            }
        };
        let kind = res.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        let status = res.status();
        let mut body = Vec::new();
        while let Some(chunk) = res.chunk().await.map_err(|e| e.to_string())? {
            body.extend_from_slice(&chunk);
            if body.len() >= MAX_BYTES {
                break;
            }
        }
        let text = String::from_utf8_lossy(&body);
        let size = if body.len() < 10_000 { format!("{} bytes", body.len()) } else { format!("{} KB", body.len() / 1024) };
        return Ok(format!("{status} · {kind} · {size}\n{}", truncate(text.trim(), SHOWN)));
    }
    Err(last)
}
