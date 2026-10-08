# JBD on FilmCraft: the plan

JBD ("AI video editor by Jabrod") is now built on top of FilmCraft, an MIT/Apache-2.0 pure-Rust
video editor. We keep FilmCraft's editor (timeline, monitors, codecs, colour, audio, captions,
command system) and add what makes JBD JBD: a Windows-first product, Jabrod accounts and Pro,
the AI Director, Hinglish captions and a simpler creator workspace.

The previous JBD (Tauri + React + FFmpeg) is frozen at `D:\ansh\jabrod\jbd`, tag
`tauri-react-final`. It is the reference for features and design when porting them.

## How this repository is organised

| Branch / tag | What it is |
|---|---|
| `jbd-main` | JBD. All our work lands here. |
| `upstream-main` | Tracks `upstream/main` (github.com/storytold/filmcraft). Never commit to it. |
| `filmcraft-base-2026-10-06` | The exact FilmCraft commit (771b614) JBD started from. |

**Staying mergeable with upstream.** FilmCraft moves fast and fixes bugs we want. To keep
`git merge upstream-main` cheap:

- Internal crate names stay `filmcraft-*`. Only what users see is renamed (app name, exe, icon,
  window title, About, links, installer).
- JBD-only code goes into new crates `crates/jbd-*` and new files
  `crates/ui-egui/src/panels/jbd_*.rs`. Edit upstream files only to hook those in.
- Merge upstream about once a week: `git fetch upstream && git checkout upstream-main && git pull`
  then `git checkout jbd-main && git merge upstream-main`, and run `cargo xtask ci`.

**Licence duties.** Keep `LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE` and `ATTRIBUTION.md`, and say
in About that JBD is based on FilmCraft. The ArtCraft name and logos (`docs/brand/`) are NOT
licensed to forks: none of them may appear in JBD. JBD itself may be closed source and sold.

## Milestones

Each one ends with something you can run and show.

### J0 — JBD boots (done 2026-10-07)
- [x] Rebrand what users see: name JBD, `jbd.exe` (with icon and version info), window title,
  About ("built on FilmCraft"), Help and Home links to jabrod.com, demo project "JBD Demo".
- [x] ArtCraft marks removed (`docs/brand/` deleted, Discord button gone, NOTICE updated);
  JBD logo in `assets/jbd/` under its own trademark licence.
- [x] JBD's black-and-white theme is the default (`crates/ui-egui/src/theme.rs`).
- [x] User data in `%APPDATA%\JBD`, apart from any FilmCraft install.
- [x] Builds and runs on Windows (first debug build 20 min on the laptop, then incremental).
- [x] CLAUDE.md and README rewritten for JBD; FilmCraft's README kept in `docs/`.
- Left for J1: the Windows installer (`packaging/windows/filmcraft.wxs`, `package.ps1`) still
  builds a FilmCraft MSI. It needs the JBD name, `jbd.exe`, jabrod.com URLs and a **new
  UpgradeCode GUID** (otherwise Windows treats JBD as an upgrade of FilmCraft).
- Decide (owner): the licence of Jabrod's own code. The workspace `Cargo.toml` still says
  `MIT OR Apache-2.0` for every crate; the README says Jabrod's additions are all rights reserved.

### J1 — Ready to sell: JBD 1.0.0 (done 2026-10-07)
The owner moved the account/Pro work into J1 and the AI work into J2 (2026-10-07).

- [x] JBD look and flow from the first JBD: launch animation (3D cube landing, faces wiping on),
  sign-in screen, frameless window with JBD's title bar (menus, undo/redo, saved state, CPU/GPU
  meters, layout buttons, white Export, account chip, window buttons), start screen (templates,
  recent projects), the JBD layout (Media | Preview | Inspector / Transcript / Director over the
  Timeline), Media toolbar, preview size badge, JBD Settings. Type: Geist, Geist Mono, Archivo
  Black (as jabrod.com). One loader: the turning cube. (`crates/ui-egui/src/jbd/`)
