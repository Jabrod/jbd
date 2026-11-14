//! Editing the person's own footage: the `edit_timeline` tool.
//!
//! The model writes a plan in plain timeline seconds ("cut these ranges", "trim 1.5 s off the
//! start of clip 12", "put this music under it at -18 dB"); this file shows it to the person as a
//! before/after strip ([`crate::EditPreview`]) and then carries it out with the editor's own
//! commands (Extract, Razor, Trim, Ripple Delete, Move, transitions, Audio Gain, captions…). So
//! every change lands on the timeline like a hand edit and the whole request undoes in one step.
//!
//! Times in an operation are the timeline as it is when that operation runs. Inside one `cut` or
//! `keep`, every range refers to the same timeline (they are applied from the last to the first,
//! so earlier ones don't move). Ids are clip ids from PROJECT NOW or the last result.
//!
//! To add an operation: add it to [`Op`], [`parse`], [`describe`] (the preview) and [`apply`],
//! and to the tool's description in `tools.rs`.

use serde_json::{Value, json};

use crate::production::{exec, find_clip};
use crate::tools::{Ctx, Outcome, clip as short};
use crate::{EditPreview, Mode};

const TICKS: f64 = 254_016_000_000.0;
/// Most operations in one plan, and most ranges in one cut (a long talk can have a few hundred
/// pauses; past this the plan is surely wrong).
const MAX_OPS: usize = 40;
const MAX_RANGES: usize = 600;

/// One step of the plan.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// Remove these timeline ranges from every track and close the gaps.
    Cut(Vec<(f64, f64)>),
    /// Keep only these ranges (a highlight); everything else goes, gaps closed.
    Keep(Vec<(f64, f64)>),
    /// Cut every track at these times (nothing is removed).
    Split(Vec<f64>),
    /// Shorten a clip from its start and/or end, closing the gap.
    Trim { clip: u64, head: f64, tail: f64 },
    /// Remove clips (with their linked sound); `ripple` closes the gap.
    Delete { clips: Vec<u64>, ripple: bool },
    /// Put these clips back to back in this order, where the first of them starts now.
    Order(Vec<u64>),
    /// A transition at the cut before each of these clips (None: every cut on V1).
    Transition { clips: Option<Vec<u64>>, effect: String, frames: i64, audio: bool },
    /// On-screen text through `add_title` (its own arguments).
    Title(Value),
    /// Captions from the transcript.
    Captions { max_chars: u64, lines: u64 },
    /// Set the volume of clips (or of every clip on a track).
    Volume { clips: Vec<u64>, track: Option<String>, db: f64 },
    /// A music bed under the edit on its own track.
    Music { item: u64, db: f64, duck: bool, fade: f64, start: f64 },
    /// Speed a clip up or slow it down, closing or opening the gap.
    Speed { clip: u64, percent: f64 },
}

impl Op {
    /// Does this change the person's footage itself (what plays, in what order)? These are the
    /// ones the person reviews before they happen, even in Auto edits.
    fn reshapes(&self) -> bool {
        matches!(self, Op::Cut(_) | Op::Keep(_) | Op::Trim { .. } | Op::Delete { .. } | Op::Order(_) | Op::Speed { .. })
    }
}

fn f(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64).filter(|x| x.is_finite())
}

fn ids(v: &Value, k: &str) -> Vec<u64> {
    match v.get(k) {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_u64).collect(),
        Some(x) => x.as_u64().into_iter().collect(),
        None => Vec::new(),
    }
}

/// `[[a, b], …]` or `[{start, end}, …]`.
fn ranges(v: &Value) -> Vec<(f64, f64)> {
    v.get("ranges")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_RANGES)
        .filter_map(|r| match r {
            Value::Array(p) => Some((p.first()?.as_f64()?, p.get(1)?.as_f64()?)),
            Value::Object(_) => Some((f(r, "start").or_else(|| f(r, "start_seconds"))?, f(r, "end").or_else(|| f(r, "end_seconds"))?)),
            _ => None,
        })
        .filter(|(a, b)| a.is_finite() && b.is_finite() && b > a)
        .collect()
}

