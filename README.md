# ai

A small, fast coding agent for your terminal, written in Rust.

`ai` works inside the directory you launch it from, and only there. It has no shell: it reads, edits and searches files through its own tools, and starts programs from an allowlist inside an OS sandbox. On macOS that sandbox has no network by default. Data can leave your machine through the network alone, and every network access needs your approval.

## Highlights

- **Sandboxed by design.** File tools go through a capability handle on the project directory: no `..`, absolute path or symlink can reach outside it. Programs run under a macOS Seatbelt profile with a cleared environment, resource limits and their own process group.
- **Secrets never reach the model.** `.env*`, private keys, `.npmrc`, `.netrc`, `.git-credentials` and similar files are blocked for file tools, sandboxed programs and git alike.
- **Three permission modes.** Confirm everything, auto-apply edits, or run fully autonomous. Network access is confirmed in every mode.
- **Undo.** Every change, including deleted directories, can be reverted with `/undo`.
- **Full-screen TUI** with streaming markdown, live tool progress, command suggestions, input history and mouse selection copied to the clipboard. It falls back to a line UI on limited terminals.
- **Headless mode** for scripts and CI: the answer goes to stdout, progress to stderr.
- **Light.** One static binary, no runtime, a handful of dependencies.
- **Lean on tokens.** Batched reads and edits, file outlines, deduplicated re-reads and a stable prompt prefix that stays cached.

## Install

Requires a recent Rust toolchain (edition 2024).

```sh
git clone https://github.com/ThomasDiego/ai-cli.git
cd ai-cli
cargo build --release
cp target/release/ai ~/.local/bin/   # or anywhere on your PATH
```

## Quick start

```sh
export OPENROUTER_API_KEY=sk-or-...
cd my-project
ai
```

With no config file, `ai` uses OpenRouter with `deepseek/deepseek-v4.1-flash-1017`, routed to DeepSeek only.

## Usage

```
ai [-p <prompt>|-] [--mode ask|edit|auto]
```

| Flag | Meaning |
| --- | --- |
| *(none)* | Interactive UI |
| `-p`, `--print <prompt>` | One turn without UI. The answer goes to stdout, progress to stderr. `-` reads the prompt from stdin. |
| `--mode ask\|edit\|auto` | Permission mode (default `ask`) |
| `-h`, `--help` | Show usage |

### Headless examples

```sh
ai -p "List every TODO with file and line number"
git diff | ai -p - --mode ask              # review a diff, read-only
ai -p "Fix the failing test" --mode auto > answer.md
```

Nobody can approve anything in headless mode, so whatever the mode would ask for is denied and reported on stderr. `--mode ask` is therefore read-only, and network access is always denied.

## Permission modes

| Mode | File changes | Commands | Network |
| --- | --- | --- | --- |
| `ask` | confirm | confirm | confirm |
| `edit` | automatic | confirm | confirm |
| `auto` | automatic | automatic (sandboxed) | confirm |

Switch modes with `Shift+Tab` or `/mode`.

## Interactive UI

### Commands

| Command | Action |
| --- | --- |
| `/undo` | Revert the last changes |
| `/mode [ask\|edit\|auto]` | Set the mode, or cycle to the next one |
| `/clear` | Clear the conversation |
| `/exit` | Quit |

Type `/` to get suggestions; `Tab` accepts one.

### Keys

| Key | Action |
| --- | --- |
| `Enter` | Send |
| `Shift+Enter`, `Alt+Enter`, `Ctrl+J` | New line |
| `Esc` | Stop the current turn |
| `Ctrl+C` | Stop the turn or clear the input; press twice to quit |
| `Ctrl+D` | Quit (empty input) |
| `Shift+Tab` | Cycle permission mode |
| `↑` / `↓` | Input history |
| `PageUp` / `PageDown` | Scroll the transcript |
| `Ctrl+A` / `Ctrl+E` | Start / end of input |
| `Ctrl+U` | Clear the input |
| `y` / `n` | Approve or deny a request |

## Tools

The model has 14 tools. None of them writes or executes anything itself: each returns its side effect to the agent, which applies the permission mode first.

| Tool | Purpose |
| --- | --- |
| `read_file` | Read one or more files; long files return an outline or both ends |
| `edit_file` | Exact text replacements, batched across files, all or nothing |
| `replace` | One literal or regex change across every file matching a glob |
| `write_file` | Create or overwrite files |
| `move_file` | Move or rename files, updating directories |
| `delete_file` | Delete files or directories (undoable through a trash) |
| `glob` | Find files by pattern, newest first, respecting `.gitignore` |
| `grep` | Regex search with context, file lists or counts |
| `list_dir` | Directory tree |
| `run` | Start an allowlisted program, in the foreground or in the background |
| `process` | Status, new output or stop for background jobs |
| `http_local` | GET a page from a dev server the agent started (loopback only) |
| `git` | Local git: status, diff, log, show, add, commit, branch, switch, restore, init, stash, uncommit, revert, cherry-pick |
| `web_fetch` | Read a web page as markdown through Jina Reader |

