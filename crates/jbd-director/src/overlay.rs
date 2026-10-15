//! Putting a logo, badge or sticker ON something in the picture — a football, a bottle, a face —
//! and keeping it there while that thing moves.
//!
//! The Director can see the edit (`look`), so it can say where a thing is: a point in per cent of
//! the frame. This file turns that point into a real composite:
//!
//! 1. the overlay goes on the video track above the shot, over exactly that shot;
//! 2. its white (or any solid) background can be keyed out;
//! 3. a small mask is put on the shot at the point, in the shot's own source pixels, and JBD's
//!    mask tracker follows it frame by frame (`masks.track`, the same tracker the Masks panel
//!    uses);
//! 4. the tracked path's centre becomes Position keyframes on the overlay — so the logo rides the
//!    ball instead of sitting in a corner;
//! 5. the helper mask is deleted, leaving a normal, hand-editable composite.
//!
//! The mask is added with mode "None", which the renderer ignores, so the shot itself is never
//! cut into even while tracking.
//!
//! To change how the logo sits: `size_pct` (its width as a per cent of the frame) and
//! [`MAX_TRACK_SECONDS`] (how long a track may run).

use serde_json::{Value, json};

use crate::production::{exec, placed_clips, secs_to_ticks, timeline};
use crate::tools::{Ctx, Outcome};

/// Longest stretch tracked in one call: tracking is real work (a frame at a time), and a logo
/// rarely needs to follow something for longer than this.
const MAX_TRACK_SECONDS: f64 = 12.0;
/// How many tracked frames become Position keyframes (every nth, so a long shot stays editable).
const MAX_KEYS: usize = 60;

/// A 3×2 matrix `[a, b, c, d, e, f]`: x' = a·x + c·y + e, y' = b·x + d·y + f.
#[derive(Clone, Copy, Debug)]
pub struct Matrix([f64; 6]);

impl Matrix {
    fn apply(&self, p: (f64, f64)) -> (f64, f64) {
        let m = self.0;
        (m[0] * p.0 + m[2] * p.1 + m[4], m[1] * p.0 + m[3] * p.1 + m[5])
    }
    /// The matrix that undoes this one (None when it squashes everything to a line).
    fn inverse(&self) -> Option<Matrix> {
        let m = self.0;
        let det = m[0] * m[3] - m[1] * m[2];
        if det.abs() < 1e-12 {
            return None;
        }
        let i = 1.0 / det;
        let (a, b, c, d) = (m[3] * i, -m[1] * i, -m[2] * i, m[0] * i);
        Some(Matrix([a, b, c, d, -(a * m[4] + c * m[5]), -(b * m[4] + d * m[5])]))
    }
}

/// Where a clip's pixels land in the finished frame at `seconds`: its source size and the matrix
/// from source pixels to frame pixels.
fn frame_transform(ctx: &Ctx, clip: u64, seconds: f64) -> Result<((f64, f64), Matrix), String> {
    let v = exec(ctx, "clip.frameTransform", json!({"clip": clip, "seconds": seconds}))?;
    let num = |k: &str, i: usize| v.get(k).and_then(|a| a.get(i)).and_then(Value::as_f64);
    let m: Vec<f64> = v.get("matrix").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_f64).collect();
    let m: [f64; 6] = m.try_into().map_err(|_| "the editor didn't say how the clip is framed".to_string())?;
    Ok(((num("source", 0).unwrap_or(0.0), num("source", 1).unwrap_or(0.0)), Matrix(m)))
}

/// The centre of a mask's path, in the clip's source pixels.
fn mask_centre(path: &Value) -> Option<(f64, f64)> {
    let pts: Vec<(f64, f64)> = path
        .get("vertices")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|v| {
            let p = v.get("p")?;
            Some((p.get(0)?.as_f64()?, p.get(1)?.as_f64()?))
        })
        .collect();
    if pts.is_empty() {
        return None;
    }
    let n = pts.len() as f64;
    Some((pts.iter().map(|p| p.0).sum::<f64>() / n, pts.iter().map(|p| p.1).sum::<f64>() / n))
}

