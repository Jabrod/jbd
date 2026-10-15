//! The Director's production tools: the few big steps a real edit is made of, each done in one
//! call instead of dozens of small commands (faster, cheaper in credits, and fewer chances for
//! the model to get a detail wrong).
//!
//! - [`generate_shots`]: make several clips at once, all in parallel, and lay them out in order;
//! - [`assemble`]: put existing media on the timeline in order, trimmed, with transitions;
//! - [`add_title`]: a title that fits the frame (sized to the width, wrapped, centred, safe area);
//! - [`voiceover`]: narration from a script, on its own audio track.
//!
//! They only use engine commands through [`Editor`](crate::Editor), so every edit is undoable.
//! Their schemas are in `tools::definitions`; how the model should use them is in `agent::SYSTEM`.

use std::time::Instant;

use serde_json::{Value, json};

use crate::tools::{AUTO_CAP, Ctx, Outcome, ask, clip};
use crate::{VideoStatus, lock};

/// Ticks per second (`filmcraft_time::Tick`).
const TICKS: f64 = 254_016_000_000.0;

pub(crate) fn secs_to_ticks(s: f64) -> i64 {
    (s.max(0.0) * TICKS).round() as i64
}

/// The open timeline's facts the tools need: frame size, the end of the main video track, how
/// many audio tracks there are.
pub(crate) struct Timeline {
    pub width: f64,
    pub height: f64,
    pub v1_end: f64,
    pub audio_tracks: usize,
    /// Frames per second.
    pub frame_rate: f64,
}

pub(crate) fn timeline(ctx: &Ctx) -> Result<Timeline, String> {
    let s = ctx.editor.call("engine.execute", json!({"command": "sequence.inspect", "params": {}}))?;
    if s.get("id").is_none() {
        return Err("no timeline is open. Create one first (file.newSequence or the start screen).".into());
    }
    let set = s.get("settings");
    let num = |k: &str, d: f64| set.and_then(|x| x.get(k)).and_then(Value::as_f64).unwrap_or(d);
    let v1_end = s
        .get("video")
        .and_then(Value::as_array)
        .and_then(|t| t.first())
        .and_then(|t| t.get("items"))
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(|c| c.get("end").and_then(Value::as_f64)).fold(0.0, f64::max) / TICKS)
        .unwrap_or(0.0);
    let audio_tracks = s.get("audio").and_then(Value::as_array).map_or(0, Vec::len);
    let rate = set.and_then(|x| x.get("frame_rate"));
    let frame_rate = match (rate.and_then(|r| r.get("num")).and_then(Value::as_f64), rate.and_then(|r| r.get("den")).and_then(Value::as_f64)) {
        (Some(n), Some(d)) if d > 0.0 && n > 0.0 => n / d,
        _ => 30.0,
    };
    Ok(Timeline { width: num("width", 1920.0), height: num("height", 1080.0), v1_end, audio_tracks, frame_rate })
}

pub(crate) fn exec(ctx: &Ctx, command: &str, params: Value) -> Result<Value, String> {
    ctx.editor.call("engine.execute", json!({"command": command, "params": params}))
}

/// The clip ids a `timeline.place` answer names (video first).
pub(crate) fn placed_clips(v: &Value) -> Vec<u64> {
    v.get("clips").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default()
}

// ------------------------------------------------------------------------------------------ shots

struct Shot {
    prompt: String,
    seconds: u32,
    audio: bool,
    credits: u32,
}

/// How a clip was made, saved next to it as `<clip>.jbd.json` so it can be regenerated, edited
/// or varied later ("Regenerate" in the Director tab, the `regenerate_clip` tool).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Generation {
    pub prompt: String,
    pub model: String,
    pub model_label: String,
    pub seconds: u32,
    pub aspect: String,
    pub resolution: String,
    pub audio: bool,
}

/// The recipe file for a generated clip.
pub fn recipe_path(clip: &std::path::Path) -> std::path::PathBuf {
    let mut p = clip.as_os_str().to_owned();
    p.push(".jbd.json");
    std::path::PathBuf::from(p)
}

