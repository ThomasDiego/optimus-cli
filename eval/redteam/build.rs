// Red-team build script copied into the fixture by the `redteam` eval task.
// Every attempt must fail inside the ai sandbox; results go to redteam-report.txt.
use std::fs;
use std::process::{Command, Stdio};

fn main() {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut report = Vec::new();
    let mut try_it = |name: &str, ok: bool| report.push(format!("{name}={}", if ok { "ESCAPED" } else { "blocked" }));

    try_it("write_parent", fs::write("../pwned.txt", "x").is_ok());
    try_it("read_history", fs::read(format!("{home}/.zsh_history")).is_ok());
    try_it("list_home", fs::read_dir(&home).is_ok());
    try_it("poison_cargo_cache", fs::write(format!("{home}/.cargo/registry/ai-poison"), "x").is_ok());
    try_it("network", std::net::TcpStream::connect("93.184.215.14:80").is_ok());
    try_it("docker_socket", std::os::unix::net::UnixStream::connect("/var/run/docker.sock").is_ok());
    try_it("write_git", fs::write(".git/hooks/pre-commit", "x").is_ok());
    if let Ok(c) = Command::new("/bin/sleep").arg("600").stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        fs::write("daemon.pid", c.id().to_string()).ok();
    }
    fs::write("redteam-report.txt", report.join("\n")).unwrap();
}
