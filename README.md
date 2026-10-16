<p align="center">
  <img src="assets/jbd/jbd-logo.png" alt="JBD" width="96">
</p>

<h1 align="center">JBD</h1>

<p align="center"><b>The video editor by Jabrod.</b> Part of Jabrod Art.</p>

<p align="center">
  <a href="https://github.com/Jabrod/jbd/releases/latest"><img src="https://img.shields.io/github/v/release/Jabrod/jbd?label=version&color=3d5cff" alt="Latest version"></a>
  <a href="#licence"><img src="https://img.shields.io/badge/licence-MIT%20or%20Apache--2.0-555" alt="Licence: MIT or Apache-2.0"></a>
  <img src="https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-555" alt="Windows 10 and 11">
</p>

<p align="center">
  <a href="https://apps.microsoft.com/detail/9P0KS6185QSX">Microsoft Store</a> ·
  <a href="https://jabrod.com/jbd">Download</a> ·
  <a href="https://github.com/Jabrod/jbd/releases">Releases</a> ·
  <a href="CHANGELOG.md">Changelog</a> ·
  <a href="https://jabrod.com/art">Jabrod Art</a>
</p>

JBD is a desktop video editor for Windows 10 and 11. It provides a multi-track timeline with
ripple, roll, slip and slide trims; colour grading with curves, wheels, looks and scopes; audio
mixing with loudness meters; titles and editable caption tracks with on-device speech to text;
and export up to 4K.

Since version 1.1, JBD includes the **AI Director**: describe the edit you want, and it plans the
edit, works step by step on your timeline, generates video and voiceover, and checks the result
on rendered frames before it reports back. Every change it makes can be undone.

JBD is the first product of [Jabrod Art](https://jabrod.com/art), Jabrod's open-source creative
suite. It is built on [FilmCraft](https://github.com/storytold/filmcraft), an open-source,
pure-Rust video editor. FilmCraft's original README is in
[docs/FILMCRAFT-README.md](docs/FILMCRAFT-README.md).

## Install

| | |
|---|---|
| Microsoft Store | [apps.microsoft.com/detail/9P0KS6185QSX](https://apps.microsoft.com/detail/9P0KS6185QSX) |
| Direct download | [jabrod.com/jbd](https://jabrod.com/jbd) |

JBD is free. JBD Pro, available from within the application, adds further features. A Jabrod
account is required to sign in; projects and media remain on your computer.

## Versions

The current version is **1.1.0**. Every version is tagged in this repository and published on the
[Releases](https://github.com/Jabrod/jbd/releases) page, on [jabrod.com](https://jabrod.com/jbd) and
in the Microsoft Store. JBD checks for new versions itself (Help ▸ Check for Updates).

| Version | Date | Highlights | |
|---|---|---|---|
| [1.1.0](https://github.com/Jabrod/jbd/releases/tag/v1.1.0) | 16 Oct 2026 | The AI Director; What's New, Check for Updates and Report a Problem; refined themes | [Changes](CHANGELOG.md#110--16-october-2026) |
| [1.0.0](https://github.com/Jabrod/jbd/releases/tag/v1.0.0) | 8 Oct 2026 | The first JBD built on FilmCraft: a new, faster editor | [Changes](CHANGELOG.md#100--8-october-2026) |

The full history is in [CHANGELOG.md](CHANGELOG.md) and at
[jabrod.com/jbd/changelog](https://jabrod.com/jbd/changelog).

## Build from source (Windows)

Requirements: Rust (stable, MSVC toolchain) and the Visual Studio 2022 C++ Build Tools.

```powershell
winget install -e --id Rustlang.Rustup
winget install -e --id Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

Then run `start-jbd.cmd` in the repository root, or from a terminal:

```powershell
.\start-jbd.cmd          # Home screen
.\start-jbd.cmd demo     # opens the demo project
.\start-jbd.cmd control  # also starts the control server on port 9876, for agents
```

The script runs `cargo run -p filmcraft` with at most six parallel compile jobs, to keep memory
use within 16 GB. The first build compiles about 50 crates and takes some time; subsequent builds
are incremental. The executable is `target\debug\jbd.exe`. For an optimised build, use
`cargo run -p filmcraft --release`.

Before submitting a change, run `cargo xtask ci`. Contribution guidelines are in
[docs/contributing.md](docs/contributing.md).

## Documentation

- [Architecture](docs/architecture.md)
- [Control protocol for agents](docs/control-protocol.md)
- [Captions](docs/captions.md)
- [Release process](docs/jbd/RELEASE.md)
- [The AI Director: design](docs/jbd/J2-DIRECTOR.md)
- [Changelog](CHANGELOG.md)

## Credits

JBD is built on **[FilmCraft](https://github.com/storytold/filmcraft)**, an open-source,
pure-Rust non-linear video editor by the FilmCraft contributors. FilmCraft provides the editing
engine, the codecs, the renderer, the timeline and most of the editor's interface; JBD adds the
Jabrod account, JBD Pro, the AI Director and its own design. We thank the FilmCraft contributors
for making their work available.

- JBD 1.0 was based on FilmCraft as of 6 October 2026 (tag `filmcraft-base-2026-10-06` in this
  repository). Improvements from FilmCraft are merged regularly (`upstream-main`).
- FilmCraft is available under the MIT or Apache 2.0 licence. Its copyright notice is retained in
  [NOTICE](NOTICE), and FilmCraft's original README is in
  [docs/FILMCRAFT-README.md](docs/FILMCRAFT-README.md).
- Third-party fonts, media and other material, with their authors and licences, are listed in
  [ATTRIBUTION.md](ATTRIBUTION.md).

## Licence

JBD is available under either the [Apache License 2.0](LICENSE-APACHE) or the
[MIT License](LICENSE-MIT), at your option. The notices in `NOTICE` and `ATTRIBUTION.md` must be
retained.

The JBD and Jabrod names and logos, including the files in `assets/jbd/`, are trademarks of
Jabrod and are not licensed under the above terms (see `assets/jbd/LICENSE-brand.txt`).

## Contact

[contact@jabrod.com](mailto:contact@jabrod.com) · [jabrod.com](https://jabrod.com)
