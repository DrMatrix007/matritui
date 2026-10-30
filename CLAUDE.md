# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`mmux` is a terminal multiplexer, structurally similar to tmux, purpose-built to run the `claude` CLI in its panes and surface Claude Code's subagent/tool/edit activity live. It is a single Rust binary with no other processes involved except the `claude` children it spawns and short-lived `hook-forward` invocations (see Architecture).

The project is being built in strict milestones, each implemented, reviewed, and manually verified against the real `claude` CLI before the next starts. Commit history reflects this: `git log --oneline` shows one commit per milestone (M0 PTY+render, M1 tmux-style panes, config system, M2 hook pipeline, M3 subagent/edit sidebar, M4 overview). Preserve that granularity in future work — don't squash milestone-sized changes together.

## Commands

```sh
cargo build                 # debug build
cargo run                   # run mmux interactively (needs a real terminal, not a pipe)
cargo test                  # all unit + integration tests
cargo test <name>           # run a single test by (partial) name match, e.g. `cargo test overview_entries`
cargo test --lib <name>     # restrict to unit tests in src/ (skip tests/pane_integration.rs)
cargo clippy --all-targets  # lint; keep clean beyond the pre-existing baseline warnings (see below)
```

There is no separate lint-fix or format-check command configured beyond `cargo clippy`/`cargo fmt`.

Manual/interactive verification (since most of the value here is a live TUI driving a real child process) has, for every milestone so far, been done by running `cargo run` against the real `claude` CLI through a scripted `tmux send-keys`/`capture-pane` session, with two or more concurrent panes to prove per-pane isolation. When the sandbox's model backend can't be driven into real tool/subagent activity, the fallback is to send crafted hook-event JSON through the real, unmodified `mmux hook-forward` binary, using a pane's actual `MMUX_PANE_ID`/`MMUX_IPC_SOCKET` (read from `/proc/<pid>/environ` for a running pane) — this exercises the real production code path end-to-end without needing the model to cooperate. Always note which of the two verification paths was used.

Known baseline: `cargo build`/`cargo test` are expected clean; `cargo clippy --all-targets` has a couple of pre-existing warnings from testing `pane.rs` internals via `tests/pane_integration.rs`'s `#[path]` inclusion (e.g. `Pane::spawn`/`Pane::wait` dead-code warnings under `cargo test` that don't appear under plain `cargo build`) — these are known and accepted, not regressions to chase.

## Architecture

### Two entry points in one binary

`main.rs` dispatches on `argv[1] == "hook-forward"` before anything else (before raw mode / alternate screen are touched). Everything else is the normal interactive TUI. `hook-forward` is never invoked by a user directly — Claude Code's own hook mechanism invokes it (see below).

### Pane → Window → App, and the layout tree