/// Read the plan. An operation that can't be understood fails the whole plan (with what to fix),
/// so a half-understood plan never runs.
pub fn parse(args: &Value) -> Result<Vec<Op>, String> {
    let list = args.get("operations").and_then(Value::as_array).ok_or("missing `operations`: a list like [{\"op\": \"cut\", \"ranges\": [[3.2, 4.1]]}]")?;
    if list.is_empty() {
        return Err("`operations` is empty".into());
    }
    if list.len() > MAX_OPS {
        return Err(format!("at most {MAX_OPS} operations in one plan; put many ranges in one cut instead"));
    }
    let mut ops = Vec::new();
    for (i, o) in list.iter().enumerate() {
        let n = i + 1;
        let kind = o.get("op").and_then(Value::as_str).unwrap_or("");
        let op = match kind {
            "cut" | "keep" => {
                let r = ranges(o);
                if r.is_empty() {
                    return Err(format!("operation {n} ({kind}) needs `ranges`: [[start, end], …] in timeline seconds"));
                }
                if kind == "cut" { Op::Cut(r) } else { Op::Keep(r) }
            }
            "split" => {
                let at: Vec<f64> = match o.get("at") {
                    Some(Value::Array(a)) => a.iter().filter_map(Value::as_f64).collect(),
                    Some(x) => x.as_f64().into_iter().collect(),
                    None => Vec::new(),
                };
                if at.is_empty() {
                    return Err(format!("operation {n} (split) needs `at`: [seconds, …]"));
                }
                Op::Split(at)
            }
            "trim" => {
                let clip = o.get("clip").and_then(Value::as_u64).ok_or(format!("operation {n} (trim) needs `clip`"))?;
                let head = f(o, "head_seconds").unwrap_or(0.0).max(0.0);
                let tail = f(o, "tail_seconds").unwrap_or(0.0).max(0.0);
                if head <= 0.0 && tail <= 0.0 {
                    return Err(format!("operation {n} (trim) needs `head_seconds` and/or `tail_seconds` (how much to take off)"));
                }
                Op::Trim { clip, head, tail }
            }
            "delete" => {
                let clips = ids(o, "clips");
                if clips.is_empty() {
                    return Err(format!("operation {n} (delete) needs `clips`"));
                }
                Op::Delete { clips, ripple: o.get("ripple").and_then(Value::as_bool).unwrap_or(true) }
            }
            "order" | "reorder" => {
                let clips = ids(o, "clips");
                if clips.len() < 2 {
                    return Err(format!("operation {n} (order) needs `clips`: at least two clip ids in the new order"));
                }
                Op::Order(clips)
            }
            "transition" => {
                let clips = match o.get("clips") {
                    Some(Value::String(s)) if s == "all" => None,
                    None => None,
                    Some(_) => Some(ids(o, "clips")),
                };
                Op::Transition {
                    clips,
                    effect: o.get("effect").and_then(Value::as_str).unwrap_or("cross_dissolve").to_string(),
                    frames: o.get("frames").and_then(Value::as_i64).unwrap_or(12).clamp(2, 120),
                    audio: o.get("audio").and_then(Value::as_bool).unwrap_or(true),
                }
            }
            "title" => {
                if o.get("text").and_then(Value::as_str).is_none_or(|t| t.trim().is_empty()) && o.get("clip").is_none() {
                    return Err(format!("operation {n} (title) needs `text`"));
                }
                Op::Title(o.clone())
            }
            "captions" => Op::Captions {
                max_chars: o.get("max_chars").and_then(Value::as_u64).unwrap_or(32).clamp(8, 80),
                lines: o.get("lines").and_then(Value::as_u64).unwrap_or(1).clamp(1, 2),
            },
            "volume" => {
                let clips = ids(o, "clips");
                let track = o.get("track").and_then(Value::as_str).map(str::to_string);
                let db = f(o, "db").ok_or(format!("operation {n} (volume) needs `db` (0 = as recorded, -12 = quieter)"))?;
                if clips.is_empty() && track.is_none() {
                    return Err(format!("operation {n} (volume) needs `clips` or `track`"));
                }
                Op::Volume { clips, track, db: db.clamp(-96.0, 24.0) }
            }
            "music" => Op::Music {
                item: o.get("item").and_then(Value::as_u64).ok_or(format!("operation {n} (music) needs `item`: the music's media item id"))?,
                db: f(o, "db").unwrap_or(-18.0).clamp(-60.0, 6.0),
                duck: o.get("duck").and_then(Value::as_bool).unwrap_or(true),
                fade: f(o, "fade_seconds").unwrap_or(1.5).clamp(0.0, 10.0),
                start: f(o, "start_seconds").unwrap_or(0.0).max(0.0),
            },
            "speed" => Op::Speed {
                clip: o.get("clip").and_then(Value::as_u64).ok_or(format!("operation {n} (speed) needs `clip`"))?,
                percent: f(o, "percent").ok_or(format!("operation {n} (speed) needs `percent` (100 = normal, 200 = twice as fast)"))?.clamp(10.0, 1000.0),
            },
            other => {
                return Err(format!(
                    "operation {n}: unknown op `{other}`. Use cut, keep, split, trim, delete, order, transition, title, captions, volume, music or speed"
                ));
            }
        };
        ops.push(op);
    }
    Ok(ops)
}

/// A clip on the timeline, as the plan needs it.
#[derive(Clone, Debug)]
struct Clip {
    id: u64,
    name: String,
    start: f64,
    end: f64,
    /// "V1", "A2".
    track: String,
    video: bool,
    link: Option<u64>,
}

/// The open timeline: its length, frame rate and clips.
struct Seq {
    duration: f64,
    fps: f64,
    clips: Vec<Clip>,
    audio_tracks: usize,
}

