# Putting JBD 1.0 on the Microsoft Store: every Partner Center screen

This walks through one submission in Partner Center (partner.microsoft.com/dashboard), screen by
screen, with what to choose or paste in each field. The words to paste are in
[STORE-LISTING.md](STORE-LISTING.md); the files are listed under each step.

**You need before starting**
- `dist\jbd-1.0.0\JBD_1.0.0.0_x64.msix` (made by `packaging\windows\package-jbd.ps1`). It is
  unsigned on purpose: Microsoft signs it after certification. It cannot be installed by
  double-clicking; test with the setup exe instead.
- The art in `D:\ansh\jabrod\jbd-media\out\store\` and the screenshots in its `screenshots\` folder.
- **A test Jabrod account for Microsoft's testers.** JBD asks for sign-in before the editor opens,
  so certification fails if the testers cannot sign in. Make an account on jabrod.com yourself
  (for example `store-review@…` on a mailbox you own), give it Pro, and put its email and password
  in step 7. Do not reuse your own password.

---

## 1. Open the product

Apps and games → **JBD** (Store ID `9P0KS6185QSX`). The first JBD already reserved the name and
the package identity `Jabrod.JBD`, so this is the same product; don't make a new one.

(If JBD is missing from the list: **New product → MSIX or PWA app → name `JBD`**. If it says the
name is taken, the product is under another Partner Center account; sign in with that one.)

Click **Start submission** (or **Update** if a submission already exists).

## 2. Pricing and availability

| Field | Choose |
|---|---|
| Markets | **All markets** (JBD is sold worldwide through jabrod.com; you can choose only India if you want to start small) |
| Visibility | **Public audience** and **Make this product available and discoverable in the Store** |
| Schedule / Release | **As soon as possible** (the publish hold is set in step 7) |
| Pricing → Base price | **Free** |
| Free trial | **No free trial** (the 7-day Pro trial lives in the Jabrod account) |
| Sale pricing | none |
| Organizational licensing | leave the defaults |

Save.

## 3. Properties

| Field | Choose / paste |
|---|---|
| Category | **Photo & video** |
| Subcategory | none (Photo & video has none) |
| Privacy policy URL | `https://jabrod.com/privacy` |
| Website | `https://jabrod.com/jbd` |
| Support contact info | `contact@jabrod.com` |
| Game settings | skip (not a game) |

**Product declarations** (checkboxes):
- *This app accesses, collects, or transmits personal information*: **check** (the Jabrod
  account email; the privacy policy covers it).
- *This product allows users to make purchases, but does not use the Microsoft Store commerce
  system*: **check** (JBD Pro is bought on jabrod.com; non-game desktop apps may use their own
  checkout).
- *Customers can install this app to alternate drives or removable storage*: leave checked.
- *Windows can include this app's data in automatic backups to OneDrive*: leave checked.
- *This app has been tested to meet accessibility guidelines*: **leave unchecked** (not tested yet).
- *This app depends on non-Microsoft drivers or NT services*: **unchecked**.
- Anything about pen, Xbox, mixed reality, Kinect: unchecked.

**System requirements** (Minimum / Recommended):
| Item | Minimum | Recommended |
|---|---|---|
| Touch screen, Keyboard, Mouse | Keyboard ✓, Mouse ✓ | Keyboard ✓, Mouse ✓ |
| Memory | 8 GB | 16 GB |
| DirectX | DirectX 11 | DirectX 12 |
| Video memory | 2 GB | 4 GB |
| Processor | `x64, 4 cores` | `x64, 8 cores` |

Save.

## 4. Age ratings