/// The overlay to place: a clip already on the timeline, or a media item to place above the shot.
fn overlay_clip(ctx: &Ctx, args: &Value, start: f64, end: f64, track_index: usize) -> Result<(u64, bool), String> {
    if let Some(c) = args.get("overlay_clip").and_then(Value::as_u64) {
        return Ok((c, false));
    }
    let Some(item) = args.get("item").and_then(Value::as_u64) else {
        return Err("give `item` (the logo's media item) or `overlay_clip` (one already on the timeline)".into());
    };
    // the track above the shot, made if it isn't there yet
    let seq = exec(ctx, "sequence.inspect", json!({}))?;
    let tracks = seq.get("video").and_then(Value::as_array).map_or(0, Vec::len);
    let above = track_index + 2; // "V{n}" is 1-based, one above the shot
    if above > tracks {
        exec(ctx, "sequence.addTracks", json!({"video": above - tracks, "audio": 0}))?;
    }
    let placed =
        exec(ctx, "timeline.place", json!({"item": item, "track": format!("V{above}"), "seconds": start, "duration": secs_to_ticks((end - start).max(0.1))}))?;
    let clip = placed_clips(&placed).first().copied().ok_or_else(|| format!("couldn't put item {item} above the shot"))?;
    let _ = exec(ctx, "clip.rename", json!({"clip": clip, "name": "Overlay"}));
    Ok((clip, true))
}

/// How far (0-255 per channel, summed over RGB) a pixel may be from the background colour and
/// still count as background; [`EDGE`] more is the soft edge.
const NEAR: i32 = 60;
const EDGE: i32 = 90;

/// Cut a picture out of its solid background, like "remove background" in a design tool: the
/// background is flood-filled from the picture's edges (so white INSIDE a logo stays white),
/// made transparent, and the border softened. Returns the cut-out as RGBA.
pub fn cut_out(img: &image::RgbaImage) -> Option<image::RgbaImage> {
    let (w, h) = img.dimensions();
    if w < 2 || h < 2 {
        return None;
    }
    // the background: the most common of the four corner colours
    let corners = [img.get_pixel(0, 0), img.get_pixel(w - 1, 0), img.get_pixel(0, h - 1), img.get_pixel(w - 1, h - 1)];
    let dist = |a: &image::Rgba<u8>, b: &image::Rgba<u8>| (0..3).map(|c| (i32::from(a.0[c]) - i32::from(b.0[c])).abs()).sum::<i32>();
    let bg = *corners.iter().max_by_key(|c| corners.iter().filter(|d| dist(c, d) < NEAR).count())?;
    if bg.0[3] < 128 {
        return None; // already transparent
    }
    let mut out = img.clone();
    let mut seen = vec![false; (w as usize) * (h as usize)];
    let mut stack: Vec<(u32, u32)> = Vec::new();
    for x in 0..w {
        stack.push((x, 0));
        stack.push((x, h - 1));
    }
    for y in 0..h {
        stack.push((0, y));
        stack.push((w - 1, y));
    }
    while let Some((x, y)) = stack.pop() {
        let i = (y as usize) * (w as usize) + x as usize;
        if seen.get(i).copied().unwrap_or(true) {
            continue;
        }
        if let Some(v) = seen.get_mut(i) {
            *v = true;
        }
        let d = dist(img.get_pixel(x, y), bg);
        if d >= EDGE {
            continue; // the logo itself: the fill stops here
        }
        // background fades out; pixels near the edge keep some of their alpha
        let keep = if d <= NEAR { 0.0 } else { f64::from(d - NEAR) / f64::from(EDGE - NEAR) };
        let p = out.get_pixel_mut(x, y);
        p.0[3] = (f64::from(p.0[3]) * keep) as u8;
        if d <= NEAR {
            if x > 0 {
                stack.push((x - 1, y));
            }
            if y > 0 {
                stack.push((x, y - 1));
            }
            if x + 1 < w {
                stack.push((x + 1, y));
            }
            if y + 1 < h {
                stack.push((x, y + 1));
            }
        }
    }
    Some(out)
}

