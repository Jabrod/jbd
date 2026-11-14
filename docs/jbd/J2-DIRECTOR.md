# J2 — The AI Director (design)

Agreed with the owner on 2026-10-14/15. The Director is JBD's own agent, in the spirit of Claude
Code: you chat with it, it plans, runs tools step by step in front of you, can drive the whole
editor, and can generate video clips. It must never crash JBD.

## Decisions
- The agent loop runs **inside JBD** (crate `crates/jbd-director`, plain `std::thread` + `ureq`, no
  async runtime). jabrod.com is only the model proxy: it holds keys, credits and the model lists.
- Thinking models: any model on the server allow-list (Claude, GPT, Gemini, Grok, DeepSeek, Qwen…),
  via OpenRouter first, Vercel AI Gateway as fallback (unchanged).
- Video models: through **Vercel AI Gateway** (our existing key). OpenRouter has no video models.
  Sora, Runway and Luma are not reachable with our keys and are out of scope.
- Credits are **money based: 1 credit = $0.01 of our provider cost**, computed from
  `platform.usage_events.cost_usd` (already recorded). Monthly budgets: Free 30, Trial 100,
  Pro 250. Top-up packs (Dodo) come later. **No database schema change.**
- Permission modes like Claude Code: **Ask** (every tool asks), **Auto edits** (default: edits run,
  anything that spends credits asks first), **Plan** (read-only tools only; proposes, then the user
  switches mode to run).
- One request = one undo step ("Director: <request>"), via a new history group in the engine.

## Server contract (jbd_home)
All routes take `?client_id=jbd` and `Authorization: Bearer <access token>`.

`GET /api/ai/models` →
```json
{ "chat":  [{ "id": "anthropic/claude-sonnet-5.5", "label": "Claude Sonnet 5.5", "vendor": "Anthropic",
              "inputPerM": 3, "outputPerM": 15, "default": true }],
  "video": [{ "id": "google/veo-3.1-lite-generate-001", "label": "Veo 3.1 Lite", "vendor": "Google",
              "durations": [4, 6, 8], "resolutions": ["720p", "1080p"], "aspects": ["16:9", "9:16"],
              "audio": true, "pricePerSecond": { "720p": 0.03, "1080p": 0.05 },
              "audioPricePerSecond": { "720p": 0.05, "1080p": 0.08 }, "default": true }],
  "credits": { "used": 68, "limit": 250 } }
```
`pricePerSecond` is in USD; the app shows `ceil(seconds * price * 100)` credits.

`POST /api/ai/chat` — as today, but `model` may also be any `chat[].id` from the list (`fast`/`best`
still work for the 1.0 app). Refuses with 429 `limit_reached` when credits used ≥ limit. Each call is
recorded with its cost (OpenRouter `usage.cost`; for the fallback, estimated from token prices).
The answer gains `usage.credits: { used, limit }`.

`POST /api/ai/video` body `{ model, prompt, seconds, aspect, resolution, audio }` → validates against
the list, checks that the up-front cost fits the remaining credits, records it (`kind 'ai.video'`),
starts a gateway job, and answers `{ job: "<signed token>", credits: <charged>, usage: { credits } }`.
`GET /api/ai/video/status?job=<token>` → `{ status: "pending" }`,
`{ status: "completed", url }` (or `data_base64` when the gateway returned bytes), or
`{ status: "error", error }`. A failed job is refunded once (`kind 'ai.video.refund'`, negative
cost, keyed by the job id). The job token is a signed JWT bound to the person; no job table.

`GET /api/desktop/me` gains `usage.month.credits: { used, limit }`.

## App side (jbd-v1.0)
- `crates/jbd-director`: model client, agent loop, tools, permission modes, cancellation. Pure logic
  is unit-tested with a scripted fake model and fake editor.
- The agent never touches the `Session`. It sends `engine.execute` / queries over a channel that
  `FilmcraftApp` drains each frame (same pattern as the control bridge), so every edit goes through
  `Session::execute` (panic-guarded, undoable, autosaved).
- Tools given to the model: `list_commands`, `inspect`, `run_command`, `generate_video`,
  `todo_write`, `ask_user`, `finish`. Generated clips are downloaded into the project's media folder,
  then `file.import` + `timeline.place`.