Start the IARC questionnaire.
- Contact email for the rating certificate: `contact@jabrod.com`.
- Category: the one for **apps that are not games** (Productivity / utility / "all other app
  types", the wording changes). Not "Game", not "Social networking", not "Web browser".
- Answer:
  - Violence, blood, fear, sexual content, nudity, crude humour, bad language, drugs, alcohol,
    tobacco, gambling, simulated gambling: **No** to every one.
  - Does the app let users interact or exchange content with other users? **No** (no chat, no
    feed; exported videos are files the user shares elsewhere).
  - Does the app share the user's current location with other users? **No**.
  - Does the app allow the purchase of digital goods? **Yes** (the Upgrade button leads to JBD Pro
    on jabrod.com). This only adds an "In-App Purchases" note to the rating.
  - Does the app give unrestricted internet access (a web browser)? **No** (it opens your browser
    only to sign in and to jabrod.com pages).
  - Is the app a news app / does it collect personal data of minors? **No**.
- Expected result: **3+ / Everyone**, with "In-App Purchases".

## 5. Packages

1. Drag `JBD_1.0.0.0_x64.msix` into the upload box. Partner Center checks that the identity is
   `Jabrod.JBD` with the publisher `CN=B31502B4-…`; the packaging script already checked it.
   - If it says the version is already used (a 1.0.0.0 package from the first JBD was published
     before), set `[workspace.package] version = "1.0.1"` in `Cargo.toml`, run the packaging
     again and upload `JBD_1.0.1.0_x64.msix`.
2. **Device family availability:** only **Windows 10/11 Desktop** checked. Uncheck Xbox, Team,
   Holographic and the rest.
3. Leave "Let Microsoft decide whether to make this app available to future device families"
   **unchecked**.

Save.

## 6. Store listings

Add the language **English (United States)** (the package says `en-us`), then open it.

| Field | What to put |
|---|---|
| Product name | **JBD** (fixed by the reservation) |
| Description | "Description" in STORE-LISTING.md |
| What's new in this version | "What's new in this version" in STORE-LISTING.md |
| Product features | the 12 lines under "Features" in STORE-LISTING.md, one per box |
| Screenshots (Desktop) | `screenshots\01-editor.png` … `07-signin.png` in that order. Captions: Editor: "Multi-track timeline, preview and inspector"; Welcome: "Start from a template for every shape you post"; Export: "Export up to 4K"; Account: "Your Jabrod plan and AI credits"; Director: "The Director is coming next"; Transcript: "Speech to text on your computer"; Sign in: "Sign in with your Jabrod account" |
| Store logos → 1:1 Box art (2160×2160) | `store-box.png` |
| Store logos → 9:16 Poster art (720×1080 or 1440×2160) | `store-poster.png` |
| Store logos → 1:1 App tile icon (300×300) | optional; skip (the package's own icons are used) |
| Super hero art (16:9, 3840×2160) | `store-hero.png` |
| Trailers | optional. Add `jbd-media\out\video\jbd-launch-1080p.mp4` with a 1920×1080 PNG thumbnail (a frame of the film); title "JBD 1.0". A trailer needs the super hero art, which is above |
| Search terms (up to 7) | `video editor`, `reels editor`, `youtube editor`, `shorts editor`, `4k video editor`, `video maker`, `jabrod`. Never other companies' names (a policy failure) |
| Copyright and trademark info | `© 2026 Jabrod. JBD and the JBD logo are trademarks of Jabrod.` |
| Additional license terms | `https://jabrod.com/terms` |
| Developed by | `Jabrod` |
| Short title, Sort title, Voice title | `JBD` |
| Short description | "Short description" in STORE-LISTING.md |
| Additional system requirements | `Windows 10 version 1809 or later, 64-bit. 8 GB RAM (16 GB recommended). Internet connection to sign in.` |

Save.

## 7. Submission options

- **Publishing hold options:** choose **"Don't publish this submission until I select Publish
  now"**. You check the listing after certification, then publish with one click.
- **Notes for certification** (paste and fill in the test account):

  ```
  JBD is a desktop video editor. It asks for a free Jabrod account on first launch:
  click "Sign in", the default browser opens jabrod.com, sign in there, and JBD continues
  by itself (the browser returns to http://127.0.0.1 on this PC).

  Test account (JBD Pro): <email>  /  <password>

  To test: on the start screen choose Landscape, click Import (Media panel) to add a video,
  double-click it to put it on the timeline, press Space to play, add Text from the Media
  panel, then Export (top right). JBD Pro exports up to 4K without a watermark.

  JBD Pro is bought on jabrod.com (a non-game desktop app using its own checkout).
  The runFullTrust capability is needed because JBD is a Win32 desktop program.
  ```

- **Restricted capabilities:** Partner Center asks why the package needs **runFullTrust**.
  Paste: `JBD is a Win32 desktop video editor (a single native executable). It needs full trust to
  run as a desktop app, read and write the user's media files, and use the GPU for playback.`

## 8. Submit

Click **Submit to the Store**. Certification usually takes 1–3 business days. Partner Center
emails you if something fails, with the reason and the step to fix. Once it passes, open the
listing preview and click **Publish now**.

## Updates later

Each new version: raise the version in `Cargo.toml` (the fourth Store field stays 0), run
`package-jbd.ps1`, start a new submission, replace the package in step 5 and update "What's new"
in step 6. Everything else carries over.