- [x] Jabrod account (`crates/jbd-account`): browser sign-in (PKCE, loopback), refresh token in
  Windows Credential Manager (the first JBD's saved session carries over), 14-day offline grace,
  plan and AI credits from jabrod.com, Upgrade / Refresh / Sign out.
- [x] Plans enforced in the engine (`Session::plan`, `export_tools::spawn_export`): Free exports
  are capped at 720p and carry the "Made with JBD" mark; Pro and the trial are unrestricted.
  Test: `engine export_tests::jbd_free_plan_caps_exports_at_720p_with_the_mark`.
- [x] Windows: taskbar icon (`platform::win_icon`), GPU meter (`platform::win_gpu`), static C
  runtime (`.cargo/config.toml`), speech to text on by default (Whisper, pure Rust).
- [x] Performance checked: software decode on the laptop runs 1080p H.264 at ~390 fps, vertical
  HEVC at ~160 fps, 4K H.264 at ~100 fps, so hardware decoding is not needed for 1.0.
- [x] Packages (`packaging/windows/package-jbd.ps1`, [RELEASE.md](RELEASE.md)): per-user NSIS
  installer that upgrades the first JBD in place (and honours its updater's /P /R), and the Store
  MSIX under the reserved identity `Jabrod.JBD`. EULA and full third-party notices included;
  FilmCraft is credited only there (its MIT licence requires the notice).
- [x] Store listing copy: [STORE-LISTING.md](STORE-LISTING.md).
- Known gaps: `crates/ui-egui/tests/media_ui.rs` fails on Windows (renames a folder that holds
  open files); projects from the first JBD (project.db folders) can't be opened yet; the setup exe
  is not code-signed (SmartScreen warns); hardware decode (Media Foundation) after launch.

### J2 — The AI Director (next)
The owner's priority after 1.0. The account, the AI credits and the jabrod.com AI gateway are ready.

- New crate `jbd-director`: the agent loop from the first JBD's `src/core/ai/agent.ts`, in Rust.
  Its tools are engine commands (the same registry the MCP server exposes), so every AI edit is a
  normal, undoable command. Model calls: jabrod.com `/api/ai/chat` with the account's access token
  (`AccountBackend::access_token`).
- The Director tab (`jbd/director.rs`) becomes real: prompt, plan, steps, undo.
- Recipes: remove pauses, 9:16 reel, captions (Hinglish via transliteration), cut to the beat.
- Import projects from the first JBD.

### J3 — Understanding the footage
- Transcription on by default: FilmCraft's pure-Rust Whisper locally, plus cloud ASR through the
  jabrod.com gateway (ElevenLabs Scribe / Deepgram, picked by the bake-off).
- Hinglish: Roman-script captions via word-count-preserving transliteration.
- Port pauses, shot changes, shot quality and music beats (old `analysis.rs`, `shots.rs`,
  `rhythm.rs`) into a `jbd-analysis` crate; searchable footage index.

### J4 — (moved into J2)

### J5 — Creator workspace
- A simple default workspace for reel creators (Media, Preview, Director, Transcript, Timeline).
  The Premiere-style workspaces stay available for people who want them.
- Caption styles and motion templates from the old JBD.

### J6 — Launch
- Paid early access on Windows: website download + Microsoft Store, crash reporting, support.

## Your tickets (learn by doing)

Each milestone gets a "you do" ticket, reviewed like a pull request. Starting with J0:

1. Run JBD from source: double-click `start-jbd.cmd` (or `start-jbd.cmd demo`). Then try
   `start-jbd.cmd control` and read `docs/control-protocol.md` to see how an agent drives it.
2. Add one JBD colour token to `crates/ui-egui/src/theme.rs` and use it somewhere visible.
3. Read `docs/architecture.md` §1–3 and explain back how a menu click becomes an engine command.