- Crash safety: the agent thread body runs in `catch_unwind`; no `unwrap` on network data; timeouts
  on every call; Stop cancels between steps and during video polling.
- UI (`crates/ui-egui/src/jbd/director.rs`): transcript with collapsible tool cards, live to-do list,
  generation cards that show the credit cost before spending, thinking/video model pickers, mode
  dropdown, Stop, Undo all, credits meter.

## Editing the person's footage (1.2.0)
Until 1.1 the Director's tools were built for making new material (generate shots, stock, voice);
editing what the person shot was only reachable through raw `run_command` calls in ticks, and the
system prompt steered it toward generating. Asked to "cut the silences", it often generated instead.
1.2 gives it two tools for the person's own clips:

- `read_footage` (`crates/jbd-director/src/footage.rs`): what is on the timeline, in timeline
  seconds. `transcript` (from the engine's transcripts; clips without one are transcribed on a
  worker thread with the speech model from Settings, then stored with `transcript.set`),
  `silences` (from the sound level: a per-clip threshold between the noise floor and the speech,
  short blips ignored, short breaths bridged; a pause is where no sound clip is loud; each pause is
  padded 0.12 s so a cut never clips a word), `beats` (the Remix beat tracker) and `scenes` (the
  Scene Edit Detection detector, sampled). The decoding runs in the app on a thread of its own
  (`director_run.rs`, editor methods `jbd.analyze` / `jbd.transcribe`), never on the UI thread.
- `edit_timeline` (`crates/jbd-director/src/edits.rs`): one plan per request, in seconds:
  `cut`, `keep`, `split`, `trim`, `delete`, `order`, `transition`, `title`, `captions`, `volume`,
  `music`, `speed`. Each maps to the editor's own commands (Extract over every sync-locked track for
  cuts, trims and ripple deletes that would break sync; Razor; Move; transitions; Audio Gain;
  `transcript.createCaptions`; Essential Sound ducking for music), so it lands on the timeline as
  ordinary undoable steps, folded into the request's one "Director: …" undo step.
- **Review before it happens.** A plan that reshapes the footage (cut, keep, trim, delete, order,
  speed) shows a preview card first, also in Auto edits: today's timeline with what goes shaded,
  the length after, one line per change, Apply / Skip. Ask mode previews every plan. In Plan mode
  the tool answers with the plan as text and changes nothing.
- PROJECT NOW also says which media is not on the timeline yet and which items have transcripts.

Tests: `engine_tests.rs` runs both tools on the real engine with real files (a WAV with pauses, a
still; the demo footage), including undo back to the exact timeline. With real camera-like clips in
`target/fixtures/director/` it also checks H.264/AAC files (made with ffmpeg as a test oracle):

```powershell
cd target\fixtures\director
ffmpeg -f lavfi -i "testsrc2=size=1280x720:rate=30:duration=9.5" -f lavfi -i "aevalsrc='0.3*sin(2*PI*220*t)*(1+0.3*sin(6*PI*t))*(1-between(t,2,3.5))*(1-between(t,6.5,6.6))*lt(t,7.6)+0.002*(random(0)-0.5)':s=48000:d=9.5" -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest talk-a.mp4
ffmpeg -f lavfi -i "smptebars=size=1280x720:rate=30:duration=7" -f lavfi -i "aevalsrc='0.25*sin(2*PI*180*t)*(1-between(t,0,1.2))*(1-between(t,3,4.4))+0.002*(random(0)-0.5)':s=48000:d=7" -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest talk-b.mp4
ffmpeg -f lavfi -i "aevalsrc='0.6*sin(2*PI*60*t)*exp(-18*mod(t,0.5))+0.25*sin(2*PI*880*t)*exp(-40*mod(t+0.25,0.5))+0.08*sin(2*PI*330*t)':s=44100:d=24" -c:a aac beat-120.m4a
```

Exercise for the owner: add an `op` to `edits.rs` (say `"mute"`, which turns a clip's sound off
with `clip.enable` or a -96 dB gain): the enum, `parse`, `describe`, `apply`, the tool description in
`tools.rs`, and a test in `engine_tests.rs`.

## Local development
Debug builds of JBD talk to `http://localhost:3000`: run jbd_home with `npm run dev`, then
`target/debug/jbd.exe`. jbd_home uses the shared production Neon database, which is why the server
work above avoids schema changes.
