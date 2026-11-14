//! Understanding the person's own footage: where the silences are, where the music's beats fall,
//! where one shot cuts to the next, and what is said (the transcript), all in TIMELINE seconds so
//! the Director can turn them straight into edits (`edits.rs`).
//!
//! Used by the `read_footage` tool. The heavy part ([`analyze`], [`transcribe`]) decodes media, so
//! the app runs it on a worker thread of its own (`ui-egui/src/jbd/director_run.rs`, editor
//! methods `jbd.analyze` and `jbd.transcribe`) and JBD never freezes while it works. It only reads
//! the project: nothing here changes the edit.
//!
//! Common changes:
//! - how quiet counts as silence: [`silence_threshold`];
//! - how much breath is left at each side of a cut silence: [`PAD`];
//! - how much footage one call may decode: [`MAX_AUDIO_SECONDS`], [`MAX_SCENE_FRAMES`].

use std::collections::BTreeSet;

use filmcraft_engine::project::{ItemId, Project, TrackItem, TrackKind};
use filmcraft_engine::render::SourceProvider;
use filmcraft_engine::time::Tick;
use serde_json::{Value, json};

/// Audio is analysed at this rate (plenty for levels and speech).
const RATE: u32 = 16_000;
/// Music is analysed at this rate (the beat tracker wants the highs of the hi-hats).
const BEAT_RATE: u32 = 22_050;
/// Level is measured every 20 ms over a 40 ms window.
const HOP_S: f32 = 0.02;
const WINDOW_S: f32 = 0.04;
/// Longest stretch of one clip's audio analysed per call (30 minutes).
const MAX_AUDIO_SECONDS: f64 = 1800.0;
/// Longest stretch of music searched for beats per clip (8 minutes).
const MAX_BEAT_SECONDS: f64 = 480.0;
/// Most video frames decoded for shot detection per call (about 3 minutes at 30 fps).
const MAX_SCENE_FRAMES: usize = 6000;
/// Breath kept at each side of a silence, so a cut never clips the end of a word.
pub const PAD: f64 = 0.12;
/// Shortest silence reported (after padding) unless the request asks for another.
const DEFAULT_MIN_SILENCE: f64 = 0.5;
/// Audio is decoded in pieces of this many seconds (a long clip never needs one huge buffer).
const CHUNK_S: f64 = 60.0;

/// One clip on the timeline, as the analysis needs it.
#[derive(Clone, Debug)]
struct Clip {
    id: u64,
    item: ItemId,
    name: String,
    /// Timeline seconds.
    start: f64,
    end: f64,
    /// Media seconds at the clip's first frame.
    source_in: f64,
    speed: f64,
    video: bool,
}

impl Clip {
    fn of(it: &TrackItem, video: bool) -> Self {
        Clip {
            id: it.id.0,
            item: it.item,
            name: it.name.clone(),
            start: it.start.seconds(),
            end: it.end().seconds(),
            source_in: it.source_in.seconds(),
            speed: if it.speed.is_finite() && it.speed > 0.0 { it.speed } else { 1.0 },
            video,
        }
    }
    /// Media seconds of the clip's last frame.
    fn source_out(&self) -> f64 {
        self.source_in + (self.end - self.start) * self.speed
    }
    /// Media time → timeline time.
    fn to_timeline(&self, media: f64) -> f64 {
        self.start + (media - self.source_in) / self.speed
    }
}

/// The timeline clips to analyse: enabled, forward-playing, not frozen, on tracks that play.
/// `only`: the clip ids asked for (empty = all).
fn clips(project: &Project, seq: ItemId, only: &[u64]) -> Vec<Clip> {
    let Some(q) = project.sequence(seq) else { return Vec::new() };
    let mut out = Vec::new();
    for t in q.all_tracks() {
        if !t.enabled || t.muted {
            continue;
        }
        let video = t.kind == TrackKind::Video;
        for it in &t.items {
            if !it.enabled || it.reverse || it.frame_hold.is_some() || it.duration.0 <= 0 {
                continue;
            }
            if !only.is_empty() && !only.contains(&it.id.0) {
                continue;
            }
            out.push(Clip::of(it, video));
        }
    }
    out
}