fn read(ctx: &Ctx) -> Result<Seq, String> {
    let s = exec(ctx, "sequence.inspect", json!({}))?;
    if s.get("id").is_none() {
        return Err("no timeline is open".into());
    }
    let rate = s.get("settings").and_then(|x| x.get("frame_rate"));
    let fps = match (rate.and_then(|r| r.get("num")).and_then(Value::as_f64), rate.and_then(|r| r.get("den")).and_then(Value::as_f64)) {
        (Some(n), Some(d)) if n > 0.0 && d > 0.0 => n / d,
        _ => 30.0,
    };
    let mut clips = Vec::new();
    for (kind, key, video) in [("V", "video", true), ("A", "audio", false)] {
        for (i, t) in s.get(key).and_then(Value::as_array).into_iter().flatten().enumerate() {
            for c in t.get("items").and_then(Value::as_array).into_iter().flatten() {
                clips.push(Clip {
                    id: c.get("clip").and_then(Value::as_u64).unwrap_or(0),
                    name: c.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                    start: c.get("start").and_then(Value::as_f64).unwrap_or(0.0) / TICKS,
                    end: c.get("end").and_then(Value::as_f64).unwrap_or(0.0) / TICKS,
                    track: format!("{kind}{}", i + 1),
                    video,
                    link: c.get("link").and_then(Value::as_u64),
                });
            }
        }
    }
    let duration = s.get("duration").and_then(Value::as_f64).unwrap_or(0.0) / TICKS;
    let audio_tracks = s.get("audio").and_then(Value::as_array).map_or(0, Vec::len);
    Ok(Seq { duration, fps, clips, audio_tracks })
}

