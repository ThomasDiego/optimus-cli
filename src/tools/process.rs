use serde::Deserialize;
use serde_json::json;

use super::{Ctx, Outcome, Tool, parse};
use crate::message::ToolDef;

/// Watches and stops jobs started with `run` background=true.
pub struct Process;

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Op {
    Status,
    Output,
    Stop,
}

#[derive(Deserialize)]
struct Args {
    op: Op,
    id: Option<usize>,
    wait: Option<u64>,
}

impl Tool for Process {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "process",
            description: "Background jobs from run background=true. status lists them; output shows new lines since last call; stop kills one.",
            schema: json!({
                "type": "object",
                "properties": {
                    "op": {"type": "string", "enum": ["status", "output", "stop"]},
                    "id": {"type": "integer", "description": "job id, for output and stop"},
                    "wait": {"type": "integer", "description": "output: wait up to N seconds (max 300) for the job to end; follows long commands"}
                },
                "required": ["op"]
            }),
        }
    }

    fn run(&self, ctx: &mut Ctx, args: &str) -> Result<Outcome, String> {
        let a: Args = parse(args)?;
        let id = || a.id.ok_or("id required: use the job id from run background=true or from process status");
        let out = match a.op {
            Op::Status => ctx.jobs.status(),
            Op::Output if a.wait.is_some_and(|w| w > 0) => return Ok(Outcome::Wait { id: id()?, secs: a.wait.unwrap_or(0) }),
            Op::Output => ctx.jobs.output(id()?)?,
            Op::Stop => ctx.jobs.stop(id()?)?,
        };
        Ok(Outcome::Done(out))
    }
}
