# ai

A small, fast coding agent for your terminal, written in Rust.

`ai` works inside the directory you launch it from, and only there. It has no shell: it reads, edits and searches files through its own tools, and starts programs from an allowlist inside an OS sandbox with no network by default. Data can leave your machine through the network alone, and every network access needs your approval.

Runs on macOS, Linux and Windows. See [Platforms](#platforms) for how far the sandbox goes on each.

## Highlights

- **Sandboxed by design.** File tools go through a capability handle on the project directory: no `..`, absolute path or symlink can reach outside it. Programs run in a kernel sandbox (Seatbelt on macOS, Landlock and seccomp on Linux) with a cleared environment, resource limits and their own process group.
- **Secrets never reach the model.** `.env*`, private keys, `.npmrc`, `.netrc`, `.git-credentials` and similar files are blocked for file tools and git everywhere, and for sandboxed programs on macOS.
- **Three permission modes.** Confirm everything, auto-apply edits, or run fully autonomous. Network access is confirmed in every mode.
- **Undo.** Every change, including deleted directories, can be reverted with `/undo`.
- **Full-screen TUI** with streaming markdown, live tool progress, command suggestions, input history and mouse selection copied to the clipboard. It falls back to a line UI on limited terminals.
- **Headless mode** for scripts and CI: the answer goes to stdout, progress to stderr.
- **Light.** One static binary, no runtime, a handful of dependencies.
- **Lean on tokens.** Batched reads and edits, file outlines, deduplicated re-reads and a stable prompt prefix that stays cached.

## Install

`ai` builds from source with a recent Rust toolchain (edition 2024) and a C compiler. `cargo install` puts the binary in `~/.cargo/bin`, which rustup adds to your `PATH`.

### macOS

```sh
xcode-select --install                                           # C compiler and git
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # Rust
git clone https://github.com/ThomasDiego/ai-cli.git
cd ai-cli
cargo install --path . --locked
```

### Linux

```sh
sudo apt install build-essential git        # Debian, Ubuntu
# sudo dnf install gcc git                  # Fedora
# sudo pacman -S base-devel git             # Arch
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
git clone https://github.com/ThomasDiego/ai-cli.git
cd ai-cli
cargo install --path . --locked
```

The sandbox needs Landlock (kernel 5.13 or later, 6.7 or later to block TCP). `cat /sys/kernel/security/lsm` must list `landlock`; without it, `ai` warns in every command result.

### Windows

In PowerShell:

```powershell
winget install Microsoft.VisualStudio.2022.BuildTools --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
winget install Rustlang.Rustup
winget install Git.Git
# Open a new terminal so PATH includes cargo and git.
git clone https://github.com/ThomasDiego/ai-cli.git
cd ai-cli
cargo install --path . --locked
```

## Quick start

```sh
cd my-project
ai
```

The first run asks for a provider (OpenRouter, OpenAI, DeepSeek or any OpenAI-compatible API such as Ollama), an API key and a model. It saves them only once a real request proves that the key works with that model. Change any of them later with `/config`, without leaving `ai`.

The default is OpenRouter with `deepseek/deepseek-v4.1-flash-1017`, routed to DeepSeek only.

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
| `/config` | Change the provider, key, model or allowed programs |
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

Use `/config` inside `ai`: it shows the current settings and changes one at a time. A new key or model is saved only after a test request succeeds. Wrong keys send you back to the key and unknown models back to the model, with the provider's own error.

Settings live in `$XDG_CONFIG_HOME/ai/config.toml` (default `~/.config/ai/config.toml`, or `%APPDATA%\ai\config.toml` on Windows). Keys are never stored in it. A key typed in `/config` goes to the `.env` file next to it, readable by you only. Being a `.env` file, it is refused to the model by the file tools, the sandbox and git, like any secret. When that file has no key, `ai` falls back to the environment variable named by `key_env`.

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

Any OpenAI-compatible endpoint works: add a `[providers.<name>]` table and point `provider` at it. On Windows, Python is usually `python` or `py`: add it to `run_allow`.

The config is checked at startup:
- `base_url` must use HTTPS, except for a local server, so the key never travels in clear.
- `key_env` must be a variable name.
- `run_allow` keeps bare program names only and never a shell.
- A file that no longer parses opens the setup, and the old file is kept as `config.toml.bak`.

| Variable | Use |
| --- | --- |
| `OPENROUTER_API_KEY` (or your `key_env`) | Provider API key, when none was saved with `/config` |
| `JINA_API_KEY` | Optional, sent to Jina Reader by `web_fetch` |
| `XDG_CONFIG_HOME` | Config location |

## Security model

`ai` assumes the model can be wrong or manipulated, for example by a prompt injection in a file or web page, and constrains what it can do.

- **No shell, anywhere.** `run` starts programs directly: no pipes, globs, `&&` or redirections.
- **One reachable directory.** File tools resolve every path through a `cap-std` handle on the launch directory. Nothing outside it can be read or written.
- **Sandboxed programs (macOS).** `run` wraps programs in a Seatbelt profile (`src/seatbelt.sb`): reads outside the project and toolchains are denied, network and unix sockets are off, and package caches become writable only when you approve network access. The environment is cleared except `PATH`, `HOME`, `USER`, `LANG` and `TMPDIR`. CPU time and file size are limited, and the program runs in its own session, with no controlling terminal and nothing it can inject into yours.
- **Network is the only way out** (macOS and Linux). Any network access (`run` with `net`, `web_fetch`) needs interactive approval, in every mode. Without it, `cargo` runs offline from its local cache.
- **Secrets stay hidden.** Secret files are denied by the file tools, by the Seatbelt profile on macOS (which also covers arbitrary `build.rs` code) and are excluded from `git add`.
- **Undoable deletions.** Deleted directories are parked in `.ai-trash/` until the session ends, so `/undo` can restore them.
- **Contained jobs.** Background jobs die with the session, and on macOS dev servers accept localhost connections only.
- **No workarounds.** When a tool refuses, the model is told to report it, not to try another route.

### Platforms

File tools, approvals, secret filtering in file tools and git, the cleared environment and job cleanup work the same everywhere. What differs is the sandbox around programs started by `run` and `git`.

| | macOS | Linux | Windows |
| --- | --- | --- | --- |
| Sandbox | Seatbelt | Landlock + seccomp | none |
| Writes outside project and temp | denied | denied | allowed |
| Reads of `HOME` and outside toolchains | denied | denied | allowed |
| Network without approval | denied | TCP and UDP denied | allowed |
| Local sockets and pipes (Docker, agents) | denied | denied | allowed |
| Project secret files, `.git`, `.ai-trash` | denied | readable and writable | readable and writable |
| Saved API key (`~/.config/ai/.env`) | denied | denied | readable |
| Servers reachable from the LAN | no | if they bind `0.0.0.0` | if they bind `0.0.0.0` |
| Process tree killed, CPU limit, no terminal | yes | yes | yes (job object) |

**Linux.** Landlock needs kernel 5.13 or later, and 6.7 or later to block TCP. On older kernels seccomp refuses every IP socket until you approve network access. Without Landlock at all, for example on WSL 1, programs run unconfined and each result says so. Landlock can only grant whole directory trees, so it cannot hide files inside the project the way Seatbelt does.

**Windows.** Programs run with your rights. Only the absence of a shell, the allowlist, the cleared environment and approvals protect you, so prefer `ask` or `edit` mode. Programs are looked up in `PATH` only, never in the project. `.cmd` scripts such as `npm` go through `cmd.exe`, with arguments escaped by Rust's standard library.

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
├── config.rs          config file, defaults and validation
├── keys.rs            API keys saved by /config
├── setup/             first-run setup and /config, with the key and model check
├── agent.rs           system prompt, tool loop, approvals
├── permission.rs      ask / edit / auto
├── sandbox.rs         workspace capability handle, secret rules
├── seatbelt.sb        macOS sandbox profile
├── os/                per-OS sandbox and process control (Seatbelt, Landlock, seccomp, job objects)
├── exec.rs            sandboxed process execution
├── jobs.rs            background jobs
├── web.rs             web fetch through Jina Reader
├── provider/          OpenAI-compatible streaming client
├── tools/             one file per tool
├── tui/               full-screen interface
├── repl.rs            line-mode fallback
└── headless.rs        -p mode
```
