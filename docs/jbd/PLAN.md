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

### J1 — Windows first
- Windows CI that runs the tests and a headless import → edit → export smoke test.
- Hardware decode with Media Foundation / D3D11 in `crates/platform` (FilmCraft only has macOS
  VideoToolbox). Phone footage (H.264/HEVC, variable frame rate) must play smoothly on a normal
  Windows laptop. This is the biggest technical risk; upstream wants it too (their #30), so
  build it in a way we could offer back.
- A corpus of real Android/iPhone/screen-recording files with round-trip tests.
- Windows bugs found so far: `crates/ui-egui/tests/media_ui.rs` fails on Windows (2 tests,
  "Access is denied" at line 93): it renames a media folder while the editor still holds files in
  it open. macOS allows that; Windows does not. Close the handles (or change the test) and check
  what users see when they move media while JBD is open.
- Direct download: the existing WiX MSI (`packaging/windows/`) renamed for JBD with a new
  UpgradeCode, signed, with an update check against jabrod.com.

#### Microsoft Store
JBD ships to the Store as an MSIX under the identity the first JBD already reserved in Partner
Center, so it is the same Store listing, not a new app:

- Package/Identity/Name `Jabrod.JBD`, Publisher `CN=B31502B4-249B-4B91-8E2D-5E7989531A54`,
  PublisherDisplayName `Jabrod`, Store ID `9P0KS6185QSX` (case-sensitive; from the old repo's
  `docs/release-windows.md` and `src-tauri/store/AppxManifest.template.xml`).
- Port `scripts/build-store-msix.ps1` from the old repo into `packaging/windows/store/`: stage
  the release `jbd.exe` (no FFmpeg sidecars any more), write the manifest (Windows.Desktop,
  runFullTrust, internetClient, the `jbd` protocol for sign-in), make the Store logos from
  `assets/jbd/`, and pack with MakeAppx. Upload it unsigned; Microsoft signs it.
- Store builds leave updates to the Store (no in-app updater), keep the fourth version field at 0
  (`1.0.0.0`), and must pass the Windows App Certification Kit on a clean Windows 10 and 11.
- Listing: description, screenshots of the new UI, privacy and support URLs on jabrod.com.

### J2 — Jabrod account and Pro
- New crate `jbd-account`: port the old `src-tauri/src/auth.rs` (OAuth code + PKCE over a
  loopback port to jabrod.com, refresh token in Windows Credential Manager, 14-day offline grace).
- Pro entitlement check (one plan, ₹4,999/year, 7-day trial); Dodo Payments checkout opens in
  the browser on jabrod.com. Free exports get the JBD watermark.

### J3 — Understanding the footage
- Transcription on by default: FilmCraft's pure-Rust Whisper locally, plus cloud ASR through the
  jabrod.com gateway (ElevenLabs Scribe / Deepgram, picked by the bake-off).
- Hinglish: Roman-script captions via word-count-preserving transliteration.
- Port pauses, shot changes, shot quality and music beats (old `analysis.rs`, `shots.rs`,
  `rhythm.rs`) into a `jbd-analysis` crate; searchable footage index.

### J4 — The AI Director
- New crate `jbd-director`: the agent loop from the old `src/core/ai/agent.ts`, in Rust. Its
  tools are FilmCraft engine commands (the same registry the MCP server exposes), so every AI
  edit is a normal, undoable command.
- Model calls go to jabrod.com `/api/ai/chat` (OpenRouter, Vercel AI Gateway fallback).
- Director panel in egui; recipes: remove pauses, make a 9:16 reel, captions, cut to the beat.

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