/// Mono audio of `media_a..media_b` (media seconds) at `rate`, decoded piece by piece.
fn mono(provider: &dyn SourceProvider, item: ItemId, media_a: f64, media_b: f64, rate: u32, cancel: &dyn Fn() -> bool) -> Option<Vec<f32>> {
    let src = provider.source(item)?;
    if !src.info().has_audio() || media_b.partial_cmp(&media_a) != Some(std::cmp::Ordering::Greater) {
        return None;
    }
    let mut out = Vec::new();
    let mut a = media_a.max(0.0);
    while a < media_b {
        if cancel() {
            return None;
        }
        let b = (a + CHUNK_S).min(media_b);
        let start = (a * f64::from(rate)).round() as i64;
        let frames = ((b - a) * f64::from(rate)).round().max(0.0) as usize;
        if frames == 0 {
            break;
        }
        let buf = src.audio(start, frames, rate).ok()?;
        let n = buf.channels.iter().map(Vec::len).min().unwrap_or(0);
        let k = 1.0 / buf.channels.len().max(1) as f32;
        out.extend((0..n).map(|i| buf.channels.iter().map(|c| c.get(i).copied().unwrap_or(0.0)).sum::<f32>() * k));
        a = b;
    }
    Some(out)
}

/// The level a sound has to be above to count as sound, from the clip's own levels: well under
/// its loud parts (speech), and clearly over its noise floor. None when the clip has no clear
/// quiet parts (a music bed, steady noise), so it never reads as silence.
pub fn silence_threshold(env_db: &[f32]) -> Option<f32> {
    let mut v: Vec<f32> = env_db.iter().copied().filter(|x| x.is_finite()).collect();
    if v.len() < 10 {
        return None;
    }
    v.sort_by(f32::total_cmp);
    let at = |p: f32| v.get(((v.len() - 1) as f32 * p) as usize).copied().unwrap_or(-120.0);
    let (floor, loud) = (at(0.1), at(0.9));
    if loud < -60.0 || loud - floor < 12.0 {
        return None;
    }
    Some((floor + 8.0).max(loud - 24.0).clamp(-65.0, -25.0))
}

/// Silent stretches of `samples` (seconds from its start), from the level envelope: everything
/// that isn't sound, after short blips are ignored and short gaps inside sound are bridged.
pub fn quiet_spans(samples: &[f32], rate: u32) -> Vec<(f64, f64)> {
    use filmcraft_audio_dsp::ducking::{activity, envelope_db};
    let env = envelope_db(&[samples], rate as f32, HOP_S, WINDOW_S);
    let Some(thr) = silence_threshold(&env) else { return Vec::new() };
    // sound: at least 60 ms, gaps under 150 ms inside it don't count
    let loud = activity(&env, HOP_S, thr, 0.06, 0.15);
    let len = samples.len() as f64 / f64::from(rate.max(1));
    complement(&loud, 0.0, len)
}

/// `[a, b]` minus `spans` (sorted, non-overlapping).
fn complement(spans: &[(f64, f64)], a: f64, b: f64) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let mut at = a;
    for &(s, e) in spans {
        if s > at {
            out.push((at, s.min(b)));
        }
        at = at.max(e);
    }
    if at < b {
        out.push((at, b));
    }
    out.retain(|(s, e)| e > s);
    out
}

