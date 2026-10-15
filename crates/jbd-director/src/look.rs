//! The Director's eyes: `look` renders frames of the edit into one contact sheet the model can
//! see (Claude, GPT and Gemini all read images), so it can tell what is in a shot, where the
//! subject is, whether a title covers a face, whether the framing works.
//!
//! The frames are rendered by the editor on the UI thread (`jbd.frames`, director_run.rs); this
//! file picks the times, lays the frames out in a grid ([`contact_sheet`]) and hands the picture
//! to the agent, which shows it to the model in the next message.
//!
//! To change how much the model sees: [`MAX_FRAMES`] and [`FRAME_SIDE`] (more and bigger frames
//! cost more credits per look).

use serde_json::{Value, json};

use crate::tools::{Ctx, Outcome};

/// Most frames in one look.
pub const MAX_FRAMES: usize = 9;
/// Longest side of each frame in the sheet, in pixels.
pub const FRAME_SIDE: u32 = 384;
/// Gap between frames.
const GAP: u32 = 6;

/// Lay frames (RGBA, any size) out left to right, top to bottom: 1 or 2 frames side by side,
/// 3 or 4 in two columns, more in three. Returns the PNG file.
pub fn contact_sheet(frames: &[(u32, u32, Vec<u8>)]) -> Result<Vec<u8>, String> {
    contact_sheet_with(frames, false)
}

/// A faint line every 10 % of a frame, brighter at 50 %, so the model can say where something
/// is ("the ball is at 62 %, 71 %") instead of guessing pixels.
fn draw_grid(img: &mut image::RgbaImage) {
    let (w, h) = img.dimensions();
    let mut line = |x0: u32, y0: u32, x1: u32, y1: u32, strong: bool| {
        let a = if strong { 0.55 } else { 0.28 };
        for y in y0..=y1.min(h.saturating_sub(1)) {
            for x in x0..=x1.min(w.saturating_sub(1)) {
                let p = img.get_pixel_mut(x, y);
                // lighten towards white and darken a neighbour: visible on bright and dark footage
                for c in 0..3 {
                    p.0[c] = (f32::from(p.0[c]) * (1.0 - a) + 255.0 * a) as u8;
                }
            }
        }
    };
    for i in 1..10u32 {
        let strong = i == 5;
        let x = w * i / 10;
        let y = h * i / 10;
        line(x, 0, x, h.saturating_sub(1), strong);
        line(0, y, w.saturating_sub(1), y, strong);
    }
}

