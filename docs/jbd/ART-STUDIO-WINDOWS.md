# Art Studio for Windows: JBD becomes Art Studio (native)

Decided 2026-11-19 with Anshuman: the Windows app becomes **Art Studio**, one native app holding
the JBD editor and the studio's chat, Image/Voice Studio, Canvas and Library, rebuilt as native
egui screens on the same `art.jabrod.com/api/art` endpoints the web Art Studio uses (no webview).

## Ground rules

- **Existing JBD users update in place.** Keep `jbd.exe`, the install folder (`%LOCALAPPDATA%\JBD`),
  settings (`%APPDATA%\JBD`), the updater endpoint (`/api/releases/jbd/...`), the Jabrod sign-in
  and the `jbd` Pro entitlement (Art Pro). Only user-visible names change.
- **Microsoft Store:** the MSIX identity (Package Name and Publisher) never changes, so Store installs
  keep updating. Whether the Store *listing* is renamed to Art Studio is Anshuman's call; the
  display name in the package follows that decision.
- **One backend.** No studio logic is duplicated on a server: the native screens call the existing
  `/api/art/*` routes, so web and Windows share boards, chats, Library and credits.
- CLAUDE.md rules hold (never crash, no GPL, everything is a command, egui theme tokens, comments
  for learning, "you do" tickets).

## Milestones

| | What | Size |
|---|---|---|
| **A0** | **Rebrand to Art Studio:** window title, icon (Jabrod mark), installer display name and Start-menu shortcut, About, links to art.jabrod.com. Keep `jbd.exe` and folders. Release 1.3.0. | small |
| **A1** | **`jbd-art` crate:** the `/api/art` client (boards, assets, models, chats, generate, video status, billing), using the app's access token. Server side (jbd_home): `/api/art/*` also accepts JBD desktop bearer tokens for the same account. Tests against the local dev server. | small–medium |
| **A2** | **Library + Image/Voice Studio panels:** browse the Library with thumbnails, generate images and voice with cost shown, "Add to timeline" imports into the open project. Video generation shows the honest "not available yet" until the gateway has credit. | medium |
| **A3** | **Chat panel:** chat history, streaming replies on a background thread, simple Markdown, model picker, and prompt cards that send to Image Studio or the timeline. | medium |
| **A4** | **Canvas:** boards in egui (pan/zoom, Prompt/Image/Video/Audio/Note/Export nodes, connections, autosave), reading and writing the same board document as the web (version 1), with generate-from-node. | large |
| **A5** | **Release Art Studio 2.0:** plans and credits in-app, changelog, jabrod.com "Download Art Studio for Windows", Store package. | small |

### Where A0 and A1 stand (19 November 2026)

- **A0 done in code, not released.** Every visible "JBD" says "Art Studio": window title, title bar,
  splash, sign-in, welcome screen, menus, About, updates, settings, dialogs, file-type names. New icon
  `assets/jbd/art-studio.ico` / `art-studio-icon-{256,1024}.png` (exe, window, installer, Store
  logos). Help and the account menu open jabrod.com/art and art.jabrod.com. Version 1.3.0.
  **Kept on purpose** so 1.2.0 upgrades in place and nobody loses anything: `jbd.exe`,
  `%LOCALAPPDATA%\JBD` (install folder), the uninstall key, `%APPDATA%\JBD` (settings, autosave),
  eframe's app name "JBD", the "JBD" workspace id, "JBD Generated" and log folders, the updater
  endpoint, the `jbd` OAuth client and Pro entitlement, the Store package identity and Store
  display name (Anshuman renames the Store listing in Partner Center when 1.3.0 ships). The
  installer removes the old `JBD.lnk` shortcuts and makes `Art Studio.lnk`.
  **Still says JBD:** the "Made with JBD" mark on free-plan exports (a baked image in
  `crates/export/src/pipeline.rs`; ticket below) and internal comments and crate names.
- **A1 done:** `crates/jbd-art` (`Client`: me, models, boards, save with conflict check, Library,
  image/voice/video generation and video status, chats with streamed replies, billing). jabrod.com's
  `artUser()` (jbd_home `lib/platform/art.ts`) now also accepts the desktop app's own access token
  (`desktopUser()`), so the app sees the same account as the web studio. Uploads are not in the
  client yet (Vercel Blob's client-upload handshake): part of A2.

Each milestone ends in one release build (about 25 minutes on this PC with `CARGO_BUILD_JOBS=2`)
and a check in the running app over the control channel (`--control 9876`, `ui.screenshot`).

## Size and memory

Native panels add a few MB to `jbd.exe` and no runtime. Thumbnails are decoded small and cached on
disk; chat streams text only. Builds on this PC need two parallel jobs (full parallelism ran out of
commit memory).

## Your tickets (learn by doing)

- A0: replace the "Made with JBD" export mark with "Made with Art Studio": make the new mark image
  (same size, white with the soft shadow), give it an `.attribution` sidecar and an
  `ATTRIBUTION.md` row, point `pipeline::JBD_MARK` at it, and update the text in
  `crates/ui-egui/src/jbd/mod.rs` that describes it. `cargo test -p filmcraft-engine export` checks it.
- A1: add `Client::library_page_after(last: &LibraryAsset)` to `crates/jbd-art` (the next page of the
  Library, passing `created_at` as `before`) and a test for the query it builds.
- A2: add the cost label to the Generate button.
