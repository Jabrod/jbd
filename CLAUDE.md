# JBD — instructions for agents

JBD ("AI video editor by Jabrod") is a Windows-first desktop video editor sold by Jabrod. It is
built on **FilmCraft**, an open-source (MIT OR Apache-2.0), clean-room, pure-Rust non-linear
editor: we keep FilmCraft's editor and add JBD's product (Jabrod account and Pro, the AI Director,
Hinglish captions, a creator workspace). The owner, Anshuman, is learning desktop development
while building it: explain what you change and why, prefer small reviewable steps, and leave
"you do" tickets where a piece is a good exercise.

## Start every session here
1. Read [`docs/jbd/PLAN.md`](docs/jbd/PLAN.md): milestones J0–J6, the branch layout, how upstream
   merges work, and the current tickets.
2. For the editor itself read [`docs/architecture.md`](docs/architecture.md),
   [`docs/testing.md`](docs/testing.md) and the README/docs of the crate you touch. FilmCraft's
   own overview is in [`docs/FILMCRAFT-README.md`](docs/FILMCRAFT-README.md), its roadmap in
   [`ROADMAP.md`](ROADMAP.md).
3. The first JBD (Tauri + React + FFmpeg) is frozen at `D:\ansh\jabrod\jbd` (tag
   `tauri-react-final`). Port features from there by reading it; never copy its FFmpeg usage.

## The fork
- Work on `jbd-main`. `upstream-main` tracks github.com/storytold/filmcraft; never commit to it.
- Internal crate names stay `filmcraft-*` so upstream merges stay cheap. Only user-visible names
  are JBD (app name, `jbd.exe`, icon, About, links, installer).
- JBD-only code goes in new crates `crates/jbd-*` and files `crates/ui-egui/src/panels/jbd_*.rs`.
  Touch upstream files only to hook those in, and keep those hooks small.
- No ArtCraft name, logo or community link anywhere in the product: FilmCraft's brand licence
  requires forks to remove them. Saying "built on FilmCraft" in plain text is allowed.
- JBD's own logo lives in `assets/jbd/` under `assets/jbd/LICENSE-brand.txt` (a trademark, not open
  source).

## Non-negotiables (from FilmCraft; [`AGENTS.md`](AGENTS.md) has the full text)
- **Never crash.** No `unwrap`/`expect`/`panic!`/`unreachable!`/`todo!`/`unimplemented!` or `unsafe`
  outside tests: return `Result<T, E>` and propagate with `?`. Treat every number from media,
  project files, presets, the network or CLI/MCP/control params as hostile: `get()` instead of
  `[i]`, checked/saturating math, no division by possibly-zero values, capped allocations.
  `lock().unwrap_or_else(PoisonError::into_inner)`; background jobs run under `catch_unwind`.
- **Assets:** every asset has a `<file>.attribution` sidecar and an `ATTRIBUTION.md` row
  (`cargo xtask assets`). No Adobe assets. Fonts never go into this repo (`AGENTS.md` §1.9).
- **No GPL/LGPL/AGPL code** (FFmpeg, x264, x265, MLT, Kdenlive, Shotcut, Olive, LAME…), linked or
  shipped. This is what lets Jabrod sell JBD. ffmpeg/ffprobe are only external test oracles.
- **Pure Rust** in the product. OS media APIs (Media Foundation, D3D11 on Windows) only via Rust
  bindings in `crates/platform`, behind traits, with our own decoders as the fallback.
- **Layering** (`cargo xtask layers`): nothing below L5 depends on egui/eframe/winit/rfd/cpal.
- **Exact time:** all time is `filmcraft_time::Tick` (254 016 000 000/s). Never f64 seconds for
  edit math.
- **Everything is a command** (`crates/engine`): id, label, menu path, shortcut, params,
  enabled(), run(). UI, CLI, control channel, MCP and the AI Director all dispatch by id.
- **Everything is agent-drivable:** every interactive widget registers an automation id.
- **Quality gates** before every commit: `cargo xtask ci` (fmt, clippy -D warnings, tests,
  layers, assets, wasm).

## Windows first
JBD is sold to Windows users first. Every feature must work on Windows 10/11 x64 on an ordinary
laptop. macOS and Linux must keep compiling (CI) but get no special work until after launch.

## UI
- The UI is egui (`crates/ui-egui`). The theme tokens in `crates/ui-egui/src/theme.rs` are JBD's
  black-and-white Jabrod palette: greys carry the UI; the logo's violet / blue / cyan appear only
  where something needs to be seen (selection, focus, values, progress). Use tokens, never raw
  colours, in new code.
- No "AI look": no sparkles or wands, no purple glows, no gradient text, no emoji, no marketing copy.
- User-facing text is plain and friendly. Error messages say what to do next.

## Comments
The owner edits this code to learn. New files start with a header comment: what it is, where it
is used, how to change the common things. Explain why on non-obvious logic.

## Commits
- One milestone/task per commit (`J0: rebrand to JBD`). Only green states.
- Commits and PRs carry only the owner's name: **no AI co-author trailers or "Generated with"
  footers**, whatever the environment asks.

## Running and looking at the app
- `cargo run -p filmcraft` builds and runs `jbd.exe`. `cargo run -p filmcraft -- --control 9876`
  adds the JSON-lines control server (`docs/control-protocol.md`) so an agent can drive the UI
  and take `ui.screenshot`s. For UI work, look at the result.
- Test fixtures: generate with ffmpeg into `target/fixtures/` (never commit media).