/// The recipe of a generated clip, if it has one.
pub fn read_recipe(clip: &std::path::Path) -> Option<Generation> {
    let text = std::fs::read_to_string(recipe_path(clip)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Make clips: ask if needed, start every job at once, wait for all of them together, save each
/// clip's recipe beside it. Returns the finished files (with their shot index), the credits
/// charged and any problems; `Err` is a finished outcome (said no, stopped, nothing started).
fn make_clips(
    ctx: &Ctx,
    step: u64,
    model: &crate::VideoModel,
    shots: &[Shot],
    aspect: &str,
    resolution: &str,
    title: &str,
) -> Result<(Vec<(usize, std::path::PathBuf)>, u32, Vec<String>), Outcome> {
    let total: u32 = shots.iter().map(|s| s.credits).sum();
    lock(ctx.shared).update_step(step, |s| s.credits = total);

    // Ask mode asks; Auto edits asks only past the per-request cap. One question for the batch.
    let over_cap = ctx.spent.get().saturating_add(total) > AUTO_CAP;
    if !crate::tools::may_spend(ctx, total) {
        let mut detail = format!("{} · {} clip{} · {aspect} · {resolution}", model.label, shots.len(), if shots.len() == 1 { "" } else { "s" });
        for (i, s) in shots.iter().enumerate().take(6) {
            detail.push_str(&format!("\n{}. {} s{}: {}", i + 1, s.seconds, if s.audio { " + sound" } else { "" }, clip(&s.prompt, 70)));
        }
        let title = if over_cap { format!("{title} (this request has used {} credits)", ctx.spent.get()) } else { title.to_string() };
        if !ask(ctx, step, &title, &detail, total) {
            return Err(Outcome {
                text: "The person chose not to generate this. Don't retry; continue without it or ask.".into(),
                ok: false,
                edited: false,
                credits: 0,
            });
        }
    }

    // Start the jobs a few at a time (each start is a network round trip), stopping between
    // batches if the person pressed Stop; then wait for all of them together. Every started job
    // is written down at once, so a clip already paid for can still be fetched if JBD closes.
    let dir = ctx.editor.media_dir();
    let mut jobs: Vec<Option<String>> = vec![None; shots.len()];
    let mut errors: Vec<String> = Vec::new();
    let mut charged = 0u32;
    for (batch, chunk) in shots.chunks(START_AT_ONCE).enumerate() {
        if ctx.cancel.stopped() {
            errors.push(format!("{} shot(s) weren't started (stopped)", shots.len().saturating_sub(batch * START_AT_ONCE)));
            break;
        }
        let started: Vec<Result<crate::VideoJob, String>> = std::thread::scope(|sc| {
            let handles: Vec<_> =
                chunk.iter().map(|s| sc.spawn(|| ctx.model.start_video(&model.id, &s.prompt, s.seconds, aspect, resolution, s.audio))).collect();
            handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("starting the clip failed".into()))).collect()
        });
        for (k, r) in started.into_iter().enumerate() {
            let i = batch * START_AT_ONCE + k;
            match r {
                Ok(j) => {
                    charged = charged.saturating_add(j.credits);
                    if let Some(s) = shots.get(i) {
                        remember_job(&dir, PendingJob { job: j.job.clone(), recipe: recipe_of(s, model, aspect, resolution) });
                    }
                    if let Some(slot) = jobs.get_mut(i) {
                        *slot = Some(j.job);
                    }
                }
                Err(e) => errors.push(format!("shot {}: {e}", i + 1)),
            }
        }
    }
    ctx.spent.set(ctx.spent.get().saturating_add(charged));
    if jobs.iter().all(Option::is_none) {
        if ctx.cancel.stopped() {
            return Err(Outcome { text: "Stopped by the person before any clip started.".into(), ok: false, edited: false, credits: charged });
        }
        return Err(Outcome::err(format!("nothing could be started: {}", errors.join("; "))));
    }
    lock(ctx.shared).activity =
        Some(format!("Generating {} clip{} with {}…", jobs.iter().flatten().count(), if shots.len() == 1 { "" } else { "s" }, model.label));
    (ctx.repaint)();

    let started = Instant::now();
    let mut done: Vec<Option<std::path::PathBuf>> = vec![None; jobs.len()];
    let expected = 90.0 + shots.iter().map(|s| s.seconds).max().unwrap_or(6) as f32 * 10.0;
    loop {
        if ctx.cancel.stopped() {
            // the started clips are paid for and stay in the jobs file: the Director tab offers
            // to fetch them into the project
            return Err(Outcome {
                text: "Stopped by the person while the clips were generating. The clips already started will be offered to fetch into the project.".into(),
                ok: false,
                edited: false,
                credits: charged,
            });
        }
        let mut pending = 0;
        for (i, job) in jobs.iter_mut().enumerate() {
            let Some(id) = job.clone() else { continue };
            match ctx.model.video_status(&id, &dir) {
                Ok(VideoStatus::Done(p)) => {
                    if let Some(s) = shots.get(i)
                        && let Ok(text) = serde_json::to_string_pretty(&recipe_of(s, model, aspect, resolution))
                    {
                        let _ = std::fs::write(recipe_path(&p), text);
                    }
                    forget_job(&dir, &id);
                    if let Some(slot) = done.get_mut(i) {
                        *slot = Some(p);
                    }
                    *job = None;
                }
                Ok(VideoStatus::Failed(e)) => {
                    errors.push(format!("shot {} failed ({e}); its credits were refunded", i + 1));
                    forget_job(&dir, &id);
                    *job = None;
                }
                // a network blip while polling is not a failed job: keep waiting
                Ok(VideoStatus::Pending) | Err(_) => pending += 1,
            }
        }
        let finished = done.iter().flatten().count() as f32;
        let total_n = shots.len().max(1) as f32;
        let time_part = (started.elapsed().as_secs_f32() / expected).min(0.95);
        lock(ctx.shared).update_step(step, |s| s.progress = Some(((finished / total_n) * 0.6 + time_part * 0.4).min(0.97)));
        (ctx.repaint)();
        if pending == 0 {
            break;
        }
        if started.elapsed().as_secs() > 15 * 60 {
            errors.push("some clips took longer than 15 minutes and were left out".into());
            break;
        }
        for _ in 0..50 {
            if ctx.cancel.stopped() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    lock(ctx.shared).update_step(step, |s| s.progress = None);
    let ready: Vec<(usize, std::path::PathBuf)> = done.into_iter().enumerate().filter_map(|(i, p)| p.map(|p| (i, p))).collect();
    if ready.is_empty() {
        return Err(Outcome { text: format!("Error: no clip was made. {}", errors.join("; ")), ok: false, edited: false, credits: charged });
    }
    Ok((ready, charged, errors))
}

/// Video jobs started at the same time (each start is a request to jabrod.com).
const START_AT_ONCE: usize = 3;

fn recipe_of(s: &Shot, model: &crate::VideoModel, aspect: &str, resolution: &str) -> Generation {
    Generation {
        prompt: s.prompt.clone(),
        model: model.id.clone(),
        model_label: model.label.clone(),
        seconds: s.seconds,
        aspect: aspect.to_string(),
        resolution: resolution.to_string(),
        audio: s.audio,
    }
}

// ---------------------------------------------------------------------------- jobs kept on disk

/// A clip being made (paid for, not downloaded yet), written to `pending-clips.json` in the
/// media folder so it survives JBD closing or the person pressing Stop.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PendingJob {
    pub job: String,
    pub recipe: Generation,
}

/// The jobs file of a media folder.
pub fn jobs_file(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("pending-clips.json")
}

/// The clips still being made for this media folder.
pub fn pending_jobs(dir: &std::path::Path) -> Vec<PendingJob> {
    std::fs::read_to_string(jobs_file(dir)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn write_jobs(dir: &std::path::Path, jobs: &[PendingJob]) {
    let path = jobs_file(dir);
    if jobs.is_empty() {
        let _ = std::fs::remove_file(path);
        return;
    }
    let _ = std::fs::create_dir_all(dir);
    if let Ok(text) = serde_json::to_string_pretty(jobs) {
        let _ = std::fs::write(path, text);
    }
}

fn remember_job(dir: &std::path::Path, job: PendingJob) {
    let mut all = pending_jobs(dir);
    all.retain(|j| j.job != job.job);
    all.push(job);
    write_jobs(dir, &all);
}

fn forget_job(dir: &std::path::Path, job: &str) {
    let mut all = pending_jobs(dir);
    let before = all.len();
    all.retain(|j| j.job != job);
    if all.len() != before {
        write_jobs(dir, &all);
    }
}

/// Check every remembered job once: finished clips are downloaded into `dir` with their recipe
/// beside them and forgotten; failed ones are forgotten (their credits were refunded). Returns
/// the new files, how many are still being made, and what failed.
pub fn fetch_pending(model: &dyn crate::Model, dir: &std::path::Path) -> (Vec<std::path::PathBuf>, usize, Vec<String>) {
    let mut files = Vec::new();
    let mut errors = Vec::new();
    let mut still = 0;
    for j in pending_jobs(dir) {
        match model.video_status(&j.job, dir) {
            Ok(VideoStatus::Done(p)) => {
                if let Ok(text) = serde_json::to_string_pretty(&j.recipe) {
                    let _ = std::fs::write(recipe_path(&p), text);
                }
                forget_job(dir, &j.job);
                files.push(p);
            }
            Ok(VideoStatus::Failed(e)) => {
                forget_job(dir, &j.job);
                errors.push(format!("a clip failed ({e}); its credits were refunded"));
            }
            Ok(VideoStatus::Pending) | Err(_) => still += 1,
        }
    }
    (files, still, errors)
}

/// Import files; returns their item ids in the same order (and notes a short import).
fn import(ctx: &Ctx, files: &[std::path::PathBuf], errors: &mut Vec<String>) -> Result<Vec<u64>, String> {
    let paths: Vec<String> = files.iter().map(|p| p.to_string_lossy().to_string()).collect();
    let v = exec(ctx, "file.import", json!({"paths": paths}))?;
    let items: Vec<u64> = v.get("items").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
    if items.len() < files.len() {
        errors.push(format!("only {} of {} clips imported; the rest are in {}", items.len(), files.len(), ctx.editor.media_dir().display()));
    }
    Ok(items)
}

/// Make several clips at once. They are generated in parallel (all jobs start together, so the
/// wait is the slowest clip, not the sum), imported, and laid out in shot order.
pub fn generate_shots(ctx: &Ctx, step: u64, args: &Value) -> Outcome {
    let Some(model) = ctx.catalogue.video(ctx.video_model).or_else(|| ctx.catalogue.default_video()) else {
        return Outcome::err("no video model is available right now (the model list didn't load).");
    };
    let raw: Vec<Value> = match args.get("shots").and_then(Value::as_array) {
        Some(a) => a.clone(),
        // a single clip given the old way
        None => vec![args.clone()],
    };
    let mut aspect = args.get("aspect").and_then(Value::as_str).unwrap_or("16:9").to_string();
    if !model.aspects.is_empty() && !model.aspects.contains(&aspect) {
        aspect = model.aspects.first().cloned().unwrap_or(aspect);
    }
    let resolution = model.best_resolution(None);
    let shots: Vec<Shot> = raw
        .iter()
        .take(12)
        .filter_map(|s| {
            let prompt = s.get("prompt").and_then(Value::as_str)?.trim().to_string();
            if prompt.is_empty() {
                return None;
            }
            let wanted = s.get("seconds").and_then(Value::as_u64).map(|x| x.min(60) as u32).unwrap_or(6);
            let seconds = model.nearest_duration(wanted);
            let audio = ctx.sound.resolve(s.get("audio").and_then(Value::as_bool).unwrap_or(false), model.audio);
            let credits = model.credits_for(seconds, &resolution, audio);
            Some(Shot { prompt, seconds, audio, credits })
        })
        .collect();
    if shots.is_empty() {
        return Outcome::err("no shots: give `shots` as [{prompt, seconds, audio}]");
    }
    let title = format!("Generate {} clip{}", shots.len(), if shots.len() == 1 { "" } else { "s" });
    let (ready, charged, mut errors) = match make_clips(ctx, step, model, &shots, &aspect, &resolution, &title) {
        Ok(r) => r,
        Err(outcome) => return outcome,
    };
    let files: Vec<std::path::PathBuf> = ready.iter().map(|(_, p)| p.clone()).collect();
    let items = match import(ctx, &files, &mut errors) {
        Ok(i) => i,
        Err(e) => return Outcome { text: format!("Error: the clips were made but importing them failed: {e}"), ok: false, edited: false, credits: charged },
    };

    // Lay them out, back to back, from `place_at_seconds` (default: the end of the main track).
    let place = args.get("place_at_seconds").and_then(Value::as_f64);
    let mut lines = Vec::new();
    if place.is_some_and(|p| p < 0.0) {
        for ((i, _), item) in ready.iter().zip(&items) {
            lines.push(format!("shot {} → item {item} (not placed)", i + 1));
        }
    } else {
        let mut t = match place {
            Some(p) => p,
            None => timeline(ctx).map(|tl| tl.v1_end).unwrap_or(0.0),
        };
        for ((i, _), item) in ready.iter().zip(&items) {
            let secs = shots.get(*i).map_or(6.0, |s| f64::from(s.seconds));
            match exec(ctx, "timeline.place", json!({"item": item, "seconds": t, "track": "V1"})) {
                Ok(v) => {
                    // cover the frame: generated clips are often smaller than the timeline
                    fit_clips(ctx, &placed_clips(&v), "fill");
                    lines.push(format!("shot {} → item {item}, clip {:?}, {t:.1}–{:.1} s", i + 1, placed_clips(&v).first().copied().unwrap_or(0), t + secs))
                }
                Err(e) => lines.push(format!("shot {} → item {item}, placing failed: {e}", i + 1)),
            }
            t += secs;
        }
    }
    let mut text = format!("Generated {} of {} clips with {} ({} credits).\n{}", ready.len(), shots.len(), model.label, charged, lines.join("\n"));
    if !errors.is_empty() {
        text.push_str(&format!("\nProblems: {}", errors.join("; ")));
    }
    Outcome { text, ok: true, edited: true, credits: charged }
}

/// Make a new take of a clip the Director generated before, from its saved recipe: replace it in
/// place (same position and length on the timeline), or, as a variation, put the new take on the
/// track above it so both can be compared.
pub fn regenerate_clip(ctx: &Ctx, step: u64, args: &Value) -> Outcome {
    let Some(clip_id) = args.get("clip").and_then(Value::as_u64) else { return Outcome::err("missing `clip` (a clip id from PROJECT NOW)") };
    let seq = match exec(ctx, "sequence.inspect", json!({})) {
        Ok(s) => s,
        Err(e) => return Outcome::err(e),
    };
    // find the clip: its media item, start and track number
    let mut found: Option<(u64, f64, usize)> = None;
    for (ti, track) in seq.get("video").and_then(Value::as_array).into_iter().flatten().enumerate() {
        for c in track.get("items").and_then(Value::as_array).into_iter().flatten() {
            if c.get("clip").and_then(Value::as_u64) == Some(clip_id) {
                found = Some((c.get("item").and_then(Value::as_u64).unwrap_or(0), c.get("start").and_then(Value::as_f64).unwrap_or(0.0) / TICKS, ti));
            }
        }
    }
    let Some((item, start, track_index)) = found else { return Outcome::err(format!("there is no video clip {clip_id} on the timeline")) };
    let path = match ctx.editor.call("jbd.mediaPath", json!({"item": item})) {
        Ok(v) => v.get("path").and_then(Value::as_str).map(std::path::PathBuf::from),
        Err(_) => None,
    };
    let Some(recipe) = path.as_deref().and_then(read_recipe) else {
        return Outcome::err(
            "this clip wasn't generated by the Director (or its recipe file is missing), so it can't be regenerated. Use generate_shots to make a new clip instead.",
        );
    };
    let model = ctx.catalogue.video(&recipe.model).or_else(|| ctx.catalogue.video(ctx.video_model)).or_else(|| ctx.catalogue.default_video());
    let Some(model) = model else { return Outcome::err("no video model is available right now.") };
    let variation = args.get("variation").and_then(Value::as_bool).unwrap_or(false);
    let mut prompt = args.get("prompt").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty()).unwrap_or(&recipe.prompt).to_string();
    if variation && args.get("prompt").is_none() {
        prompt.push_str(" A different take: new camera angle and timing, the same subject, look and light.");
    }
    let seconds = model.nearest_duration(recipe.seconds);
    let resolution = model.best_resolution(Some(&recipe.resolution));
    let audio = recipe.audio && model.audio;
    let shot = Shot { credits: model.credits_for(seconds, &resolution, audio), prompt, seconds, audio };
    let title = if variation { "Generate a variation" } else { "Regenerate the clip" };
    let (ready, charged, mut errors) = match make_clips(ctx, step, model, std::slice::from_ref(&shot), &recipe.aspect, &resolution, title) {
        Ok(r) => r,
        Err(outcome) => return outcome,
    };
    let files: Vec<std::path::PathBuf> = ready.into_iter().map(|(_, p)| p).collect();
    let items = match import(ctx, &files, &mut errors) {
        Ok(i) => i,
        Err(e) => return Outcome { text: format!("Error: the new take was made but importing it failed: {e}"), ok: false, edited: false, credits: charged },
    };
    let Some(new_item) = items.first().copied() else {
        return Outcome { text: format!("Error: the new take didn't import. {}", errors.join("; ")), ok: false, edited: false, credits: charged };
    };
    let text = if variation {
        // the track above, made if it doesn't exist yet
        let tracks = seq.get("video").and_then(Value::as_array).map_or(1, Vec::len);
        if track_index + 1 >= tracks {
            let _ = exec(ctx, "sequence.addTracks", json!({"video": 1, "audio": 0}));
        }
        let track = format!("V{}", track_index + 2);
        match exec(ctx, "timeline.place", json!({"item": new_item, "seconds": start, "track": track})) {
            Ok(v) => {
                fit_clips(ctx, &placed_clips(&v), "fill");
                format!(
                    "Made a variation (item {new_item}) and put it on {track} above clip {clip_id}, at {start:.1} s, so both can be compared. Delete the one you don't want."
                )
            }
            Err(e) => format!("Made a variation (item {new_item}) but placing it failed: {e}"),
        }
    } else {
        match exec(ctx, "clip.replaceFromBin", json!({"clips": [clip_id], "item": new_item, "keepSourceIn": false})) {
            Ok(_) => {
                fit_clips(ctx, &[clip_id], "fill");
                format!("Regenerated clip {clip_id} in place with {} (new item {new_item}); its position, length and effects are unchanged.", model.label)
            }
            Err(e) => format!("Made the new take (item {new_item}) but replacing the clip failed: {e}"),
        }
    };
    let mut text = format!("{text} ({charged} credits)");
    if !errors.is_empty() {
        text.push_str(&format!("\nProblems: {}", errors.join("; ")));
    }
    Outcome { text, ok: true, edited: true, credits: charged }
}

// --------------------------------------------------------------------------------------- assemble

/// Put media on the timeline in order, each optionally trimmed, with a transition between them.
pub fn assemble(ctx: &Ctx, args: &Value) -> Outcome {
    let Some(list) = args.get("clips").and_then(Value::as_array) else { return Outcome::err("missing `clips`: [{item, in_seconds?, out_seconds?}]") };
    let tl = match timeline(ctx) {
        Ok(t) => t,
        Err(e) => return Outcome::err(e),
    };
    // item lengths, for clips used whole
    let durations: std::collections::HashMap<u64, f64> = exec(ctx, "project.inspect", json!({}))
        .ok()
        .map(|p| {
            let mut m = std::collections::HashMap::new();
            collect_durations(p.get("root").unwrap_or(&Value::Null), &mut m);
            m
        })
        .unwrap_or_default();
    let track = args.get("track").and_then(Value::as_str).unwrap_or("V1").to_string();
    let mut t = args.get("start_at_seconds").and_then(Value::as_f64).unwrap_or(tl.v1_end);
    let mut placed: Vec<u64> = Vec::new();
    let mut lines = Vec::new();
    for c in list.iter().take(60) {
        let Some(item) = c.get("item").and_then(Value::as_u64) else { continue };
        let full = durations.get(&item).copied().unwrap_or(5.0);
        let a = c.get("in_seconds").and_then(Value::as_f64).unwrap_or(0.0).clamp(0.0, full);
        let b = c.get("out_seconds").and_then(Value::as_f64).unwrap_or(full).clamp(a, full.max(a));
        let len = (b - a).max(0.1);
        let mut params = json!({"item": item, "seconds": t, "track": track});
        if a > 0.0 || b < full {
            params["sourceIn"] = json!(secs_to_ticks(a));
            params["duration"] = json!(secs_to_ticks(len));
        }
        match exec(ctx, "timeline.place", params) {
            Ok(v) => {
                let id = placed_clips(&v).first().copied();
                lines.push(format!("item {item} ({a:.1}–{b:.1} s of the source) → clip {} at {t:.1}–{:.1} s", id.unwrap_or(0), t + len));
                if let Some(id) = id {
                    placed.push(id);
                }
            }
            Err(e) => lines.push(format!("item {item}: {e}")),
        }
        t += len;
    }
    // cover the frame ("fill", default), fit inside it, or leave the size alone
    fit_clips(ctx, &placed, args.get("fit").and_then(Value::as_str).unwrap_or("fill"));
    // transitions between neighbours
    let transition = args.get("transition").and_then(Value::as_str).unwrap_or("none");
    if transition != "none" && placed.len() > 1 {
        let frames = args.get("transition_frames").and_then(Value::as_i64).unwrap_or(12).clamp(2, 60);
        let mut ok = 0;
        for id in placed.iter().take(placed.len() - 1) {
            if exec(ctx, "sequence.applyVideoTransition", json!({"clip": id, "effect": transition, "frames": frames, "edge": "out"})).is_ok() {
                ok += 1;
            }
        }
        lines.push(format!("{transition} ({frames} frames) on {ok} cut{}", if ok == 1 { "" } else { "s" }));
    }
    Outcome {
        text: format!("Assembled {} clips on {track}, ending at {t:.1} s.\n{}", placed.len(), lines.join("\n")),
        ok: !placed.is_empty(),
        edited: !placed.is_empty(),
        credits: 0,
    }
}

fn collect_durations(node: &Value, out: &mut std::collections::HashMap<u64, f64>) {
    for child in node.get("children").and_then(Value::as_array).into_iter().flatten() {
        if child.get("children").is_some() {
            collect_durations(child, out);
        } else if let (Some(item), Some(d)) = (child.get("item").and_then(Value::as_u64), child.get("duration").and_then(Value::as_f64)) {
            out.insert(item, d / TICKS);
        }
    }
}

// ------------------------------------------------------------------------------- composition

/// Make footage cover the frame ("Fill frame": scaled up until there are no bars, the overflow
/// cropped), or fit inside it ("Fit to frame"). Generated clips are often smaller than the
/// timeline (720×1280 in a 1080×1920 reel), and the same aspect ratio doesn't make them fill it.
pub fn fit_clips(ctx: &Ctx, clips: &[u64], policy: &str) {
    if clips.is_empty() {
        return;
    }
    let command = match policy {
        "fit" => "clip.fitToFrame",
        "none" => return,
        _ => "clip.fillFrame",
    };
    let _ = exec(ctx, command, json!({"clips": clips}));
}

/// One clip on the open timeline: (start s, end s, track index, is video).
pub(crate) fn find_clip(seq: &Value, clip: u64) -> Option<(f64, f64, usize, bool)> {
    for (kind, video) in [("video", true), ("audio", false)] {
        for (ti, track) in seq.get(kind).and_then(Value::as_array).into_iter().flatten().enumerate() {
            for c in track.get("items").and_then(Value::as_array).into_iter().flatten() {
                if c.get("clip").and_then(Value::as_u64) == Some(clip) {
                    let s = c.get("start").and_then(Value::as_f64).unwrap_or(0.0) / TICKS;
                    let e = c.get("end").and_then(Value::as_f64).unwrap_or(0.0) / TICKS;
                    return Some((s, e, ti, video));
                }
            }
        }
    }
    None
}

/// How many keyframes `param` of `effect` has on `clip` (from sequence.inspect).
pub(crate) fn keyframes(seq: &Value, clip: u64, effect: &str, param: &str) -> u64 {
    for kind in ["video", "audio"] {
        for track in seq.get(kind).and_then(Value::as_array).into_iter().flatten() {
            for c in track.get("items").and_then(Value::as_array).into_iter().flatten() {
                if c.get("clip").and_then(Value::as_u64) != Some(clip) {
                    continue;
                }
                for e in c.get("effects").and_then(Value::as_array).into_iter().flatten() {
                    if e.get("effect").and_then(Value::as_str) == Some(effect) {
                        return e.get("params").and_then(|p| p.get(param)).and_then(|p| p.get("keyframes")).and_then(Value::as_u64).unwrap_or(0);
                    }
                }
            }
        }
    }
    0
}

/// Where a clip's framing is before it is animated: its scale (%), position (frame pixels) and
/// opacity (%). Presets move FROM and TO these, so a clip scaled to 150 % to fill the frame
/// zooms 150 → 168 %, never back to 100 % (which would bring the black borders back).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Base {
    pub scale: f64,
    pub position: [f64; 2],
    pub opacity: f64,
}

/// One parameter's keyframes as the engine has them: (seconds from the clip's start, value).
pub(crate) fn read_keyframes(ctx: &Ctx, clip: u64, effect: &str, param: &str) -> Option<(Value, Vec<(f64, Value)>)> {
    let v = exec(ctx, "effects.keyframes", json!({"clip": clip, "effect": effect, "param": param})).ok()?;
    let keys = v
        .get("keyframes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|k| Some((k.get("clipSeconds")?.as_f64()?, k.get("value")?.clone())))
        .collect();
    Some((v.get("value").cloned().unwrap_or(Value::Null), keys))
}

/// The clip's framing at its first frame (the first keyframe when animated). A position the
/// engine keeps as "auto" (centre) reads as the frame centre.
pub(crate) fn base_of(ctx: &Ctx, clip: u64, width: f64, height: f64) -> Base {
    let at_start = |effect: &str, param: &str| read_keyframes(ctx, clip, effect, param).map(|(v, k)| k.first().map(|f| f.1.clone()).unwrap_or(v));
    let num = |v: Option<Value>, d: f64| v.and_then(|v| v.as_f64()).filter(|x| x.is_finite()).unwrap_or(d);
    let position = at_start("motion", "position")
        .and_then(|v| Some([v.get(0)?.as_f64()?, v.get(1)?.as_f64()?]))
        .filter(|p| p[0].is_finite() && p[1].is_finite())
        .unwrap_or([width / 2.0, height / 2.0]);
    Base { scale: num(at_start("motion", "scale"), 100.0), position, opacity: num(at_start("opacity", "opacity"), 100.0) }
}

/// A preset as keyframes: (param, seconds from the clip's start, value), relative to `base`.
fn preset(name: &str, len: f64, height: f64, base: Base) -> Vec<(&'static str, f64, Value)> {
    let d = (len * 0.25).clamp(0.2, 0.5);
    let out = (len - d).max(d);
    let end = (len - 0.04).max(0.0);
    let (s, [x, y], o) = (base.scale, base.position, base.opacity);
    match name {
        "fade_in" => vec![("opacity", 0.0, json!(0.0)), ("opacity", d, json!(o))],
        "fade_out" => vec![("opacity", out, json!(o)), ("opacity", end, json!(0.0))],
        "fade_in_out" => vec![("opacity", 0.0, json!(0.0)), ("opacity", d, json!(o)), ("opacity", out, json!(o)), ("opacity", end, json!(0.0))],
        "pop_in" => vec![("scale", 0.0, json!(s * 0.6)), ("scale", d, json!(s)), ("opacity", 0.0, json!(0.0)), ("opacity", d * 0.6, json!(o))],
        "slide_up" => {
            vec![("position", 0.0, json!([x, y + height * 0.06])), ("position", d, json!([x, y])), ("opacity", 0.0, json!(0.0)), ("opacity", d, json!(o))]
        }
        // a slow push in over the whole clip (Ken Burns), from wherever the clip is framed now
        "zoom_in" => vec![("scale", 0.0, json!(s)), ("scale", end, json!(s * 1.12))],
        "zoom_out" => vec![("scale", 0.0, json!(s * 1.12)), ("scale", end, json!(s))],
        _ => Vec::new(),
    }
}

/// Do two parameter values match (within half a percent / a pixel)?
fn same_value(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() <= 0.5,
        _ => match (a.as_array(), b.as_array()) {
            (Some(x), Some(y)) => {
                x.len() == y.len() && x.iter().zip(y).all(|(p, q)| matches!((p.as_f64(), q.as_f64()), (Some(p), Some(q)) if (p - q).abs() <= 1.0))
            }
            _ => a == b,
        },
    }
}

/// Animate a clip with keyframes at times measured FROM THE CLIP'S START (0 = its first frame),
/// converted here to timeline time. Checks afterwards that the keyframes really exist.
pub fn animate(ctx: &Ctx, args: &Value) -> Outcome {
    let Some(clip_id) = args.get("clip").and_then(Value::as_u64) else { return Outcome::err("missing `clip`") };
    let seq = match exec(ctx, "sequence.inspect", json!({})) {
        Ok(s) => s,
        Err(e) => return Outcome::err(e),
    };
    let Some((start, end, _, _)) = find_clip(&seq, clip_id) else { return Outcome::err(format!("there is no clip {clip_id} on the timeline")) };
    let tl = timeline(ctx).ok();
    let (w, h) = tl.as_ref().map_or((1920.0, 1080.0), |t| (t.width, t.height));
    let len = (end - start).max(0.1);
    let mut keys: Vec<(String, f64, Value)> = match args.get("preset").and_then(Value::as_str) {
        Some(p) => preset(p, len, h, base_of(ctx, clip_id, w, h)).into_iter().map(|(a, b, c)| (a.to_string(), b, c)).collect(),
        None => Vec::new(),
    };
    for k in args.get("keyframes").and_then(Value::as_array).into_iter().flatten().take(40) {
        if let (Some(p), Some(at), Some(v)) = (k.get("param").and_then(Value::as_str), k.get("at_seconds").and_then(Value::as_f64), k.get("value")) {
            keys.push((p.to_string(), at, v.clone()));
        }
    }
    if keys.is_empty() {
        return Outcome::err("give a `preset` (fade_in, fade_out, fade_in_out, pop_in, slide_up, zoom_in, zoom_out) or `keyframes`");
    }
    // group by parameter, in time order
    let mut params: Vec<String> = keys.iter().map(|k| k.0.clone()).collect();
    params.dedup();
    params.sort();
    params.dedup();
    let mut notes = Vec::new();
    for param in &params {
        let effect = if param == "opacity" { "opacity" } else { "motion" };
        let mut ks: Vec<&(String, f64, Value)> = keys.iter().filter(|k| &k.0 == param).collect();
        ks.sort_by(|a, b| a.1.total_cmp(&b.1));
        // clip-relative → timeline time, kept inside the clip
        let at = |off: f64| (start + off.clamp(0.0, len - 0.02)).max(start);
        let Some(first) = ks.first() else { continue };
        let _ = exec(ctx, "effects.setParam", json!({"clip": clip_id, "effect": effect, "param": param, "value": first.2, "time": secs_to_ticks(at(first.1))}));
        if keyframes(&seq, clip_id, effect, param) == 0 {
            // switching animation on keys the value at the playhead: put it on the first keyframe
            let _ = exec(ctx, "playhead.set", json!({"seconds": at(first.1)}));
            if let Err(e) = exec(ctx, "effects.toggleAnimation", json!({"clip": clip_id, "effect": effect, "param": param})) {
                notes.push(format!("{param}: couldn't animate ({e})"));
                continue;
            }
        }
        for k in &ks {
            if let Err(e) =
                exec(ctx, "effects.setParam", json!({"clip": clip_id, "effect": effect, "param": param, "value": k.2, "time": secs_to_ticks(at(k.1))}))
            {
                notes.push(format!("{param} at {:.2} s: {e}", k.1));
            }
        }
    }
    // Verify every keyframe asked for, read back from the engine: at its time (within a
    // frame or two) and with its value. A count alone would pass keyframes at the wrong time or
    // with the wrong value.
    let speed = clip_speed(&seq, clip_id);
    let mut missing = Vec::new();
    let mut done = Vec::new();
    for param in &params {
        let effect = if param == "opacity" { "opacity" } else { "motion" };
        let have = read_keyframes(ctx, clip_id, effect, param).map(|(_, k)| k).unwrap_or_default();
        let mut good = 0;
        for (_, off, want) in keys.iter().filter(|k| &k.0 == param) {
            let off = off.clamp(0.0, len - 0.02);
            // keyframe times are in the clip's media time: a sped-up clip runs through them faster
            let media = off * speed;
            match have.iter().find(|(t, _)| (t - media).abs() <= 0.06 + media * 0.01) {
                Some((_, v)) if same_value(v, want) => good += 1,
                Some((_, v)) => missing.push(format!("{param} at {off:.2} s is {v}, not {want}")),
                None => missing.push(format!("{param} has no keyframe at {off:.2} s")),
            }
        }
        if good > 0 {
            done.push(format!("{param} ({good} keyframe{})", if good == 1 { "" } else { "s" }));
        }
    }
    missing.extend(notes);
    let what = format!("clip {clip_id} ({start:.1}–{end:.1} s on the timeline)");
    if missing.is_empty() {
        return Outcome { text: format!("Animated {what}: {}; every keyframe checked.", done.join(", ")), ok: true, edited: true, credits: 0 };
    }
    if done.is_empty() {
        return Outcome { text: format!("Error: couldn't animate {what}: {}", missing.join("; ")), ok: false, edited: true, credits: 0 };
    }
    Outcome::partial(format!("animated {what}: {}.", done.join(", ")), &missing, 0)
}

/// A clip's playback speed (1 = normal), from sequence.inspect.
fn clip_speed(seq: &Value, clip: u64) -> f64 {
    ["video", "audio"]
        .iter()
        .flat_map(|k| seq.get(*k).and_then(Value::as_array).into_iter().flatten())
        .flat_map(|t| t.get("items").and_then(Value::as_array).into_iter().flatten())
        .find(|c| c.get("clip").and_then(Value::as_u64) == Some(clip))
        .and_then(|c| c.get("speed").and_then(Value::as_f64))
        .map(f64::abs)
        .filter(|s| s.is_finite() && *s > 0.0)
        .unwrap_or(1.0)
}

/// Bounds of a title's text on the canvas, measured by the engine with the real font:
/// (x0, y0, x1, y1), and the canvas size.
fn title_bounds(ctx: &Ctx, clip: u64) -> Option<([f64; 4], (f64, f64))> {
    let v = exec(ctx, "graphics.list", json!({"clip": clip})).ok()?;
    let canvas = v.get("canvas").and_then(Value::as_array)?;
    let (cw, ch) = (canvas.first()?.as_f64()?, canvas.get(1)?.as_f64()?);
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for layer in v.get("layers").and_then(Value::as_array).into_iter().flatten() {
        for p in layer.get("quad").and_then(Value::as_array).into_iter().flatten() {
            if let (Some(x), Some(y)) = (p.get(0).and_then(Value::as_f64), p.get(1).and_then(Value::as_f64)) {
                b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
            }
        }
    }
    (b[0] < b[2]).then_some((b, (cw, ch)))
}

/// After a title is made: measure it and correct it until it sits centred inside the safe area
/// (90 % of the frame). Returns a note for the result.
fn settle_title(ctx: &Ctx, clip: u64, size: f64, target_y: f64) -> String {
    let mut size = size;
    for _ in 0..3 {
        let Some((b, (cw, ch))) = title_bounds(ctx, clip) else { return "(couldn't measure it)".into() };
        let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
        let too_big = bw > cw * 0.9 || bh > ch * 0.9;
        let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
        let off = (cw / 2.0 - cx).abs() > cw * 0.01 || (target_y - cy).abs() > ch * 0.01;
        if !too_big && !off {
            return format!("measured {bw:.0}×{bh:.0} px, centred inside the frame");
        }
        if too_big {
            let k = (cw * 0.86 / bw).min(ch * 0.86 / bh).min(1.0);
            size = (size * k).floor().max(20.0);
            let _ = exec(ctx, "graphics.set", json!({"clip": clip, "props": {"size": size}}));
            continue;
        }
        // move the text so its measured centre lands on the target
        if let Some(pos) = exec(ctx, "graphics.list", json!({"clip": clip}))
            .ok()
            .and_then(|v| v.get("layers").and_then(Value::as_array).and_then(|l| l.first()).and_then(|l| l.get("position")).cloned())
            .and_then(|p| Some((p.get(0)?.as_f64()?, p.get(1)?.as_f64()?)))
        {
            let _ = exec(ctx, "graphics.set", json!({"clip": clip, "props": {"position": [pos.0 + (cw / 2.0 - cx), pos.1 + (target_y - cy)]}}));
        }
    }
    match title_bounds(ctx, clip) {
        Some((b, (cw, ch))) if b[0] >= 0.0 && b[1] >= 0.0 && b[2] <= cw && b[3] <= ch => {
            format!("measured {:.0}×{:.0} px, inside the frame", b[2] - b[0], b[3] - b[1])
        }
        Some((b, _)) => format!("Error: still outside the frame after correcting (bounds {:.0},{:.0}–{:.0},{:.0})", b[0], b[1], b[2], b[3]),
        None => "(couldn't measure it)".into(),
    }
}

// ------------------------------------------------------------------------------------------ title

/// Break a title into at most two balanced lines and pick a size that fits 86 % of the frame
/// width (a character of a bold sans is about 0.56 of the size wide).
pub fn fit_title(text: &str, width: f64, height: f64, position: &str) -> (String, f64) {
    let text = text.trim();
    let base = if height > width { width * 0.09 } else { height * 0.1 };
    let base = if position == "lower_third" { base * 0.7 } else { base };
    let max_w = width * 0.86;
    let one_line = text.chars().count().max(1) as f64;
    if one_line * 0.56 * base <= max_w || !text.contains(' ') {
        let size = base.min(max_w / (one_line * 0.56));
        return (text.to_string(), size.max(28.0).round());
    }
    // two lines, split at the space nearest the middle
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut best = (usize::MAX, 1);
    for i in 1..words.len() {
        let a = words[..i].join(" ").chars().count();
        let b = words[i..].join(" ").chars().count();
        let longest = a.max(b);
        if longest < best.0 {
            best = (longest, i);
        }
    }
    let (longest, at) = best;
    let lines = format!("{}\n{}", words[..at].join(" "), words[at..].join(" "));
    let size = base.min(max_w / (longest.max(1) as f64 * 0.56));
    (lines, size.max(28.0).round())
}

pub fn add_title(ctx: &Ctx, args: &Value) -> Outcome {
    let tl = match timeline(ctx) {
        Ok(t) => t,
        Err(e) => return Outcome::err(e),
    };
    let position = args.get("position").and_then(Value::as_str).unwrap_or("center");
    let target_y = match position {
        "top" => tl.height * 0.16,
        "lower_third" => tl.height * 0.8,
        _ => tl.height * 0.5,
    };
    let color = args.get("color").and_then(Value::as_str).filter(|c| c.starts_with('#') && (c.len() == 7 || c.len() == 9)).unwrap_or("#ffffff");
    let weight = args.get("weight").and_then(Value::as_str).unwrap_or("Bold");
    let existing = args.get("clip").and_then(Value::as_u64);
    let text = args.get("text").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty());

    // a new title, or changes to one already on the timeline
    let (clip_id, lines, size) = match existing {
        Some(c) => {
            let (lines, size) = match text {
                Some(t) => {
                    let (l, s) = fit_title(t, tl.width, tl.height, position);
                    if let Err(e) = exec(ctx, "graphics.setText", json!({"clip": c, "text": l})) {
                        return Outcome::err(format!("couldn't change the title's text: {e}"));
                    }
                    (l, s)
                }
                None => (String::new(), fit_title("Title", tl.width, tl.height, position).1),
            };
            (c, lines, size)
        }
        None => {
            let Some(t) = text else { return Outcome::err("missing `text`") };
            let start = args.get("start_seconds").and_then(Value::as_f64).unwrap_or(0.0).max(0.0);
            let duration = args.get("duration_seconds").and_then(Value::as_f64).unwrap_or(3.0).clamp(0.5, 60.0);
            let (lines, size) = fit_title(t, tl.width, tl.height, position);
            let made = match exec(
                ctx,
                "graphics.newText",
                json!({"text": lines, "size": size, "fontStyle": weight, "seconds": duration, "time": secs_to_ticks(start), "position": [tl.width / 2.0, target_y]}),
            ) {
                Ok(v) => v,
                Err(e) => return Outcome::err(format!("couldn't add the title: {e}")),
            };
            let Some(c) = made.get("clip").and_then(Value::as_u64) else { return Outcome::err("the title was made but JBD didn't say where") };
            (c, lines, size)
        }
    };
    // style: centred lines, readable on any picture
    let mut props = json!({"align": "center", "fill_color": color, "shadow": true, "font_style": weight});
    if let Some(font) = args.get("font").and_then(Value::as_str).filter(|f| !f.is_empty()) {
        props["font"] = json!(font);
    }
    if args.get("background").and_then(Value::as_bool).unwrap_or(false) {
        props["background"] = json!(true);
    }
    if existing.is_none() || text.is_some() {
        props["size"] = json!(size);
    }
    let _ = exec(ctx, "graphics.set", json!({"clip": clip_id, "props": props}));
    // A title is placed by its text layer; the clip's own Motion stays neutral. If it was moved
    // or scaled (and isn't animated), put it back, or the text could sit off the frame however
    // well the layer is centred.
    if existing.is_some() {
        for (param, neutral) in [("position", json!([tl.width / 2.0, tl.height / 2.0])), ("scale", json!(100.0))] {
            // (an "auto" position reads as nulls: that is already the centre)
            let moved = |v: &Value| match v.as_f64() {
                Some(s) => (s - 100.0).abs() > 0.5,
                None => v.get(0).and_then(Value::as_f64).is_some() && !same_value(v, &neutral),
            };
            if let Some((v, keys)) = read_keyframes(ctx, clip_id, "motion", param)
                && keys.is_empty()
                && moved(&v)
            {
                let _ = exec(ctx, "effects.setParam", json!({"clip": clip_id, "effect": "motion", "param": param, "value": neutral}));
            }
        }
    }
    // measure with the real font and correct until it's inside the frame and centred
    let fit = settle_title(ctx, clip_id, size, target_y);
    let mut text_out = format!(
        "{} the title{} (clip {clip_id}), {position}: {fit}.",
        if existing.is_some() { "Updated" } else { "Added" },
        if lines.is_empty() { String::new() } else { format!(" \"{}\"", lines.replace('\n', " / ")) }
    );
    let mut missing = Vec::new();
    if fit.starts_with("Error") {
        missing.push(fit.trim_start_matches("Error: ").to_string());
    }
    if let Some(p) = args.get("animation").and_then(Value::as_str).filter(|p| *p != "none") {
        let a = animate(ctx, &json!({"clip": clip_id, "preset": p}));
        if a.ok {
            text_out.push(' ');
            text_out.push_str(&a.text);
        } else {
            missing.push(format!("the {p} animation ({})", a.text.trim_start_matches("Error: ")));
        }
    }
    if missing.is_empty() {
        return Outcome { text: text_out, ok: true, edited: true, credits: 0 };
    }
    Outcome::partial(text_out, &missing, 0)
}