/// Sort and merge overlapping spans.
fn union(mut spans: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    spans.retain(|(a, b)| a.is_finite() && b.is_finite() && b > a);
    spans.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut out: Vec<(f64, f64)> = Vec::new();
    for (a, b) in spans {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// Timeline silences: where the timeline has sound clips but none of them is making a sound.
/// Each span is already padded by [`PAD`] on both sides and at least `min` long, so it can be
/// cut as it is.
fn timeline_silences(audio: &[(Clip, Vec<(f64, f64)>)], min: f64) -> Vec<(f64, f64)> {
    // where there is audio at all, and where any of it is loud
    let covered = union(audio.iter().map(|(c, _)| (c.start, c.end)).collect());
    let mut loud = Vec::new();
    for (c, quiet) in audio {
        let quiet_tl: Vec<(f64, f64)> =
            quiet.iter().map(|(a, b)| (c.to_timeline(c.source_in + a * c.speed), c.to_timeline(c.source_in + b * c.speed))).collect();
        loud.extend(complement(&quiet_tl, c.start, c.end));
    }
    let loud = union(loud);
    let mut out = Vec::new();
    for (a, b) in covered {
        for (s, e) in complement(&loud, a, b) {
            // the very start and end of the edit need no breath on the outside
            let s2 = if s <= a + 1e-6 { s } else { s + PAD };
            let e2 = if e >= b - 1e-6 { e } else { e - PAD };
            if e2 - s2 >= min {
                out.push((s2, e2));
            }
        }
    }
    out
}

fn r2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Analyse the open timeline. `params`: `what` (any of "silences", "beats", "scenes"), `clips`
/// (ids to limit it to), `minSilenceSeconds`. Reads only; never fails on one bad clip (it says
/// so in `notes`). Called on a worker thread.
pub fn analyze(project: &Project, seq: ItemId, provider: &dyn SourceProvider, params: &Value, cancel: &dyn Fn() -> bool) -> Result<Value, String> {
    let q = project.sequence(seq).ok_or("no timeline is open")?;
    let what: BTreeSet<String> =
        params.get("what").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    let only: Vec<u64> = params.get("clips").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
    let min = params.get("minSilenceSeconds").and_then(Value::as_f64).filter(|m| m.is_finite()).unwrap_or(DEFAULT_MIN_SILENCE).clamp(0.1, 30.0);
    let all = clips(project, seq, &only);
    let mut out = json!({"duration": r2(q.duration().seconds())});
    let mut notes: Vec<String> = Vec::new();

    if what.contains("silences") {
        let mut audio = Vec::new();
        for c in all.iter().filter(|c| !c.video) {
            let (a, b) = (c.source_in, c.source_out().min(c.source_in + MAX_AUDIO_SECONDS));
            if c.source_out() > b {
                notes.push(format!("clip {}: only its first {} minutes were checked for silences", c.id, (MAX_AUDIO_SECONDS / 60.0) as u32));
            }
            match mono(provider, c.item, a, b, RATE, cancel) {
                Some(samples) => audio.push((c.clone(), quiet_spans(&samples, RATE))),
                None if cancel() => return Err("stopped".into()),
                None => notes.push(format!("clip {} (\"{}\"): its sound couldn't be read", c.id, c.name)),
            }
        }
        if audio.is_empty() {
            notes.push("no sound clips on the timeline to find silences in".into());
        }
        let s = timeline_silences(&audio, min);
        let total: f64 = s.iter().map(|(a, b)| b - a).sum();
        out["silences"] = json!(s.iter().map(|(a, b)| json!([r2(*a), r2(*b)])).collect::<Vec<_>>());
        out["silenceSeconds"] = json!(r2(total));
    }

    if what.contains("beats") {
        let mut found = Vec::new();
        for c in all.iter().filter(|c| !c.video) {
            let (a, b) = (c.source_in, c.source_out().min(c.source_in + MAX_BEAT_SECONDS));
            let Some(samples) = mono(provider, c.item, a, b, BEAT_RATE, cancel) else {
                if cancel() {
                    return Err("stopped".into());
                }
                continue;
            };
            match filmcraft_audio_dsp::remix::analyze(&[&samples], BEAT_RATE) {
                Ok(an) => {
                    let times: Vec<f64> = an
                        .beats
                        .iter()
                        .map(|s| r2(c.to_timeline(a + *s as f64 / f64::from(BEAT_RATE))))
                        .filter(|t| *t >= c.start && *t < c.end)
                        .take(600)
                        .collect();
                    found.push(json!({"clip": c.id, "name": c.name, "bpm": (an.bpm * 10.0).round() / 10.0, "beats": times}));
                }
                Err(_) => notes.push(format!("clip {} (\"{}\"): no steady beat found (speech or ambience, or too short)", c.id, c.name)),
            }
        }
        out["beats"] = json!(found);
    }

    if what.contains("scenes") {
        let mut found = Vec::new();
        let mut budget = MAX_SCENE_FRAMES;
        for c in all.iter().filter(|c| c.video) {
            let Some(src) = provider.source(c.item) else { continue };
            let Some(v) = src.info().video.clone() else { continue };
            let fps = v.frame_rate.as_f64();
            if !fps.is_finite() || fps <= 0.0 {
                continue;
            }
            let frames = (((c.source_out() - c.source_in) * fps).ceil().max(1.0)) as usize;
            if budget == 0 {
                notes.push(format!("clip {}: not checked for shot changes (too much footage in one call; ask for fewer clips)", c.id));
                continue;
            }
            // long clips are sampled every few frames: cuts still stand out
            let stride = frames.div_ceil(budget.max(1)).max(1);
            let scale = (160.0 / v.width.max(1) as f32).min(1.0);
            let mut det = filmcraft_engine::render::scene::SceneDetector::new();
            let mut times = Vec::new();
            let mut k = 0;
            while k < frames {
                if cancel() {
                    return Err("stopped".into());
                }
                let mt = c.source_in + k as f64 / fps;
                if let Ok(f) = src.video_frame(filmcraft_media::FrameRequest { time: Tick::from_seconds_f64(mt), scale }) {
                    det.push_rgba(&f.to_rgba8(), f.width as usize, f.height as usize);
                    times.push(mt);
                }
                k += stride;
            }
            budget = budget.saturating_sub(times.len());
            let opts = filmcraft_engine::render::scene::SceneOptions { sensitivity: 50.0, min_shot_frames: (6 / stride).max(1) };
            let cuts: Vec<f64> = det.cuts(&opts).into_iter().filter_map(|i| times.get(i)).map(|m| r2(c.to_timeline(*m))).collect();
            found.push(json!({"clip": c.id, "name": c.name, "cuts": cuts}));
        }
        out["scenes"] = json!(found);
    }
    if !notes.is_empty() {
        out["notes"] = json!(notes);
    }
    Ok(out)
}

/// Transcribe media items that have sound but no transcript yet, on the caller's (worker) thread.
/// Returns `(item, transcript JSON)` pairs, ready for the engine's `transcript.set`.
pub fn transcribe(
    project: &Project,
    provider: &dyn SourceProvider,
    items: &[u64],
    transcriber: &dyn filmcraft_speech::Transcriber,
    language: Option<String>,
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<(u64, Value)>, String> {
    let mut out = Vec::new();
    for &id in items {
        let item = ItemId(id);
        let Some(dur) = project.item(item).map(|i| i.duration().seconds()) else { continue };
        let Some(audio) = mono(provider, item, 0.0, dur.min(MAX_AUDIO_SECONDS), filmcraft_speech::SAMPLE_RATE, cancel) else {
            if cancel() {
                return Err("stopped".into());
            }
            continue;
        };
        let opts = filmcraft_speech::Options { language: language.clone(), diarize: false, max_speakers: 1 };
        let mut t = transcriber.transcribe(&audio, &opts, &mut |_, _| !cancel()).map_err(|e| e.to_string())?;
        t.normalize();
        out.push((id, serde_json::to_value(&t).map_err(|e| e.to_string())?));
    }
    Ok(out)
}

/// The speech model to transcribe with: the one the editor already has (tests install one), else
/// the model chosen in Settings ▸ Media Analysis & Transcription, loaded from disk.
pub fn transcriber(
    installed: Option<std::sync::Arc<dyn filmcraft_speech::Transcriber>>,
    model: &str,
) -> Result<std::sync::Arc<dyn filmcraft_speech::Transcriber>, String> {
    if let Some(t) = installed {
        return Ok(t);
    }
    let dir = filmcraft_engine::transcript::models_dir().ok_or("there is no folder for speech models")?;
    filmcraft_speech::load(&dir, model).map_err(|e| format!("{e}. Download the speech model in Settings ▸ Media Analysis & Transcription"))
}

/// Longest transcript sent to the model in one answer (characters); ask for a part for more.
const MAX_TRANSCRIPT: usize = 9_000;

/// The `read_footage` tool: the transcript (made first where missing), silences, beats and shot
/// changes, as plain text in timeline seconds.
pub fn read_footage(ctx: &crate::tools::Ctx, args: &Value) -> crate::tools::Outcome {
    use crate::tools::Outcome;
    let what: Vec<String> =
        args.get("what").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    if what.is_empty() {
        return Outcome::err("say what to read: any of transcript, silences, beats, scenes");
    }
    let clips: Vec<u64> = args.get("clips").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
    let exec = |command: &str, params: Value| ctx.editor.call("engine.execute", json!({"command": command, "params": params}));
    let mut parts: Vec<String> = Vec::new();

    let measured: Vec<&str> = what.iter().map(String::as_str).filter(|w| matches!(*w, "silences" | "beats" | "scenes")).collect();
    if !measured.is_empty() {
        let params = json!({"what": measured, "clips": clips, "minSilenceSeconds": args.get("min_silence_seconds").and_then(Value::as_f64).unwrap_or(DEFAULT_MIN_SILENCE)});
        match ctx.editor.call("jbd.analyze", params) {
            Ok(v) => parts.push(analysis_text(&v)),
            Err(e) => parts.push(format!("Couldn't analyse the footage: {e}")),
        }
    }

    if what.iter().any(|w| w == "transcript") {
        let made = match transcribe_missing(ctx, &clips) {
            Ok(n) => n,
            Err(e) => {
                parts.push(format!("TRANSCRIPT: the clips couldn't be transcribed ({e}). Edit by silences and by looking instead, or ask the person to transcribe them in the Text panel."));
                0
            }
        };
        match exec("transcript.inspect", json!({})) {
            Ok(v) => {
                let from = args.get("from_seconds").and_then(Value::as_f64).unwrap_or(f64::NEG_INFINITY);
                let to = args.get("to_seconds").and_then(Value::as_f64).unwrap_or(f64::INFINITY);
                const T: f64 = 254_016_000_000.0;
                let words: Vec<Value> = v
                    .get("words")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|w| {
                        let s = w.get("start").and_then(Value::as_f64).unwrap_or(0.0) / T;
                        s >= from && s <= to && (clips.is_empty() || w.get("clip").and_then(Value::as_u64).is_some_and(|c| clips.contains(&c)))
                    })
                    .cloned()
                    .collect();
                if words.is_empty() {
                    parts.push("TRANSCRIPT: nothing is said on the timeline (or there is no speech in those clips).".into());
                } else {
                    let note = if made > 0 { format!(" ({made} clip{} transcribed just now)", if made == 1 { "" } else { "s" }) } else { String::new() };
                    parts.push(format!("TRANSCRIPT{note}, timeline seconds:\n{}", transcript_lines(&words, MAX_TRANSCRIPT)));
                }
            }
            Err(e) => parts.push(format!("TRANSCRIPT: couldn't read it ({e})")),
        }
    }
    Outcome::ok(parts.join("\n\n"))
}

/// Transcribe the sound clips on the timeline whose media has no transcript yet (on a worker
/// thread in the app), and store each with the editor's `transcript.set`. Returns how many.
fn transcribe_missing(ctx: &crate::tools::Ctx, only: &[u64]) -> Result<usize, String> {
    let exec = |command: &str, params: Value| ctx.editor.call("engine.execute", json!({"command": command, "params": params}));
    let have: BTreeSet<u64> = exec("transcript.inspect", json!({}))?
        .get("items")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|i| i.get("item").and_then(Value::as_u64)).collect())
        .unwrap_or_default();
    let seq = exec("sequence.inspect", json!({}))?;
    let mut missing: Vec<u64> = Vec::new();
    for c in seq.get("audio").and_then(Value::as_array).into_iter().flatten().flat_map(|t| t.get("items").and_then(Value::as_array).into_iter().flatten()) {
        let (Some(id), Some(item)) = (c.get("clip").and_then(Value::as_u64), c.get("item").and_then(Value::as_u64)) else { continue };
        let linked_asked = only.is_empty() || only.contains(&id) || {
            // a video clip named: its linked sound counts
            let link = c.get("link").and_then(Value::as_u64);
            link.is_some()
                && seq
                    .get("video")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .flat_map(|t| t.get("items").and_then(Value::as_array).into_iter().flatten())
                    .any(|v| v.get("link").and_then(Value::as_u64) == link && v.get("clip").and_then(Value::as_u64).is_some_and(|vid| only.contains(&vid)))
        };
        if linked_asked && !have.contains(&item) && !missing.contains(&item) {
            missing.push(item);
        }
    }
    if missing.is_empty() {
        return Ok(0);
    }
    crate::lock(ctx.shared).activity = Some(format!("Transcribing {} clip{}…", missing.len(), if missing.len() == 1 { "" } else { "s" }));
    (ctx.repaint)();
    let got = ctx.editor.call("jbd.transcribe", json!({"items": missing}))?;
    let mut n = 0;
    for t in got.get("transcripts").and_then(Value::as_array).into_iter().flatten() {
        let (Some(item), Some(tr)) = (t.get("item").and_then(Value::as_u64), t.get("transcript")) else { continue };
        if exec("transcript.set", json!({"item": item, "transcript": tr})).is_ok() {
            n += 1;
        }
    }
    Ok(n)
}