/// Sort, clamp to `0..len` and merge ranges.
fn tidy(mut r: Vec<(f64, f64)>, len: f64) -> Vec<(f64, f64)> {
    for x in r.iter_mut() {
        x.0 = x.0.clamp(0.0, len);
        x.1 = x.1.clamp(0.0, len);
    }
    r.retain(|(a, b)| b - a > 1e-6);
    r.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut out: Vec<(f64, f64)> = Vec::new();
    for (a, b) in r {
        match out.last_mut() {
            Some(l) if a <= l.1 + 1e-6 => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// `0..len` minus `keep`.
fn complement(keep: &[(f64, f64)], len: f64) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let mut at = 0.0;
    for &(a, b) in keep {
        if a > at {
            out.push((at, a));
        }
        at = at.max(b);
    }
    if at < len {
        out.push((at, len));
    }
    out
}

fn total(r: &[(f64, f64)]) -> f64 {
    r.iter().map(|(a, b)| b - a).sum()
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The timeline time `t` (after some ranges were removed) in today's timeline time.
fn to_original(t: f64, removed: &[(f64, f64)]) -> f64 {
    let mut o = t;
    for &(a, b) in removed {
        if a <= o + 1e-9 {
            o += b - a;
        } else {
            break;
        }
    }
    o
}

/// The preview: what each step does, in plain words, and what disappears from today's timeline.
fn describe(ops: &[Op], seq: &Seq) -> EditPreview {
    let mut removed: Vec<(f64, f64)> = Vec::new();
    let mut marks = Vec::new();
    let mut lines = Vec::new();
    let mut len = seq.duration;
    let name = |id: u64| seq.clips.iter().find(|c| c.id == id).map(|c| format!("\"{}\"", short(&c.name, 28))).unwrap_or_else(|| format!("clip {id}"));
    let remove = |r: Vec<(f64, f64)>, removed: &mut Vec<(f64, f64)>, len: &mut f64| {
        let r = tidy(r, *len);
        *len -= total(&r);
        let mapped: Vec<(f64, f64)> = r.iter().map(|(a, b)| (to_original(*a, removed), to_original(*b - 1e-9, removed) + 1e-9)).collect();
        let mut all = removed.clone();
        all.extend(mapped);
        *removed = tidy(all, seq.duration.max(0.0));
        r
    };
    for op in ops {
        match op {
            Op::Cut(r) => {
                let r = remove(r.clone(), &mut removed, &mut len);
                lines.push(format!("Cut {} ({:.1} s), closing the gaps", plural(r.len(), "range", "ranges"), total(&r)));
            }
            Op::Keep(k) => {
                let k = tidy(k.clone(), len);
                let kept = total(&k);
                let r = remove(complement(&k, len), &mut removed, &mut len);
                lines.push(format!("Keep {} ({kept:.1} s) and cut the rest ({:.1} s)", plural(k.len(), "moment", "moments"), total(&r)));
            }
            Op::Split(at) => {
                marks.extend(at.iter().map(|t| to_original(*t, &removed)));
                lines.push(format!("Split every track at {}", at.iter().map(|t| format!("{t:.1} s")).collect::<Vec<_>>().join(", ")));
            }
            Op::Trim { clip, head, tail } => {
                if let Some(c) = seq.clips.iter().find(|c| c.id == *clip) {
                    let mut r = Vec::new();
                    if *head > 0.0 {
                        r.push((c.start, (c.start + head).min(c.end)));
                    }
                    if *tail > 0.0 {
                        r.push(((c.end - tail).max(c.start), c.end));
                    }
                    let mut all = removed.clone();
                    all.extend(r.iter().copied());
                    removed = tidy(all, seq.duration);
                    len -= total(&r);
                }
                let parts: Vec<String> =
                    [(*head, "start"), (*tail, "end")].iter().filter(|(s, _)| *s > 0.0).map(|(s, w)| format!("{s:.1} s off the {w}")).collect();
                lines.push(format!("Trim {} ({})", name(*clip), parts.join(", ")));
            }
            Op::Delete { clips, ripple } => {
                if *ripple {
                    let r: Vec<(f64, f64)> =
                        clips.iter().filter_map(|id| seq.clips.iter().find(|c| c.id == *id && c.video)).map(|c| (c.start, c.end)).collect();
                    let mut all = removed.clone();
                    all.extend(r.iter().copied());
                    removed = tidy(all, seq.duration);
                    len -= total(&tidy(r, seq.duration));
                }
                let names: Vec<String> = clips.iter().take(4).map(|c| name(*c)).collect();
                lines.push(format!("Remove {}{}", names.join(", "), if clips.len() > 4 { format!(" and {} more", clips.len() - 4) } else { String::new() }));
            }
            Op::Order(clips) => {
                marks.extend(clips.iter().filter_map(|id| seq.clips.iter().find(|c| c.id == *id)).map(|c| c.start));
                lines.push(format!(
                    "Reorder {}: {}",
                    plural(clips.len(), "clip", "clips"),
                    clips.iter().take(6).map(|c| name(*c)).collect::<Vec<_>>().join(" → ")
                ));
            }
            Op::Transition { clips, effect, frames, .. } => {
                let pretty = effect.replace('_', " ");
                match clips {
                    Some(c) => {
                        marks.extend(c.iter().filter_map(|id| seq.clips.iter().find(|x| x.id == *id)).map(|x| x.start));
                        lines.push(format!("{pretty} ({frames} frames) before {}", plural(c.len(), "clip", "clips")));
                    }
                    None => lines.push(format!("{pretty} ({frames} frames) at every cut on V1")),
                }
            }
            Op::Title(t) => {
                let at = f(t, "start_seconds").unwrap_or(0.0);
                marks.push(to_original(at, &removed));
                lines.push(format!("Title \"{}\" at {at:.1} s", short(t.get("text").and_then(Value::as_str).unwrap_or("(changed)"), 40)));
            }
            Op::Captions { lines: l, .. } => lines.push(format!("Captions from what is said ({})", if *l == 1 { "one line" } else { "two lines" })),
            Op::Volume { clips, track, db } => {
                let what = match track {
                    Some(t) => format!("everything on {t}"),
                    None => clips.iter().take(4).map(|c| name(*c)).collect::<Vec<_>>().join(", "),
                };
                lines.push(format!("Volume of {what} to {db:+.0} dB"));
            }
            Op::Music { item, db, duck, .. } => {
                lines.push(format!("Music (item {item}) under the edit at {db:+.0} dB{}", if *duck { ", dipping under speech" } else { "" }))
            }
            Op::Speed { clip, percent } => {
                if let Some(c) = seq.clips.iter().find(|c| c.id == *clip && c.video) {
                    len += (c.end - c.start) * (100.0 / percent - 1.0);
                }
                lines.push(format!("Play {} at {percent:.0} %", name(*clip)));
            }
        }
    }
    EditPreview { before: seq.duration, after: len.max(0.0), removed, marks, lines }
}

/// The preview as text, for the model (and the Plan mode answer).
pub fn preview_text(p: &EditPreview) -> String {
    format!(
        "{}\nTimeline {:.1} s → about {:.1} s{}",
        p.lines.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n"),
        p.before,
        p.after,
        if p.removed.is_empty() { String::new() } else { format!(" ({:.1} s removed)", total(&p.removed)) }
    )
}

/// The plan as Plan mode answers it: what it would do, nothing changed.
pub fn plan_only(ctx: &Ctx, args: &Value) -> String {
    match (parse(args), read(ctx)) {
        (Ok(ops), Ok(seq)) => format!("Plan mode is on, so nothing was changed. This plan would:\n{}", preview_text(&describe(&ops, &seq))),
        (Err(e), _) | (_, Err(e)) => format!("Plan mode is on, so nothing was changed (and the plan has a problem: {e})."),
    }
}

/// Run the plan. Asks first with the preview in Ask mode, and in Auto edits when the plan
/// reshapes the person's footage (cuts, trims, deletes, reorders, speed).
pub fn edit_timeline(ctx: &Ctx, step: u64, args: &Value) -> Outcome {
    let ops = match parse(args) {
        Ok(o) => o,
        Err(e) => return Outcome::err(e),
    };
    let seq = match read(ctx) {
        Ok(s) => s,
        Err(e) => return Outcome::err(e),
    };
    let preview = describe(&ops, &seq);
    let review = ctx.mode == Mode::Ask || ops.iter().any(Op::reshapes);
    if review {
        let summary =
            args.get("summary").and_then(Value::as_str).map(|s| short(s.trim(), 120)).filter(|s| !s.is_empty()).unwrap_or_else(|| "Edit the timeline".into());
        if !crate::tools::ask_with(ctx, step, &summary, &preview.lines.join(" · "), 0, Some(preview.clone())) {
            return Outcome {
                text: "The person chose not to apply this edit. Nothing was changed. Ask what they'd like different, or offer another version.".into(),
                ok: false,
                edited: false,
                credits: 0,
            };
        }
    }
    let mut done = Vec::new();
    let mut failed = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        if ctx.cancel.stopped() {
            failed.push(format!("operation {} and after: stopped by the person", i + 1));
            break;
        }
        match apply(ctx, op) {
            Ok(line) => done.push(line),
            Err(e) => failed.push(format!("operation {} ({}): {e}", i + 1, preview.lines.get(i).map(String::as_str).unwrap_or("?"))),
        }
    }
    let after = read(ctx).ok();
    let mut text = done.join("\n");
    if let Some(a) = &after {
        text.push_str(&format!("\nTimeline {:.1} s → {:.1} s.\n{}", seq.duration, a.duration, listing(a)));
    }
    if done.is_empty() {
        return Outcome::err(format!("nothing was changed: {}", failed.join("; ")));
    }
    if !failed.is_empty() {
        return Outcome::partial(text, &failed, 0);
    }
    Outcome { text, ok: true, edited: true, credits: 0 }
}

/// The clips as they are now (the ids the next step needs), kept short.
fn listing(seq: &Seq) -> String {
    let mut out = String::from("Clips now:");
    let mut by_track: Vec<(String, Vec<&Clip>)> = Vec::new();
    for c in &seq.clips {
        match by_track.iter_mut().find(|(t, _)| *t == c.track) {
            Some((_, v)) => v.push(c),
            None => by_track.push((c.track.clone(), vec![c])),
        }
    }
    for (t, clips) in by_track {
        let shown: Vec<String> = clips.iter().take(40).map(|c| format!("{} {:.2}–{:.2}", c.id, c.start, c.end)).collect();
        out.push_str(&format!("\n  {t}: {}{}", shown.join(", "), if clips.len() > 40 { format!(" … {} more", clips.len() - 40) } else { String::new() }));
    }
    out
}

/// Frame number of `t` seconds (the engine's In/Out and Razor take whole frames).
fn frame(t: f64, fps: f64) -> i64 {
    (t * fps).round() as i64
}

/// Remove ranges from every track, last first, closing each gap (the editor's Extract).
fn cut(ctx: &Ctx, r: Vec<(f64, f64)>) -> Result<String, String> {
    let seq = read(ctx)?;
    let fps = seq.fps;
    // whole frames, merged, at least one frame long
    let mut frames: Vec<(i64, i64)> = tidy(r, seq.duration).into_iter().map(|(a, b)| (frame(a, fps), frame(b, fps))).filter(|(a, b)| b > a).collect();
    frames.dedup();
    if frames.is_empty() {
        return Err("every range was shorter than a frame or outside the timeline".into());
    }
    let mut n = 0;
    let mut problems = Vec::new();
    for (a, b) in frames.iter().rev() {
        let r = exec(ctx, "markers.markIn", json!({"frame": a}))
            .and_then(|_| exec(ctx, "markers.markOut", json!({"frame": b - 1})))
            .and_then(|_| exec(ctx, "sequence.extract", json!({})));
        match r {
            Ok(_) => n += 1,
            Err(e) => problems.push(format!("{:.2}–{:.2} s: {e}", *a as f64 / fps, *b as f64 / fps)),
        }
    }
    let _ = exec(ctx, "markers.clearInOut", json!({}));
    let removed: f64 = frames.iter().map(|(a, b)| (b - a) as f64 / fps).sum();
    let after = read(ctx)?;
    if n == 0 {
        return Err(problems.join("; "));
    }
    // every track should have closed up by the same amount; if not, a track was locked or
    // sync-lock was off on it and is now out of step with the picture
    let mut line = format!("Cut {} ({removed:.1} s); the timeline is now {:.1} s.", plural(n, "range", "ranges"), after.duration);
    if !problems.is_empty() {
        line.push_str(&format!(" Not cut: {}.", problems.join("; ")));
    }
    if (seq.duration - after.duration - removed).abs() > 2.0 / fps && after.duration > 0.0 {
        line.push_str(" Note: the length changed by a different amount than was cut (a locked track, or the cut ran past the end).");
    }
    Ok(line)
}

/// Clips on the main video track in time order.
fn v1(seq: &Seq) -> Vec<&Clip> {
    let mut v: Vec<&Clip> = seq.clips.iter().filter(|c| c.track == "V1").collect();
    v.sort_by(|a, b| a.start.total_cmp(&b.start));
    v
}

fn apply(ctx: &Ctx, op: &Op) -> Result<String, String> {
    match op {
        Op::Cut(r) => cut(ctx, r.clone()),
        Op::Keep(k) => {
            let seq = read(ctx)?;
            let k = tidy(k.clone(), seq.duration);
            if k.is_empty() {
                return Err("none of the ranges to keep is on the timeline".into());
            }
            let gone = complement(&k, seq.duration);
            if gone.is_empty() {
                return Ok("Kept everything (the ranges cover the whole timeline).".into());
            }
            cut(ctx, gone).map(|l| format!("Kept {} ({:.1} s). {l}", plural(k.len(), "moment", "moments"), total(&k)))
        }
        Op::Split(at) => {
            let seq = read(ctx)?;
            let mut n = 0;
            for t in at {
                if *t > 0.0 && *t < seq.duration && exec(ctx, "timeline.razor", json!({"frame": frame(*t, seq.fps)})).is_ok() {
                    n += 1;
                }
            }
            if n == 0 {
                return Err("no split time was inside the timeline".into());
            }
            Ok(format!("Split every track at {}.", plural(n, "point", "points")))
        }
        Op::Trim { clip, head, tail } => {
            let seq = read(ctx)?;
            let c = seq.clips.iter().find(|c| c.id == *clip).ok_or(format!("there is no clip {clip} on the timeline now"))?;
            let len = c.end - c.start;
            if head + tail >= len - 1.0 / seq.fps {
                return Err(format!("that would trim away all of clip {clip} ({len:.1} s long); delete it instead"));
            }
            // the head and tail come out of every track at once (Extract), like a ripple trim
            // with sync lock on: the clip's own sound, and anything under it, stay in step
            let mut r = Vec::new();
            if *head > 0.0 {
                r.push((c.start, c.start + head));
            }
            if *tail > 0.0 {
                r.push((c.end - tail, c.end));
            }
            let (track, start) = (c.track.clone(), c.start);
            cut(ctx, r)?;
            // what's left of the clip may carry a new id (the editor splits it to cut the head)
            let now = read(ctx)?;
            let id = now.clips.iter().find(|x| x.track == track && (x.start - start).abs() < 1.5 / now.fps).map(|x| x.id).unwrap_or(*clip);
            let renamed = if id == *clip { String::new() } else { format!(" It is now clip {id}.") };
            Ok(format!("Trimmed clip {clip} \"{}\": {len:.1} s → {:.1} s, gap closed on every track.{renamed}", short(&c.name, 30), len - head - tail))
        }
        Op::Delete { clips, ripple } => {
            let seq = read(ctx)?;
            let present: Vec<u64> = clips.iter().copied().filter(|id| seq.clips.iter().any(|c| c.id == *id)).collect();
            if present.is_empty() {
                return Err("none of those clips is on the timeline now".into());
            }
            match exec(ctx, if *ripple { "edit.rippleDelete" } else { "edit.clear" }, json!({"clips": present})) {
                Ok(_) => Ok(format!("Removed {}{}.", plural(present.len(), "clip", "clips"), if *ripple { " and closed the gap" } else { "" })),
                // something on another sync-locked track sits under the clip (a title, music):
                // closing the gap on one track only would knock it out of sync, so take the
                // clip's time out of every track instead, as cuts do
                Err(e) if *ripple && e.contains("sync") => {
                    let spans: Vec<(f64, f64)> = seq.clips.iter().filter(|c| present.contains(&c.id)).map(|c| (c.start, c.end)).collect();
                    cut(ctx, spans).map(|l| {
                        format!(
                            "Removed {} with everything under {} on the other tracks, to keep them in sync. {l}",
                            plural(present.len(), "clip", "clips"),
                            if present.len() == 1 { "it" } else { "them" }
                        )
                    })
                }
                Err(e) => Err(e),
            }
        }
        Op::Order(order) => {
            let seq = read(ctx)?;
            let mut list = Vec::new();
            for id in order {
                let c = seq.clips.iter().find(|c| c.id == *id).ok_or(format!("there is no clip {id} on the timeline now"))?;
                list.push(c);
            }
            let track = list.first().map(|c| c.track.clone()).unwrap_or_default();
            if list.iter().any(|c| c.track != track) {
                return Err("all the clips to reorder must be on one track".into());
            }
            let start = list.iter().map(|c| c.start).fold(f64::INFINITY, f64::min);
            let fps = seq.fps;
            let mut t = frame(start, fps);
            let mut moves = Vec::new();
            for c in &list {
                let len = frame(c.end, fps) - frame(c.start, fps);
                moves.push(json!({"clip": c.id, "track": track, "time": ((t as f64 / fps) * TICKS).round() as i64}));
                t += len;
            }
            exec(ctx, "timeline.move", json!({"moves": moves, "linked": true}))?;
            Ok(format!("Reordered {} on {track} (their sound moved with them).", plural(list.len(), "clip", "clips")))
        }
        Op::Transition { clips, effect, frames, audio } => {
            let seq = read(ctx)?;
            let main = v1(&seq);
            let tol = 1.5 / seq.fps;
            // the clips that start right where another ends: there is a cut before them
            let cut_before: Vec<&Clip> = main.iter().skip(1).copied().filter(|c| main.iter().any(|p| (p.end - c.start).abs() < tol)).collect();
            let targets: Vec<&Clip> = match clips {
                None => cut_before,
                Some(ids) => ids.iter().filter_map(|id| seq.clips.iter().find(|c| c.id == *id && c.video)).collect(),
            };
            if targets.is_empty() {
                return Err("there is no cut between two clips there".into());
            }
            let mut ok = 0;
            let mut err = None;
            for c in &targets {
                match exec(ctx, "sequence.applyVideoTransition", json!({"clip": c.id, "effect": effect, "frames": frames, "edge": "in"})) {
                    Ok(_) => ok += 1,
                    Err(e) => err = Some(e),
                }
                if *audio && let Some(a) = seq.clips.iter().find(|a| !a.video && a.link.is_some() && a.link == c.link) {
                    let _ = exec(ctx, "sequence.applyAudioTransition", json!({"clip": a.id, "frames": frames, "edge": "in"}));
                }
            }
            if ok == 0 {
                return Err(err.unwrap_or_else(|| "no transition could be added".into()));
            }
            Ok(format!("{} ({frames} frames) at {}.", effect.replace('_', " "), plural(ok, "cut", "cuts")))
        }
        Op::Title(t) => {
            let out = crate::production::add_title(ctx, t);
            if out.ok { Ok(out.text) } else { Err(out.text) }
        }
        Op::Captions { max_chars, lines } => match exec(ctx, "transcript.createCaptions", json!({"maxChars": max_chars, "lines": lines})) {
            Ok(v) => Ok(format!(
                "Captions made from the transcript{}.",
                v.get("captions").and_then(Value::as_u64).map(|n| format!(" ({n} captions)")).unwrap_or_default()
            )),
            Err(e) if e.contains("transcript") => Err(format!("{e}. Call read_footage with what [\"transcript\"] first: it transcribes the clips")),
            Err(e) => Err(e),
        },
        Op::Volume { clips, track, db } => {
            let seq = read(ctx)?;
            let mut list: Vec<u64> = clips.iter().copied().filter(|id| seq.clips.iter().any(|c| c.id == *id && !c.video)).collect();
            // a video clip named: its linked sound
            for id in clips {
                if let Some(v) = seq.clips.iter().find(|c| c.id == *id && c.video)
                    && let Some(a) = seq.clips.iter().find(|a| !a.video && a.link.is_some() && a.link == v.link)
                {
                    list.push(a.id);
                }
            }
            if let Some(t) = track {
                list.extend(seq.clips.iter().filter(|c| c.track.eq_ignore_ascii_case(t) && !c.video).map(|c| c.id));
            }
            list.sort_unstable();
            list.dedup();
            if list.is_empty() {
                return Err("no sound clips there to change".into());
            }
            exec(ctx, "clip.audioGain", json!({"clips": list, "mode": "set", "db": db}))?;
            Ok(format!("Set {} to {db:+.0} dB.", plural(list.len(), "sound clip", "sound clips")))
        }
        Op::Music { item, db, duck, fade, start } => music(ctx, *item, *db, *duck, *fade, *start),
        Op::Speed { clip, percent } => {
            let seq = read(ctx)?;
            let c = seq.clips.iter().find(|c| c.id == *clip).ok_or(format!("there is no clip {clip} on the timeline now"))?;
            let len = c.end - c.start;
            exec(ctx, "clip.speedDuration", json!({"clips": [clip], "speed": percent, "ripple": true}))?;
            let after = read(ctx)?.clips.iter().find(|c| c.id == *clip).map(|c| c.end - c.start).unwrap_or(0.0);
            Ok(format!("Clip {clip} plays at {percent:.0} %: {len:.1} s → {after:.1} s."))
        }
    }
}

/// Music under the whole edit: its own audio track, cut at the end of the picture, faded in and
/// out, at a level under speech, and (when asked) dipping further whenever someone speaks.
fn music(ctx: &Ctx, item: u64, db: f64, duck: bool, fade: f64, start: f64) -> Result<String, String> {
    let seq = read(ctx)?;
    let end = seq.duration;
    if end <= start {
        return Err("the timeline is empty there; put the picture down first".into());
    }
    exec(ctx, "sequence.addTracks", json!({"video": 0, "audio": 1}))?;
    let track = format!("A{}", seq.audio_tracks + 1);
    let placed = exec(ctx, "timeline.place", json!({"item": item, "seconds": start, "track": track}))?;
    let clip = crate::production::placed_clips(&placed).first().copied().ok_or("the music didn't land on the timeline")?;
    let mut notes = Vec::new();
    // ends with the picture: cut the rest off
    let now = exec(ctx, "sequence.inspect", json!({}))?;
    if let Some((_, e, _, _)) = find_clip(&now, clip)
        && e > end + 0.05
    {
        exec(ctx, "timeline.razor", json!({"frame": frame(end, seq.fps), "track": track}))?;
        let after = read(ctx)?;
        let tail: Vec<u64> = after.clips.iter().filter(|c| c.track == track && c.start >= end - 0.5 / seq.fps).map(|c| c.id).collect();
        if !tail.is_empty() {
            exec(ctx, "edit.clear", json!({"clips": tail}))?;
        }
    } else if let Some((_, e, _, _)) = find_clip(&now, clip)
        && e < end - 1.0
    {
        notes.push(format!("the music ends at {e:.1} s, before the picture ({end:.1} s)"));
    }
    exec(ctx, "clip.audioGain", json!({"clips": [clip], "mode": "set", "db": db}))?;
    if fade > 0.0 {
        let frames = frame(fade, seq.fps).max(2);
        for edge in ["in", "out"] {
            let _ = exec(ctx, "sequence.applyAudioTransition", json!({"clip": clip, "frames": frames, "edge": edge}));
        }
    }
    if duck {
        let r = exec(ctx, "essentialSound.setType", json!({"clips": [clip], "type": "music"}))
            .and_then(|_| exec(ctx, "essentialSound.set", json!({"clips": [clip], "values": {"ducking.enabled": true, "ducking.against_untyped": true}})))
            .and_then(|_| exec(ctx, "essentialSound.generateDucking", json!({"clips": [clip]})));
        if let Err(e) = r {
            notes.push(format!("it couldn't dip under speech ({e})"));
        }
    }
    let mut line =
        format!("Music on {track} (clip {clip}) under the edit at {db:+.0} dB, {fade:.1} s fades{}.", if duck { ", dipping under speech" } else { "" });
    if !notes.is_empty() {
        line.push_str(&format!(" Note: {}.", notes.join("; ")));
    }
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq() -> Seq {
        let c = |id, start, end, track: &str| Clip {
            id,
            name: format!("clip{id}.mp4"),
            start,
            end,
            track: track.into(),
            video: track.starts_with('V'),
            link: Some(id),
        };
        Seq { duration: 30.0, fps: 30.0, clips: vec![c(1, 0.0, 10.0, "V1"), c(2, 10.0, 20.0, "V1"), c(3, 20.0, 30.0, "V1")], audio_tracks: 1 }
    }

    #[test]
    fn a_plan_is_read_or_refused_whole() {
        let ok = parse(&json!({"operations": [
            {"op": "cut", "ranges": [[1, 2], {"start": 5, "end": 6.5}]},
            {"op": "trim", "clip": 2, "head_seconds": 1.5},
            {"op": "transition", "clips": "all"},
            {"op": "music", "item": 9}
        ]}));
        let ops = ok.unwrap_or_default();
        assert_eq!(ops.len(), 4);
        assert_eq!(ops[0], Op::Cut(vec![(1.0, 2.0), (5.0, 6.5)]));
        assert!(matches!(ops[2], Op::Transition { clips: None, .. }));
        assert!(matches!(ops[3], Op::Music { db, duck: true, .. } if (db + 18.0).abs() < 1e-9));
        // one bad operation and nothing runs, with what to fix
        let bad = parse(&json!({"operations": [{"op": "cut", "ranges": [[1, 2]]}, {"op": "explode"}]}));
        assert!(bad.is_err_and(|e| e.contains("operation 2") && e.contains("unknown op")));
        assert!(parse(&json!({"operations": [{"op": "trim", "clip": 2}]})).is_err());
        assert!(parse(&json!({})).is_err());
        // backwards and NaN ranges are dropped, not run
        assert!(parse(&json!({"operations": [{"op": "cut", "ranges": [[5, 2]]}]})).is_err());
    }

    #[test]
    fn the_preview_says_what_goes_and_how_long_it_will_be() {
        let ops = vec![Op::Cut(vec![(2.0, 4.0), (3.0, 5.0), (25.0, 40.0)]), Op::Trim { clip: 2, head: 1.0, tail: 0.0 }];
        let p = describe(&ops, &seq());
        // 2-5 and 25-30 are cut (merged, clamped), then a second off clip 2's start
        assert!((p.after - (30.0 - 3.0 - 5.0 - 1.0)).abs() < 1e-6, "{p:?}");
        assert_eq!(p.lines.len(), 2);
        assert!(p.lines[0].starts_with("Cut 2 ranges (8.0 s)"), "{}", p.lines[0]);
        assert!(p.lines[1].contains("\"clip2.mp4\"") && p.lines[1].contains("1.0 s off the start"));
        assert!(p.removed.iter().any(|(a, b)| (*a - 2.0).abs() < 1e-6 && (*b - 5.0).abs() < 1e-6));
    }

    #[test]
    fn a_second_cut_is_shown_in_todays_timeline_time() {
        // after cutting 0-10, "cut 0-5" means today's 10-15
        let p = describe(&[Op::Cut(vec![(0.0, 10.0)]), Op::Cut(vec![(0.0, 5.0)])], &seq());
        assert_eq!(p.removed.len(), 1);
        assert!((p.removed[0].0).abs() < 1e-6 && (p.removed[0].1 - 15.0).abs() < 1e-6, "{p:?}");
        assert!((p.after - 15.0).abs() < 1e-6);
    }

    #[test]
    fn keep_cuts_everything_else() {
        let p = describe(&[Op::Keep(vec![(4.0, 8.0), (20.0, 26.0)])], &seq());
        assert!((p.after - 10.0).abs() < 1e-6, "{p:?}");
        assert_eq!(p.removed, vec![(0.0, 4.0), (8.0, 20.0), (26.0, 30.0)]);
        assert!(p.lines[0].starts_with("Keep 2 moments (10.0 s)"));
    }

    #[test]
    fn only_reshaping_edits_need_a_review_in_auto_mode() {
        assert!(Op::Cut(vec![(0.0, 1.0)]).reshapes());
        assert!(Op::Order(vec![1, 2]).reshapes());
        assert!(!Op::Captions { max_chars: 32, lines: 1 }.reshapes());
        assert!(!Op::Volume { clips: vec![1], track: None, db: -6.0 }.reshapes());
    }

    #[test]
    fn mapping_back_to_todays_time_skips_removed_ranges() {
        let removed = vec![(2.0, 4.0), (10.0, 12.0)];
        assert!((to_original(1.0, &removed) - 1.0).abs() < 1e-9);
        assert!((to_original(3.0, &removed) - 5.0).abs() < 1e-9);
        assert!((to_original(9.0, &removed) - 13.0).abs() < 1e-9);
    }
}