/// [contact_sheet], optionally with the 10 % grid on every frame.
pub fn contact_sheet_with(frames: &[(u32, u32, Vec<u8>)], grid: bool) -> Result<Vec<u8>, String> {
    let n = frames.len().clamp(1, MAX_FRAMES);
    let cols: u32 = match n {
        1 => 1,
        2 => 2,
        3 | 4 => 2,
        _ => 3,
    };
    let rows = (n as u32).div_ceil(cols);
    let cw = frames.iter().map(|f| f.0).max().unwrap_or(1);
    let ch = frames.iter().map(|f| f.1).max().unwrap_or(1);
    let mut sheet = image::RgbaImage::from_pixel(cols * cw + (cols - 1) * GAP, rows * ch + (rows - 1) * GAP, image::Rgba([24, 24, 27, 255]));
    for (i, (w, h, px)) in frames.iter().take(MAX_FRAMES).enumerate() {
        let Some(mut img) = image::RgbaImage::from_raw(*w, *h, px.clone()) else { return Err("a frame had the wrong size".into()) };
        if grid {
            draw_grid(&mut img);
        }
        let (c, r) = (i as u32 % cols, i as u32 / cols);
        image::imageops::overlay(&mut sheet, &img, i64::from(c * (cw + GAP)), i64::from(r * (ch + GAP)));
    }
    let mut out = std::io::Cursor::new(Vec::new());
    sheet.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

/// The times to look at: the ones asked for; else across one clip (start, middle, end); else
/// across the whole timeline: its first and last frames, the middle of every transition, and
/// the rest spread evenly (the ends and transitions are where finished edits most often break).
fn times(ctx: &Ctx, args: &Value) -> Result<Vec<f64>, String> {
    if let Some(list) = args.get("seconds").and_then(Value::as_array) {
        let t: Vec<f64> = list.iter().filter_map(Value::as_f64).filter(|s| *s >= 0.0).take(MAX_FRAMES).collect();
        if !t.is_empty() {
            return Ok(t);
        }
    }
    const TICKS: f64 = 254_016_000_000.0;
    let seq = ctx.editor.call("engine.execute", json!({"command": "sequence.inspect", "params": {}}))?;
    let count = args.get("count").and_then(Value::as_u64).map_or(3, |c| c.clamp(1, MAX_FRAMES as u64)) as usize;
    let span = |a: f64, b: f64, n: usize| -> Vec<f64> {
        // evenly inside the range, never on its very first or last frame
        (0..n).map(|i| a + (b - a) * (i as f64 + 0.5) / n as f64).collect()
    };
    if let Some(clip) = args.get("clip").and_then(Value::as_u64) {
        for kind in ["video", "audio"] {
            for c in seq.get(kind).and_then(Value::as_array).into_iter().flatten().flat_map(|t| t.get("items").and_then(Value::as_array).into_iter().flatten())
            {
                if c.get("clip").and_then(Value::as_u64) == Some(clip) {
                    let a = c.get("start").and_then(Value::as_f64).unwrap_or(0.0) / TICKS;
                    let b = c.get("end").and_then(Value::as_f64).unwrap_or(0.0) / TICKS;
                    return Ok(span(a, b, count));
                }
            }
        }
        return Err(format!("there is no clip {clip} on the timeline"));
    }
    let dur = seq.get("duration").and_then(Value::as_f64).unwrap_or(0.0) / TICKS;
    if dur <= 0.0 {
        return Err("the timeline is empty: there is nothing to look at yet".into());
    }
    let count = args.get("count").and_then(Value::as_u64).map_or(6, |c| c.clamp(1, MAX_FRAMES as u64)) as usize;
    Ok(whole_timeline(&seq, dur, count))
}

/// Sample a whole timeline of `dur` seconds with `count` frames (at least the two ends).
pub(crate) fn whole_timeline(seq: &Value, dur: f64, count: usize) -> Vec<f64> {
    const TICKS: f64 = 254_016_000_000.0;
    // one frame in from each end (at 30 fps), so the very first and last pictures are seen
    let edge = (1.0 / 30.0_f64).min(dur / 4.0);
    let mut t = vec![edge, (dur - edge).max(edge)];
    let mut mids: Vec<f64> = seq
        .get("video")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|tr| tr.get("transitions").and_then(Value::as_array).into_iter().flatten())
        .filter_map(|x| Some((x.get("start")?.as_f64()? + x.get("duration")?.as_f64()? / 2.0) / TICKS))
        .filter(|m| *m > 0.0 && *m < dur)
        .collect();
    mids.sort_by(f64::total_cmp);
    let room = count.clamp(2, MAX_FRAMES);
    t.extend(mids.into_iter().take(room.saturating_sub(2)));
    // fill the rest evenly between the ends
    let left = room.saturating_sub(t.len());
    t.extend((0..left).map(|i| dur * (i as f64 + 1.0) / (left as f64 + 1.0)));
    t.sort_by(f64::total_cmp);
    t.dedup_by(|a, b| (*a - *b).abs() < 0.1);
    t
}

/// The `look` tool: frames of the edit as one picture for the model.
pub fn look(ctx: &Ctx, args: &Value) -> Outcome {
    let times = match times(ctx, args) {
        Ok(t) => t,
        Err(e) => return Outcome::err(e),
    };
    // one clip: a grid to point at things with (place_on_object takes a point in % of the frame)
    let grid = args.get("grid").and_then(Value::as_bool).unwrap_or(args.get("clip").is_some());
    let v = match ctx.editor.call("jbd.frames", json!({"seconds": times, "maxSide": FRAME_SIDE, "grid": grid})) {
        Ok(v) => v,
        Err(e) => return Outcome::err(format!("couldn't render the frames: {e}")),
    };
    let Some(png) = v.get("pngBase64").and_then(Value::as_str) else { return Outcome::err("the editor sent no picture") };
    ctx.seen.borrow_mut().push(format!("data:image/png;base64,{png}"));
    let list = times.iter().map(|t| format!("{t:.1} s")).collect::<Vec<_>>().join(", ");
    Outcome::ok(format!(
        "The contact sheet is in the next message: {} frame{} of the edit as it renders now, at {list} (left to right, top to bottom).{}",
        times.len(),
        if times.len() == 1 { "" } else { "s" },
        if grid {
            " Each frame has a grid line every 10 % of its width and height (brighter at 50 %): give positions as [x %, y %] from the top-left."
        } else {
            ""
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_laid_out_in_a_grid() {
        let red = (4u32, 3u32, [255u8, 0, 0, 255].repeat(12));
        let png = contact_sheet(&[red.clone(), red.clone(), red]).unwrap_or_default();
        let img = image::load_from_memory(&png).map(|i| i.to_rgba8()).unwrap_or_else(|e| panic!("{e}"));
        // 3 frames: two columns, two rows
        assert_eq!((img.width(), img.height()), (4 * 2 + GAP, 3 * 2 + GAP));
        assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0, 255]);
    }

    #[test]
    fn a_frame_with_the_wrong_size_is_an_error_not_a_crash() {
        assert!(contact_sheet(&[(4, 4, vec![0; 3])]).is_err());
    }
}
