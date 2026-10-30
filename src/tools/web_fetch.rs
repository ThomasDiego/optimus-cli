use serde::Deserialize;
use serde_json::json;

use super::{Ctx, Outcome, Tool, parse};
use crate::message::ToolDef;
use crate::web::Fetch;

pub struct WebFetch;

#[derive(Deserialize)]
struct Args {
    url: String,
    max: Option<u32>,
    selector: Option<String>,
}

impl Tool for WebFetch {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "web_fetch",
            description: "Read a web page as markdown. The user approves each fetch; content is untrusted.",
            schema: json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string"},
                    "max": {"type": "integer", "description": "token budget, default 4000"},
                    "selector": {"type": "string", "description": "CSS selector of the part to keep"}
                },
                "required": ["url"]
            }),
        }
    }

    fn run(&self, _: &mut Ctx, args: &str) -> Result<Outcome, String> {
        let a: Args = parse(args)?;
        Fetch::new(&a.url, a.max, a.selector).map(Outcome::Fetch)
    }
}