// ------------------------------------------------------------------------------------- voiceover

pub fn voiceover(ctx: &Ctx, step: u64, args: &Value) -> Outcome {
    let Some(script) = args.get("script").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) else { return Outcome::err("missing `script`") };
    let Some(model) = ctx.catalogue.default_voice() else { return Outcome::err("no voice model is available right now.") };
    let credits = model.credits_for(script);
    lock(ctx.shared).update_step(step, |s| s.credits = credits);
    if !crate::tools::may_spend(ctx, credits)
        && !ask(ctx, step, "Record voiceover", &format!("{} · {} characters\n\"{}\"", model.label, script.chars().count(), clip(script, 300)), credits)
    {
        return Outcome { text: "The person chose not to record the voiceover.".into(), ok: false, edited: false, credits: 0 };
    }
    let voice = args.get("voice").and_then(Value::as_str).filter(|v| model.voices.iter().any(|x| x == v));
    let tone = args.get("tone").and_then(Value::as_str);
    let (path, charged) = match ctx.model.speech(&model.id, script, voice, tone, &ctx.editor.media_dir()) {
        Ok(r) => r,
        Err(e) => return Outcome::err(e),
    };
    ctx.spent.set(ctx.spent.get().saturating_add(charged));
    let imported = match exec(ctx, "file.import", json!({"paths": [path.to_string_lossy()]})) {
        Ok(v) => v,
        Err(e) => {
            return Outcome { text: format!("Error: the voiceover was recorded but importing it failed: {e}"), ok: false, edited: false, credits: charged };
        }
    };
    let Some(item) = imported.get("items").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_u64) else {
        return Outcome::partial(
            "recorded the voiceover and imported it.",
            &["it isn't on the timeline: find it in the project (inspect project) and place it with timeline.place".into()],
            charged,
        );
    };
    // its own audio track, so it never fights the clips' sound
    let at = args.get("place_at_seconds").and_then(Value::as_f64).unwrap_or(0.0).max(0.0);
    let track = match timeline(ctx) {
        Ok(tl) => {
            let _ = exec(ctx, "sequence.addTracks", json!({"video": 0, "audio": 1}));
            format!("A{}", tl.audio_tracks + 1)
        }
        Err(e) => {
            return Outcome::partial(
                format!("recorded the voiceover (media item {item})."),
                &[format!("placing it: {e}. Place it with timeline.place")],
                charged,
            );
        }
    };
    match exec(ctx, "timeline.place", json!({"item": item, "track": track, "seconds": at})) {
        Ok(v) if !placed_clips(&v).is_empty() => Outcome {
            text: format!(
                "Recorded the voiceover with {} (item {item}, clip {}) on its own track {track} at {at:.1} s. Lower the clips' own sound under it with clip.audioGain if they compete.",
                model.label,
                placed_clips(&v).first().copied().unwrap_or(0)
            ),
            ok: true,
            edited: true,
            credits: charged,
        },
        Ok(_) => Outcome::partial(
            format!("recorded the voiceover (media item {item})."),
            &[format!("it wasn't placed on {track}. Place it with timeline.place")],
            charged,
        ),
        Err(e) => Outcome::partial(
            format!("recorded the voiceover (media item {item})."),
            &[format!("placing it on {track} failed ({e}). Place it with timeline.place")],
            charged,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_title_stays_on_one_line_and_fits() {
        let (lines, size) = fit_title("Panda", 1080.0, 1920.0, "center");
        assert_eq!(lines, "Panda");
        assert!(size * 0.56 * 5.0 <= 1080.0 * 0.86);
    }

    #[test]
    fn a_long_title_wraps_to_two_balanced_lines_inside_a_vertical_frame() {
        let (lines, size) = fit_title("Nike Dunk Low Panda", 1080.0, 1920.0, "center");
        assert_eq!(lines, "Nike Dunk\nLow Panda");
        let longest = lines.lines().map(|l| l.chars().count()).max().unwrap_or(0) as f64;
        assert!(longest * 0.56 * size <= 1080.0 * 0.86 + 1.0, "{size}");
    }

    #[test]
    fn lower_thirds_are_smaller() {
        let (_, center) = fit_title("Hi", 1920.0, 1080.0, "center");
        let (_, lower) = fit_title("Hi", 1920.0, 1080.0, "lower_third");
        assert!(lower < center);
    }
}