/// The `jbd.analyze` answer as text for the model.
fn analysis_text(v: &Value) -> String {
    let mut out = Vec::new();
    let pairs = |a: &Value| -> String {
        a.as_array()
            .map(|x| x.iter().filter_map(|p| Some(format!("[{}, {}]", p.get(0)?.as_f64()?, p.get(1)?.as_f64()?))).collect::<Vec<_>>().join(", "))
            .unwrap_or_default()
    };
    if let Some(s) = v.get("silences").and_then(Value::as_array) {
        if s.is_empty() {
            out.push("SILENCES: none (the sound never drops to a pause, or it's a steady music bed).".to_string());
        } else {
            out.push(format!(
                "SILENCES ({} pauses, {} s in all; already padded {PAD} s each side, ready to cut):\n{}",
                s.len(),
                v.get("silenceSeconds").and_then(Value::as_f64).unwrap_or(0.0),
                pairs(&Value::Array(s.clone()))
            ));
        }
    }
    for b in v.get("beats").and_then(Value::as_array).into_iter().flatten() {
        let times: Vec<String> = b.get("beats").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_f64).map(|t| format!("{t}")).collect();
        out.push(format!(
            "BEATS of clip {} \"{}\", {} bpm: {}",
            b.get("clip").cloned().unwrap_or(Value::Null),
            b.get("name").and_then(Value::as_str).unwrap_or(""),
            b.get("bpm").cloned().unwrap_or(Value::Null),
            times.join(", ")
        ));
    }
    if v.get("beats").and_then(Value::as_array).is_some_and(Vec::is_empty) {
        out.push("BEATS: no music with a steady beat on the timeline.".into());
    }
    for s in v.get("scenes").and_then(Value::as_array).into_iter().flatten() {
        let cuts: Vec<String> = s.get("cuts").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_f64).map(|t| format!("{t}")).collect();
        out.push(format!(
            "SHOT CHANGES in clip {} \"{}\": {}",
            s.get("clip").cloned().unwrap_or(Value::Null),
            s.get("name").and_then(Value::as_str).unwrap_or(""),
            if cuts.is_empty() { "none (one continuous shot)".into() } else { cuts.join(", ") }
        ));
    }
    for n in v.get("notes").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        out.push(format!("Note: {n}"));
    }
    format!("Timeline {} s.\n{}", v.get("duration").cloned().unwrap_or(Value::Null), out.join("\n"))
}

