use serde::Deserialize;
use serde_json::json;

use super::{Ctx, Outcome, Tool, parse};
use crate::message::ToolDef;

/// Checks a dev server the agent started, without curl.
pub struct HttpLocal;

#[derive(Deserialize)]
struct Args {
    port: u16,
    path: Option<String>,
}

impl Tool for HttpLocal {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "http_local",
            description: "GET a page from a dev server you started with run background=true (only the ports it printed). Returns status, type and the start of the body.",
            schema: json!({
                "type": "object",
                "properties": {
                    "port": {"type": "integer"},
                    "path": {"type": "string", "description": "default /"}
                },
                "required": ["port"]
            }),
        }
    }

    fn run(&self, ctx: &mut Ctx, args: &str) -> Result<Outcome, String> {
        let a: Args = parse(args)?;
        let ports = ctx.jobs.ports();
        if !ports.contains(&a.port) {
            let seen = ports.iter().map(u16::to_string).collect::<Vec<_>>().join(", ");
            return Err(format!(
                "port {} is not served by a running background job (ports seen: {}); start the server with run background=true and use the port it prints",
                a.port,
                if seen.is_empty() { "none" } else { &seen }
            ));
        }
        let path = a.path.unwrap_or_else(|| "/".into());
        if !path.starts_with('/') || path.len() > 1000 || path.contains(char::is_whitespace) {
            return Err(r#"path must start with / and have no spaces, e.g. "/" or "/api/users""#.into());
        }
        Ok(Outcome::Local { port: a.port, path })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::HttpLocal;
    use crate::exec::Exec;
    use crate::tools::tests::ctx;
    use crate::tools::{Outcome, Tool};

    #[tokio::test]
    async fn reaches_only_ports_its_own_jobs_announced() {
        let mut c = ctx("http-local", &[("index.html", "hello page")]);
        let root = c.ws.root.clone();
        let run = |c: &mut crate::tools::Ctx, v: serde_json::Value| HttpLocal.run(c, &v.to_string());
        assert!(run(&mut c, json!({"port": 22})).err().unwrap().contains("ports seen: none"));

        let args = ["-u", "-m", "http.server", "0", "--bind", "127.0.0.1"].map(String::from).to_vec();
        c.jobs.start(&Exec::new("python3".into(), args, root.clone(), Duration::ZERO), &root).await.unwrap();
        let port = *c.jobs.ports().first().expect("server printed its port");
        let Outcome::Local { port, path } = run(&mut c, json!({"port": port, "path": "/index.html"})).unwrap() else { unreachable!() };
        let page = crate::local::get(port, &path).await.unwrap();
        assert!(page.starts_with("200 OK · text/html") && page.contains("hello page"), "{page}");

        // The path cannot redirect the request to another host.
        assert!(run(&mut c, json!({"port": port, "path": "@evil.com/"})).err().is_some());
        assert!(crate::local::get(port, "@evil.com/").await.is_err());
    }
}