## Configuration

`ai` reads `$XDG_CONFIG_HOME/ai/config.toml` (default `~/.config/ai/config.toml`). Keys are never stored in it, only the name of the environment variable that holds them.

```toml
provider = "openrouter"

# Programs the `run` tool may start, by bare name.
run_allow = ["cargo", "rustc", "rustfmt", "npm", "pnpm", "yarn", "bun", "node", "deno", "tsc", "python3", "pytest", "go", "make"]

[providers.openrouter]
kind = "openai_compat"
base_url = "https://openrouter.ai/api/v1"
key_env = "OPENROUTER_API_KEY"
model = "deepseek/deepseek-v4.1-flash-1017"
only = ["deepseek"]          # OpenRouter: these upstream providers only, no fallback
```

Any OpenAI-compatible endpoint works: add a `[providers.<name>]` table and point `provider` at it.

| Variable | Use |
| --- | --- |
| `OPENROUTER_API_KEY` (or your `key_env`) | Provider API key, required |
| `JINA_API_KEY` | Optional, sent to Jina Reader by `web_fetch` |
| `XDG_CONFIG_HOME` | Config location |

## Security model

`ai` assumes the model can be wrong or manipulated, for example by a prompt injection in a file or web page, and constrains what it can do.

- **No shell, anywhere.** `run` starts programs directly: no pipes, globs, `&&` or redirections.
- **One reachable directory.** File tools resolve every path through a `cap-std` handle on the launch directory. Nothing outside it can be read or written.
- **Sandboxed programs (macOS).** `run` wraps programs in a Seatbelt profile (`src/seatbelt.sb`): reads outside the project and toolchains are denied, network and unix sockets are off, and package caches become writable only when you approve network access. The environment is cleared except `PATH`, `HOME`, `USER`, `LANG` and `TMPDIR`. CPU time and file size are limited, and the program runs in its own session, with no controlling terminal and nothing it can inject into yours.
- **Network is the only way out.** Any network access (`run` with `net`, `web_fetch`) needs interactive approval, in every mode. Without it, `cargo` runs offline from its local cache.
- **Secrets stay hidden.** Secret files are denied by the file tools, by the Seatbelt profile (which also covers arbitrary `build.rs` code) and are excluded from `git add`.
- **Undoable deletions.** Deleted directories are parked in `.ai-trash/` until the session ends, so `/undo` can restore them.
- **Contained jobs.** Background jobs die with the session, and dev servers accept localhost connections only.
- **No workarounds.** When a tool refuses, the model is told to report it, not to try another route.

> [!WARNING]
> On Linux and other platforms there is no kernel sandbox yet: only the absence of a shell, the program allowlist, the cleared environment and approvals apply. Prefer `ask` or `edit` mode there.

## Development

```sh
cargo build
cargo test
cargo clippy -- -D warnings
```

Some tests need a terminal; run them under `script -q /dev/null cargo test`.

### Evaluation suite

`eval/` holds a headless benchmark of about 100 real tasks: refactors, git work, web lookups, background servers, and adversarial cases that try to read secrets, escape the project or leave daemons behind. Each task runs `ai -p … --mode auto` in a fresh copy of `eval/fixture`, then a shell check decides pass or fail. Escapes and surviving daemons are detected separately.

```sh
cargo build --release
OPENROUTER_API_KEY=... JOBS=4 eval/run.sh          # all tasks
OPENROUTER_API_KEY=... eval/run.sh rename secret   # some tasks
OPENROUTER_API_KEY=... eval/redteam.sh             # hostile build.rs and escape prompts
```

Tasks are defined in `eval/tasks.txt` as `name|check|prompt`, with optional setup scripts in `eval/setup/<name>.sh`. Runs happen under `$HOME`, because temp directories are writable by the sandbox and escape tests there would prove nothing. Results land in `eval/results/`.

### Layout

```
src/
├── main.rs            entry point: config, provider, UI selection
├── cli.rs             argument parsing
├── config.rs          config file and defaults
├── agent.rs           system prompt, tool loop, approvals
├── permission.rs      ask / edit / auto
├── sandbox.rs         workspace capability handle, secret rules
├── seatbelt.sb        macOS sandbox profile
├── exec.rs            sandboxed process execution
├── jobs.rs            background jobs
├── web.rs             web fetch through Jina Reader
├── provider/          OpenAI-compatible streaming client
├── tools/             one file per tool
├── tui/               full-screen interface
├── repl.rs            line-mode fallback
└── headless.rs        -p mode
```
