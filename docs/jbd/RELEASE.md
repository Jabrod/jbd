# Releasing JBD for Windows

Two channels ship the same `jbd.exe`:

| Channel | Package | Updates |
|---|---|---|
| jabrod.com download | `JBD-<version>-setup.exe` (NSIS, per-user, no admin) | the first JBD's updater (0.1.x) and, later, JBD's own |
| Microsoft Store | `JBD_<version>.0_x64.msix` (unsigned; the Store signs it) | the Store |

## 1. Build and package

```powershell
pwsh packaging/windows/package-jbd.ps1
```

This builds `jbd.exe` (release, static C runtime, icon and version info required), checks it is a
64-bit GUI program, writes the full third-party notices, and makes both packages plus
`release.json` (sizes and SHA-256s) in `dist\jbd-<version>\`. The version is
`[workspace.package] version` in `Cargo.toml`.

## 2. Test on a clean Windows account

- Install the setup exe; JBD opens from the Start menu and desktop shortcut; the taskbar shows
  the JBD cube.
- Sign in (browser opens jabrod.com, JBD continues by itself), check the account menu: plan and
  AI credits.
- Free account: export a clip; it is 720p with the "Made with JBD" mark. Pro: 1080p/4K, no mark.
- Install over the first JBD (0.1.x): one "JBD" in Apps, the old FFmpeg files gone, sign-in kept.
- Uninstall: files and shortcuts go; projects and `%APPDATA%\JBD` stay.

## 3. Sign the installer for the first JBD's updater

The 0.1.x apps only install an update signed with the updater key (the old repo,
`D:\ansh\jabrod\jbd\.tauri\jbd-updater.key`; never commit it):

```powershell
cd D:\ansh\jabrod\jbd
npx tauri signer sign -f .tauri\jbd-updater.key -p "" ..\jbd-v1.0\dist\jbd-<version>\JBD-<version>-setup.exe
```

It writes `JBD-<version>-setup.exe.sig` next to the installer.

## 4. Publish on jabrod.com (repo jbd_home)

1. Copy the setup exe to `public/downloads/`.
2. Update `lib/jbd-release.ts`: version, date, notes, `downloadUrl`
   (`https://jabrod.com/downloads/JBD-<version>-setup.exe`), `publicDownloadUrl`
   (`https://jabrod.com/download/jbd/<version>`), SHA-256, size, and the `.sig` contents as
   `signature`.
3. `npm run build`, then commit and push `master` (Vercel deploys in about a minute).
4. Check: `/download/jbd/<version>` redirects to the installer; the updater endpoint
   `/api/releases/jbd/update/windows/x86_64/0.1.0` returns the manifest; `.../<version>` returns 204.

## 5. Microsoft Store

Upload the MSIX to the **JBD** product in Partner Center (Store ID `9P0KS6185QSX`), with the
listing in [STORE-LISTING.md](STORE-LISTING.md) and the art from `jbd-media/out/store/`. Keep the
fourth version field 0 and raise an earlier field for every update.

## Code signing

The setup exe is not Authenticode-signed yet, so Windows SmartScreen warns "unknown publisher"
until it builds reputation. Signing (Azure Trusted Signing, about $10 a month, or an OV
certificate) removes that; `package-jbd.ps1` is where to add it. The Store package is signed by
Microsoft and needs nothing.