/// What was said, as the model reads it: one line per phrase with its timeline time,
/// `[12.30–15.80] so today we're going to`. A phrase ends at a pause of half a second, at the end
/// of a sentence, or after 16 words. `words` is `transcript.inspect`'s `words`. Capped at
/// `max_chars` (the rest is summarised by count).
pub fn transcript_lines(words: &[Value], max_chars: usize) -> String {
    const TICKS: f64 = 254_016_000_000.0;
    let w: Vec<(String, f64, f64)> =
        words.iter().filter_map(|x| Some((x.get("text")?.as_str()?.to_string(), x.get("start")?.as_f64()? / TICKS, x.get("end")?.as_f64()? / TICKS))).collect();
    let mut lines = Vec::new();
    let mut cur: Vec<&(String, f64, f64)> = Vec::new();
    let flush = |cur: &mut Vec<&(String, f64, f64)>, lines: &mut Vec<String>| {
        if let (Some(a), Some(b)) = (cur.first(), cur.last()) {
            let text: Vec<&str> = cur.iter().map(|x| x.0.as_str()).collect();
            lines.push(format!("[{:.2}–{:.2}] {}", a.1, b.2, text.join(" ")));
        }
        cur.clear();
    };
    for x in &w {
        if let Some(prev) = cur.last()
            && (x.1 - prev.2 >= 0.5 || cur.len() >= 16 || prev.0.ends_with(['.', '?', '!', '।']))
        {
            flush(&mut cur, &mut lines);
        }
        cur.push(x);
    }
    flush(&mut cur, &mut lines);
    let mut text = String::new();
    for (i, l) in lines.iter().enumerate() {
        if text.len() + l.len() > max_chars {
            text.push_str(&format!("… and {} more lines (ask for a part of the timeline with from/to)\n", lines.len() - i));
            break;
        }
        text.push_str(l);
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tone, silence, tone… at 16 kHz: `pattern` is (seconds, loud?).
    fn speechish(pattern: &[(f64, bool)]) -> Vec<f32> {
        let mut out = Vec::new();
        let mut n = 0usize;
        for &(secs, loud) in pattern {
            for _ in 0..(secs * f64::from(RATE)) as usize {
                let t = n as f32 / RATE as f32;
                // a little room noise everywhere, a voice-like tone where it's loud
                let noise = ((n.wrapping_mul(2_654_435_761) >> 7) % 1000) as f32 / 1000.0 - 0.5;
                out.push(if loud { 0.3 * (t * 220.0 * std::f32::consts::TAU).sin() } else { 0.0 } + 0.002 * noise);
                n += 1;
            }
        }
        out
    }

    #[test]
    fn finds_the_pauses_between_phrases() {
        let s = speechish(&[(2.0, true), (1.5, false), (3.0, true), (0.1, false), (1.0, true), (2.0, false)]);
        let q = quiet_spans(&s, RATE);
        // the 1.5 s pause and the 2 s tail; the 0.1 s breath is bridged, not a silence
        assert_eq!(q.len(), 2, "{q:?}");
        assert!((q[0].0 - 2.0).abs() < 0.1 && (q[0].1 - 3.5).abs() < 0.1, "{q:?}");
        assert!((q[1].0 - 7.6).abs() < 0.1 && (q[1].1 - 9.6).abs() < 0.1, "{q:?}");
    }

    #[test]
    fn steady_sound_has_no_silences() {
        let s = speechish(&[(5.0, true)]);
        assert!(quiet_spans(&s, RATE).is_empty());
        let hum = vec![0.001f32; 16_000 * 3];
        assert!(quiet_spans(&hum, RATE).is_empty(), "a quiet but steady room is not a pause");
    }

    #[test]
    fn timeline_silences_need_every_track_quiet_and_keep_a_breath() {
        let c = |id, start: f64, end: f64| Clip { id, item: ItemId(id), name: String::new(), start, end, source_in: 0.0, speed: 1.0, video: false };
        // A1 talks 0-2 and 4-6; A2 is quiet throughout 0-10 → only 2-4 and 6-10 are silent
        let a1 = (c(1, 0.0, 10.0), vec![(2.0, 4.0), (6.0, 10.0)]);
        let s = timeline_silences(std::slice::from_ref(&a1), 0.5);
        assert_eq!(s.len(), 2);
        assert!((s[0].0 - (2.0 + PAD)).abs() < 1e-9 && (s[0].1 - (4.0 - PAD)).abs() < 1e-9);
        // the tail runs to the end of the sound: no breath needed after it
        assert!((s[1].1 - 10.0).abs() < 1e-9);
        // a second track talking 3-7 fills half the first pause and part of the tail
        let a2 = (c(2, 0.0, 10.0), vec![(0.0, 3.0), (7.0, 10.0)]);
        let s = timeline_silences(&[a1, a2], 0.5);
        assert_eq!(s.len(), 2, "{s:?}");
        assert!((s[0].0 - (2.0 + PAD)).abs() < 1e-9 && (s[0].1 - (3.0 - PAD)).abs() < 1e-9, "{s:?}");
        assert!((s[1].0 - (7.0 + PAD)).abs() < 1e-9, "{s:?}");
        // shorter than the minimum once padded: not reported
        assert!(timeline_silences(std::slice::from_ref(&(c(3, 0.0, 10.0), vec![(4.0, 4.6)])), 0.5).is_empty());
    }

    #[test]
    fn clips_map_media_time_to_the_timeline_with_speed() {
        let c = Clip { id: 1, item: ItemId(1), name: String::new(), start: 10.0, end: 15.0, source_in: 4.0, speed: 2.0, video: false };
        assert!((c.to_timeline(4.0) - 10.0).abs() < 1e-9);
        assert!((c.to_timeline(8.0) - 12.0).abs() < 1e-9);
        assert!((c.source_out() - 14.0).abs() < 1e-9);
    }

    #[test]
    fn transcript_reads_as_timed_phrases() {
        let t = |s: f64| (s * 254_016_000_000.0) as i64;
        let words = vec![
            json!({"text": "Hello", "start": t(0.0), "end": t(0.4)}),
            json!({"text": "everyone.", "start": t(0.45), "end": t(1.0)}),
            json!({"text": "Today", "start": t(1.1), "end": t(1.4)}),
            json!({"text": "we", "start": t(2.5), "end": t(2.6)}),
        ];
        let text = transcript_lines(&words, 10_000);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines, vec!["[0.00–1.00] Hello everyone.", "[1.10–1.40] Today", "[2.50–2.60] we"]);
        assert!(transcript_lines(&words, 30).contains("more lines"));
    }
}