/// Remove the overlay's solid background (a logo on white): a cut-out PNG is made beside the
/// picture and the clip switched to it. Video overlays get a colour key instead.
fn remove_background(ctx: &Ctx, clip: u64, colour: Option<&str>) -> Result<String, String> {
    let seq = exec(ctx, "sequence.inspect", json!({}))?;
    let item = seq
        .get("video")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|t| t.get("items").and_then(Value::as_array).into_iter().flatten())
        .find(|c| c.get("clip").and_then(Value::as_u64) == Some(clip))
        .and_then(|c| c.get("item").and_then(Value::as_u64))
        .ok_or("the overlay isn't on the timeline")?;
    let path = ctx
        .editor
        .call("jbd.mediaPath", json!({"item": item}))?
        .get("path")
        .and_then(Value::as_str)
        .map(std::path::PathBuf::from)
        .ok_or("the overlay isn't a file")?;
    let picture = matches!(path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(), Some("png" | "jpg" | "jpeg" | "webp" | "bmp"));
    if !picture {
        // moving footage: a colour key on its background colour, gentle enough to keep dark marks
        let colour = colour.unwrap_or("#ffffff");
        exec(ctx, "effects.apply", json!({"clips": [clip], "effect": "color_key"}))?;
        exec(ctx, "effects.setParam", json!({"clip": clip, "effect": "color_key", "param": "color", "value": colour}))?;
        exec(ctx, "effects.setParam", json!({"clip": clip, "effect": "color_key", "param": "tolerance", "value": 20.0}))?;
        return Ok("keyed its background colour out".into());
    }
    let img = image::open(&path).map_err(|e| format!("couldn't read the picture ({e})"))?.to_rgba8();
    let cut = cut_out(&img).ok_or("its background isn't a solid colour (or it is already transparent)")?;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("overlay");
    let dir = ctx.editor.media_dir();
    let _ = std::fs::create_dir_all(&dir);
    let out = dir.join(format!("{stem} cut-out.png"));
    cut.save(&out).map_err(|e| format!("couldn't save the cut-out ({e})"))?;
    let imported = exec(ctx, "file.import", json!({"paths": [out.to_string_lossy()]}))?;
    let new_item = imported.get("items").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_u64).ok_or("the cut-out didn't import")?;
    exec(ctx, "clip.replaceFromBin", json!({"clip": clip, "item": new_item}))?;
    Ok(format!("cut it out of its background (\"{stem} cut-out.png\")"))
}

/// Put the overlay at `(x, y)` in frame pixels, sized to `size_pct` of the frame width, at
/// `seconds` (a keyframe when the parameter is animated, else its plain value).
fn put_at(ctx: &Ctx, clip: u64, x: f64, y: f64, seconds: f64) {
    let _ = exec(ctx, "effects.setParam", json!({"clip": clip, "effect": "motion", "param": "position", "value": [x, y], "seconds": seconds}));
}

/// `place_on_object`: put a logo (or any overlay) on a thing in the picture and, if it moves,
/// keep it there.
pub fn place_on_object(ctx: &Ctx, args: &Value) -> Outcome {
    let Some(target) = args.get("target_clip").and_then(Value::as_u64) else { return Outcome::err("missing `target_clip` (the shot the thing is in)") };
    let point: Vec<f64> = args.get("point").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_f64).collect();
    let (px, py) = match point.as_slice() {
        [x, y] => (x / 100.0, y / 100.0),
        _ => return Outcome::err("missing `point`: [x, y] in per cent of the frame, from the top-left (look at the shot first)"),
    };
    if !(0.0..=1.0).contains(&px) || !(0.0..=1.0).contains(&py) {
        return Outcome::err("`point` is in per cent of the frame: [0, 0] is the top-left corner, [100, 100] the bottom-right");
    }
    let seq = match exec(ctx, "sequence.inspect", json!({})) {
        Ok(v) => v,
        Err(e) => return Outcome::err(e),
    };
    let Some((start, end, track_index, true)) = crate::production::find_clip(&seq, target) else {
        return Outcome::err(format!("there is no video clip {target} on the timeline"));
    };
    let tl = match timeline(ctx) {
        Ok(t) => t,
        Err(e) => return Outcome::err(e),
    };
    // where the person (or the model) saw the thing: the middle of the shot unless told otherwise
    let at = args.get("at_seconds").and_then(Value::as_f64).unwrap_or((start + end) / 2.0).clamp(start, (end - 0.05).max(start));
    let (fx, fy) = (px * tl.width, py * tl.height);

    let (overlay, placed_now) = match overlay_clip(ctx, args, start, end, track_index) {
        Ok(v) => v,
        Err(e) => return Outcome::err(e),
    };
    let mut notes = Vec::new();
    let mut cut = String::new();
    if args.get("remove_background").and_then(Value::as_bool).unwrap_or(false) {
        match remove_background(ctx, overlay, args.get("background_color").and_then(Value::as_str)) {
            Ok(how) => cut = format!(" Background: {how}."),
            Err(e) => notes.push(format!("its background is still there ({e})")),
        }
    }
    // size: a share of the frame's width (a logo is usually 8-15 %)
    let size_pct = args.get("size_pct").and_then(Value::as_f64).unwrap_or(12.0).clamp(1.0, 100.0);
    if let Ok((src, _)) = frame_transform(ctx, overlay, at)
        && src.0 > 0.0
    {
        let want = tl.width * size_pct / 100.0;
        let _ =
            exec(ctx, "effects.setParam", json!({"clip": overlay, "effect": "motion", "param": "scale", "value": (want / src.0 * 100.0).clamp(1.0, 1000.0)}));
    }
    put_at(ctx, overlay, fx, fy, at);

    // Follow the thing, unless the shot is static (track: false).
    let want_track = args.get("track").and_then(Value::as_bool).unwrap_or(true);
    let tracked = if want_track { follow(ctx, target, overlay, (fx, fy), at, start, end, &tl) } else { Ok(0) };
    let where_ = format!("clip {overlay} on the shot (clip {target}) at {:.0} %, {:.0} % of the frame", px * 100.0, py * 100.0);
    let made = if placed_now { "Placed the overlay as" } else { "Moved the overlay," };
    match tracked {
        Ok(0) => {
            let text = format!("{made} {where_}, {size_pct:.0} % of the frame wide. It stays where it is (not tracked).{cut}");
            if notes.is_empty() { Outcome { text, ok: true, edited: true, credits: 0 } } else { Outcome::partial(text, &notes, 0) }
        }
        Ok(n) => {
            let text = format!(
                "{made} {where_}, {size_pct:.0} % of the frame wide, and it now follows the thing: {n} position keyframes from {at:.1} s. Look at the shot to check it rides it."
            );
            if notes.is_empty() { Outcome { text, ok: true, edited: true, credits: 0 } } else { Outcome::partial(text, &notes, 0) }
        }
        Err(e) => {
            notes.push(format!("it doesn't follow the thing: {e}. It stays where you put it; say track: false to keep it still, or pick a point with more detail (an edge or a marking, not flat colour)"));
            Outcome::partial(format!("{made} {where_}."), &notes, 0)
        }
    }
}

