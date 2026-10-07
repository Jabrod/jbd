<p align="center">
  <img src="assets/jbd/jbd-logo.png" alt="JBD" width="96">
</p>

<h1 align="center">JBD</h1>

<p align="center"><b>AI video editor by Jabrod.</b> Windows first.</p>

JBD is a desktop video editor for creators: a real multi-track timeline, colour, audio and
captions, plus an AI Director that edits for you ("make a 45 second reel from this, remove the
pauses, add Hinglish captions, make it vertical").

JBD is built on [FilmCraft](https://github.com/storytold/filmcraft), an open-source, pure-Rust
video editor (MIT OR Apache-2.0). FilmCraft's own README is in
[docs/FILMCRAFT-README.md](docs/FILMCRAFT-README.md).

## Status

Early. Milestone J0 (JBD boots as JBD on Windows) is in progress. The plan, milestones and the
branch layout are in [docs/jbd/PLAN.md](docs/jbd/PLAN.md).

## Build and run (Windows)

You need Rust (stable, MSVC toolchain) and the Visual Studio 2022 C++ Build Tools:

```powershell
winget install -e --id Rustlang.Rustup
winget install -e --id Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

Then double-click **`start-jbd.cmd`** in this folder, or from a terminal:

```powershell
.\start-jbd.cmd          # Home screen
.\start-jbd.cmd demo     # opens the demo project
.\start-jbd.cmd control  # plus the control server on port 9876, for agents
```

(It runs `cargo run -p filmcraft` with at most 6 parallel compile jobs, because 16 at once runs a
16 GB laptop out of memory.)

The first build compiles about 50 crates (the codecs, renderer and UI) and takes a while; after
that it is incremental. The program is `target\debug\jbd.exe`. For a fast build to try on real
footage, use `cargo run -p filmcraft --release`.

Checks before a commit: `cargo xtask ci`.

## Licence

The FilmCraft code in this repository is MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`,
`NOTICE`, `ATTRIBUTION.md`); those notices must stay. Jabrod's own additions are © Jabrod, all
rights reserved, unless a file says otherwise. The JBD name and logo in `assets/jbd/` are
trademarks of Jabrod (`assets/jbd/LICENSE-brand.txt`).