- `pane.rs`: one `Pane` = one PTY-backed `claude` (or other command) child process. Each pane runs its own OS thread blocking-reading the PTY into a shared `Arc<Mutex<vt100::Parser>>`; the main loop never blocks on PTY I/O, it only locks the parser briefly to render. `Pane::spawn_with_hooks` is used for real panes (threads the hook env vars/settings-file flag through `portable-pty`'s `CommandBuilder`); `Pane::spawn` is a hooks-free path used only by tests. The PTY's cwd is explicitly set via `builder.cwd(...)` — this was *not* automatic (a real bug caught during M4: `portable-pty` silently defaults to `$HOME` if `cwd` is never set).
- `layout.rs`: pure, PTY-free binary split tree (`Node::Leaf`/`Node::Split`) plus `compute_rects`/`split_leaf`/`remove_leaf`/`navigate`. This is the one module that's cleanly unit-testable without spawning real processes, and layout math changes should go here, not be duplicated in `window.rs`.
- `window.rs`: owns a `HashMap<PaneId, Pane>` plus one `layout::Node` tree for that window. Handles split/close/focus-navigation/resize. Enforces the tmux-exact border rule: a window with exactly one pane renders full-bleed (no border), borders appear only once split into 2+ panes (see `main.rs`'s `draw_border = rects.len() >= 2`).
- `app.rs`: the implicit single "session" — `Vec<Window>` plus focus/resize/kill-all. There is no separate `Session` type; M1's data model treats the single running `mmux` process as one implicit session (detach/reattach, which would need a real session concept behind an IPC boundary, is explicitly deferred — see Deliberately not built, below).

Resize flows one way: terminal resize event → `App::resize` → every `Window::resize` (not just the focused one) → each window's layout tree recomputes rects → each affected pane's PTY and `vt100::Parser` are resized to its actual rect. `App::resize` and `App::new` both call `clamp_min_size` (min 1x1) because a window with only 1 pane skips the border inset that otherwise implicitly provided a floor — an extreme terminal shrink previously panicked here (`NonZeroU16::new(0).unwrap()`); don't remove the clamp.

### Config (`config.rs`)

`Config::load()` resolves `~/.config/mmux/config.toml` (platform-appropriate path via the `directories` crate — don't hand-roll per-OS path logic) at the very start of `main()`, before raw mode is entered, so a parse warning is visible on stderr. Loading is structured as: parse the whole TOML file; on total failure, fall back entirely to defaults with a warning; on a per-field problem, that one field falls back to its default while every other field still applies (partial overrides must keep working — don't change this to all-or-nothing). `KeyBindings` and `Colors` both have a `Default` impl that is the single source of truth for "what mmux does with no config file" — if you add a new configurable keybinding or color, add it to the struct, the `Default` impl, and the TOML parsing (see `parse_key`'s `"C-<letter>"`/named-arrow/single-char grammar), not as a separate hardcoded constant elsewhere. `input.rs`/`render.rs`/`main.rs` all take `KeyBindings`/`Colors` by value/reference rather than reading globals.

### Hook pipeline (`hooks.rs`, `ipc.rs`, `hook_forward.rs`)

This is how `mmux` learns what a `claude` child is actually doing, and it is **hooks-only by deliberate choice** — no `statusLine` integration, no session-transcript JSONL parsing. That means cumulative token/cost data is not available anywhere in this codebase; don't add it without also adding a second data source.

1. Before spawning a pane's `claude` child, `hooks::write_settings_file` writes an isolated per-pane temp JSON file (`mmux-hooks-<hook_id>.json`) registering all 10 hook events (`SessionStart`, `SessionEnd`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `SubagentStart`, `SubagentStop`, `Stop`, `Notification`, `PreCompact`) with matcher `"*"`, each pointing its `command` at `mmux hook-forward` (the current binary's own path, via `std::env::current_exe()`).
2. `hooks::pane_launch_extras` produces the env vars (`MMUX_PANE_ID`, `MMUX_IPC_SOCKET`) and CLI args (`--settings <path>`) that get threaded into that pane's `claude` child via `Pane::spawn_with_hooks`.
3. When Claude Code fires a hook, it runs `mmux hook-forward`, which reads the event JSON from stdin plus `MMUX_PANE_ID`/`MMUX_IPC_SOCKET` from its environment, and forwards both over a local IPC connection (`ipc::send_event`). This subcommand must stay fast and must never panic or block — it runs once per hook event, inline with Claude Code's own execution.
4. `ipc.rs` wraps the `interprocess` crate (abstract-namespace Unix socket on Linux/macOS, named pipe on Windows — this is the one deliberately platform-abstracted IPC boundary in the project; don't add OS-specific branches here, extend the abstraction instead). Framing is newline-delimited compact JSON (`Envelope { pane_id, event }`) — safe because compact JSON never contains a literal newline. `Server::bind` is called exactly once in `main()`, before any panes are spawned, under the socket name `ipc::socket_name()` (`mmux-<pid>`) — every pane's `MMUX_IPC_SOCKET` is set to that same name, so the binding must happen first or panes will fail to connect.
5. The main event loop drains the server non-blockingly (`ipc_server.try_recv()`) once per iteration and feeds events into `App::record_hook_event` → `state::StateEngine::record`.

If you need to verify this pipeline without the model cooperating (see Commands), the crafted-event technique sends directly through the real `hook_forward`/`ipc` code — it is not a mock, it exercises the actual wire format.

### State engine and subagent attribution (`state.rs`)

`StateEngine` holds one `PaneState` per pane (`by_pane: HashMap<String, PaneState>`, keyed by `hook_id`/pane id). Each `PaneState` tracks `root_subagents: Vec<SubagentNode>` (a tree, not a flat list) and `edit_timeline: Vec<EditEntry>` (capped at `EDIT_TIMELINE_CAP = 200`, FIFO eviction).

The key correctness property here: **every hook event fired from inside a subagent carries `agent_id`/`agent_type` fields**, which is what makes it possible to attribute a tool call to the correct subagent even when subagents run in parallel (this was verified against Claude Code's hooks docs before M3 was built — see git history / commit messages for the research). `SubagentNode`s are matched/nested by `agent_id` via `find_node_mut`/`remove_node`, both of which recurse into `children`, not just the root list — a first version of this that only searched root-level nodes had a real re-parenting bug (a nested tool-use event arriving, via IPC, before its owning subagent's `SubagentStart` created a root-level placeholder that never got moved once the real parent resolved); if you touch this matching logic, make sure cross-level re-parenting still works, not just root-level attribution.

`PendingCall` correlates a tool's `PreToolUse` with its later `PostToolUse`/`SubagentStart` by tool name + a FIFO `seq` counter, since hook payloads don't carry a documented direct parent/child link between a `Agent`/`Task` tool call and the `SubagentStart` it triggers. Note: Claude Code renamed its `Task` tool to `Agent` at some point (v2.1.63) — this code should keep accepting both names defensively rather than assuming one.

### Rendering (`render.rs`) and the main loop (`main.rs`)

`main.rs`'s loop, each iteration: reap exited panes (and exit `mmux` entirely if that empties the last window) → drain IPC events into the state engine → poll for one terminal event (33ms) → dispatch `input::Action` → redraw. The redraw path picks one of three mutually exclusive views: the overview (`render::render_overview`, full-screen, whenever `show_overview`), or the normal pane grid (always) plus an optional sidebar column (`render::render_sidebar`, `SIDEBAR_WIDTH = 40` columns, reserved via `app::split_for_sidebar` so pane rects/PTY sizes actually shrink to make room — toggling the sidebar is a real resize, not an overlay).

`vt100::Cell::contents()` returns `""` for a blank cell. Rendering that empty string directly into ratatui's buffer desyncs `CrosstermBackend`'s cursor-tracking diff logic and causes visible screen corruption — blank cells must be written as a single space. This was a real bug found and fixed in M0; don't reintroduce it if you touch the vt100→ratatui translation in `render_pane`.

### Keybindings

Default prefix is `Ctrl-b` (tmux-compatible), configurable via `config.rs`. Current bindings (`input.rs`'s state machine: `Normal` / `AwaitingPrefix` / `AwaitingKillConfirm` / overview-navigation state): `c` new window, `n`/`p` next/prev window, `0`-`9` select window directly (hardcoded, not rebindable — a character class, not a single value, so it's out of the config system by design, same as kill-confirm `y`/`n`), `%`/`"` split horizontal/vertical, arrow keys navigate panes, `x` kill focused pane (prompts `y`/`n` — matches tmux's real destructive-action default), `s` toggle sidebar, `e` expand/cycle the sidebar's edit timeline into a full diff, `o` toggle the cross-pane overview (arrow keys + Enter to jump to a pane from there), `Ctrl-b Ctrl-b` passthrough (forwards a literal prefix keypress to the focused pane, since the prefix key is also meaningful to some programs, e.g. readline's backward-char). `d` quits `mmux` entirely (killing every pane) — this is deliberately repurposing tmux's real `detach-client` binding, since detach/reattach doesn't exist yet; see the comment on `input::Action::Quit`'s handling in `main.rs` before changing this.

## Deliberately not built (don't add speculatively)

These were explicitly scoped out at specific points, not overlooked — check with the project owner before building any of them, since each was a conscious product decision:

- **Detach/reattach.** Everything lives in one process's memory; there's no client/server split. `App`/`Window`/`Pane` state would need to move behind an IPC boundary for this.
- **Per-window working directory.** Every window in one `mmux` process shares that process's launch cwd; there's no UI to open a new window elsewhere. The cross-pane overview's project-grouping (`app::OverviewEntry`) is real infrastructure, but within a single process it will show one group, since all cwds are currently identical — this only varies across separate `mmux` processes today.
- **Cost/token display.** Not available via hooks (see above); would need `statusLine` or transcript-JSONL parsing as a second data source.
- **Historical session browsing.** `mmux` only ever knows about sessions it itself launched (via its own hook injection) — it does not read `~/.claude/projects` transcripts from past/unrelated sessions.
- **Non-Claude-Code agent support.** The hook payload shapes, tool names (`Agent`/`Task`), and settings-file format in `hooks.rs`/`state.rs` are Claude-Code-specific. No `AgentProvider`-style abstraction exists yet.