/// Run the mask tracker in the background (it works a frame at a time; the editor stays live)
/// and wait for it here, on the Director's own thread: until the job is finished and the editor
/// has written every tracked frame into the mask.
fn track(ctx: &Ctx, target: u64, mask: u64, direction: &str, frames: i64, at: f64) -> Result<(), String> {
    let started =
        exec(ctx, "masks.track", json!({"clip": target, "effect": "opacity", "mask": mask, "direction": direction, "frames": frames, "seconds": at}))?;
    let job = started.get("job").and_then(Value::as_u64).ok_or("the tracker didn't start")?;
    let keyframes = || -> usize {
        exec(ctx, "masks.list", json!({"clip": target}))
            .ok()
            .and_then(|v| {
                v.get("masks")
                    .and_then(Value::as_array)
                    .and_then(|a| a.get(mask as usize))
                    .and_then(|m| m.get("pathKeyframes"))
                    .and_then(Value::as_array)
                    .map(Vec::len)
            })
            .unwrap_or(0)
    };
    let began = std::time::Instant::now();
    let mut error = None;
    // 1. the job runs
    loop {
        if ctx.cancel.stopped() {
            let _ = exec(ctx, "jobs.cancel", json!({"job": job}));
            return Err("stopped".into());
        }
        if began.elapsed() > std::time::Duration::from_secs(180) {
            let _ = exec(ctx, "jobs.cancel", json!({"job": job}));
            return Err("tracking took too long".into());
        }
        let jobs = exec(ctx, "jobs.list", json!({})).unwrap_or(Value::Null);
        let me = jobs.as_array().into_iter().flatten().find(|j| j.get("id").and_then(Value::as_u64) == Some(job));
        match me {
            Some(j) if j.get("finished").and_then(Value::as_bool) == Some(true) => {
                error = j.get("result").and_then(|r| r.get("error")).and_then(Value::as_str).map(str::to_string);
                break;
            }
            Some(_) => {}
            // gone from the list: finished and cleared
            None => break,
        }
        std::thread::sleep(std::time::Duration::from_millis(120));
    }
    // 2. the editor writes the tracked frames into the mask (a frame or two later)
    let mut last = keyframes();
    for _ in 0..25 {
        std::thread::sleep(std::time::Duration::from_millis(80));
        let now = keyframes();
        if now == last {
            break;
        }
        last = now;
    }
    match error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Track the thing under `(fx, fy)` through the shot and write the overlay's Position keyframes.
/// Returns how many keyframes were written (0 = nothing to follow).
#[allow(clippy::too_many_arguments)]
fn follow(
    ctx: &Ctx,
    target: u64,
    overlay: u64,
    at_frame: (f64, f64),
    at: f64,
    start: f64,
    end: f64,
    tl: &crate::production::Timeline,
) -> Result<usize, String> {
    let (src, m) = frame_transform(ctx, target, at)?;
    if src.0 <= 0.0 || src.1 <= 0.0 {
        return Err("the shot has no picture to track".into());
    }
    let inv = m.inverse().ok_or("the shot's framing can't be mapped back to its own pixels")?;
    let centre = inv.apply(at_frame);
    // a small ellipse around the point: big enough to hold features, small enough to stay on the
    // thing itself (about a twelfth of the picture)
    let r = (src.0.min(src.1) / 12.0).max(8.0);
    let added =
        exec(ctx, "masks.add", json!({"clip": target, "effect": "opacity", "shape": "ellipse", "center": [centre.0, centre.1], "size": [r * 2.0, r * 2.0]}))?;
    let mask = added.get("mask").and_then(Value::as_u64).unwrap_or(0);
    // "None": the renderer ignores it, so the shot itself is untouched while we track with it
    let _ = exec(ctx, "masks.set", json!({"clip": target, "effect": "opacity", "mask": mask, "mode": "None", "name": "Director tracker"}));
    let drop_mask = || {
        let _ = exec(ctx, "masks.remove", json!({"clip": target, "effect": "opacity", "mask": mask}));
    };

    let rate = tl.frame_rate.max(1.0);
    let after = ((end - at).min(MAX_TRACK_SECONDS) * rate).floor().max(0.0) as i64;
    let before = ((at - start).min(MAX_TRACK_SECONDS) * rate).floor().max(0.0) as i64;
    let mut failure = None;
    for (direction, frames) in [("forward", after), ("backward", before)] {
        if frames <= 0 {
            continue;
        }
        if let Err(e) = track(ctx, target, mask, direction, frames, at) {
            failure.get_or_insert(e);
        }
        if ctx.cancel.stopped() {
            drop_mask();
            return Err("stopped".into());
        }
    }

    // Read the tracked path back frame by frame and turn it into the overlay's Position.
    let step = ((after + before).max(1) as usize).div_ceil(MAX_KEYS).max(1);
    let mut keys: Vec<(f64, (f64, f64))> = Vec::new();
    let mut f = -before;
    while f <= after {
        let t = at + f as f64 / rate;
        if t >= start
            && t < end
            && let Ok(v) = exec(ctx, "masks.list", json!({"clip": target, "seconds": t}))
            && let Some(path) = v.get("masks").and_then(Value::as_array).and_then(|a| a.get(mask as usize)).and_then(|m| m.get("path"))
            && let Some(c) = mask_centre(path)
            && let Ok((_, mt)) = frame_transform(ctx, target, t)
        {
            keys.push((t, mt.apply(c)));
        }
        f += step as i64;
    }
    drop_mask();
    // nothing moved: one position is enough, no keyframes needed
    let moved = keys.iter().any(|(_, p)| (p.0 - at_frame.0).abs() > 1.5 || (p.1 - at_frame.1).abs() > 1.5);
    if !moved || keys.len() < 2 {
        return match failure {
            Some(e) if keys.len() < 2 => Err(e),
            _ => Ok(0),
        };
    }
    keys.sort_by(|a, b| a.0.total_cmp(&b.0));
    // switch Position on at the first keyframe, then write them all
    let first = keys.first().copied().unwrap_or((at, at_frame));
    put_at(ctx, overlay, first.1.0, first.1.1, first.0);
    let _ = exec(ctx, "playhead.set", json!({"seconds": first.0}));
    exec(ctx, "effects.toggleAnimation", json!({"clip": overlay, "effect": "motion", "param": "position"}))?;
    for (t, p) in &keys {
        put_at(ctx, overlay, p.0, p.1, *t);
    }
    // what the engine really kept
    let have = crate::production::read_keyframes(ctx, overlay, "motion", "position").map_or(0, |(_, k)| k.len());
    if have < 2 {
        return Err("the position keyframes didn't stick".into());
    }
    Ok(have)
}
