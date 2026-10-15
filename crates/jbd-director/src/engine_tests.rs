//! The Director's production tools run against the REAL editing engine (a headless
//! `filmcraft_engine::Session` with the demo project), not a fake: the same commands the app
//! runs, so a wrong parameter, a misplaced clip or a lost track shows up here.
//!
//! Only the AI services are faked (no network, no credits): the "video model" writes a real
//! picture file, which the engine imports and places like any clip.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use filmcraft_engine::Session;
use serde_json::{Value, json};

use crate::tools::{self, Ctx};
use crate::{Cancel, Catalogue, Editor, Mode, Model, Reply, Shared, Sound, VideoJob, VideoModel, VideoStatus, lock};

/// Ticks per second.
const T: f64 = 254_016_000_000.0;

/// A 1×1 PNG (the smallest valid picture: something real for the engine to import).
const PNG: [u8; 67] = [
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06,
    0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D,
    0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

/// The editor, as the app gives it to the Director, but on a headless session.
struct SessionEditor {
    s: Mutex<Session>,
    dir: PathBuf,
}

impl SessionEditor {
    fn demo(name: &str) -> Self {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap_or_default();
        let dir = std::env::temp_dir().join(format!("jbd-engine-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        SessionEditor { s: Mutex::new(s), dir }
    }

    fn run(&self, command: &str, params: Value) -> Value {
        self.call("engine.execute", json!({"command": command, "params": params})).unwrap_or_else(|e| panic!("{command}: {e}"))
    }

    /// (clip id, item id, start s, end s) of every clip on video track `track` (0 = V1).
    fn clips(&self, track: usize) -> Vec<(u64, u64, f64, f64)> {
        let seq = self.run("sequence.inspect", json!({}));
        seq["video"][track]["items"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|c| {
                        (
                            c["clip"].as_u64().unwrap_or(0),
                            c["item"].as_u64().unwrap_or(0),
                            c["start"].as_f64().unwrap_or(0.0) / T,
                            c["end"].as_f64().unwrap_or(0.0) / T,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Drop for SessionEditor {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Editor for SessionEditor {
    fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let mut s = lock(&self.s);
        // what the app does every frame: write finished mask-tracking results into the project
        filmcraft_engine::masks::poll(&mut s);
        match method {
            "engine.execute" => {
                let command = params["command"].as_str().unwrap_or("");
                s.execute(command, params.get("params").cloned().unwrap_or(json!({}))).map_err(|e| e.to_string())
            }
            // what the app answers on the UI thread (director_run.rs)
            "jbd.mediaPath" => {
                let item = params["item"].as_u64().unwrap_or(0);
                match s.project.item(filmcraft_engine::project::ItemId(item)).map(|i| &i.kind) {
                    Some(filmcraft_engine::project::ItemKind::Media(m)) => match &m.media {
                        filmcraft_engine::project::MediaRef::File { path } => Ok(json!({"path": path})),
                        _ => Err("not a file".into()),
                    },
                    _ => Err("no such item".into()),
                }
            }
            // what the app does for look (director_run.rs render_frames)
            "jbd.frames" => {
                use base64::Engine as _;
                let (w, h) = s.active_sequence().map(|q| (q.settings.width, q.settings.height)).ok_or("no sequence")?;
                let scale = (384.0 / w.max(h) as f32).min(1.0);
                let mut frames = Vec::new();
                for t in params["seconds"].as_array().into_iter().flatten().filter_map(Value::as_f64) {
                    s.set_playhead(filmcraft_engine::time::Tick::from_seconds_f64(t));
                    if let Some(img) = s.render_program(scale) {
                        frames.push((img.w as u32, img.h as u32, img.over_black_rgba8()));
                    }
                }
                let png = crate::look::contact_sheet(&frames)?;
                Ok(json!({"pngBase64": base64::engine::general_purpose::STANDARD.encode(png)}))
            }
            other => Err(format!("{other} isn't available")),
        }
    }
    fn media_dir(&self) -> PathBuf {
        self.dir.clone()
    }
}

/// The AI services, faked: every "generated clip" is a real picture file; nothing is charged.
struct FakeAi;

impl Model for FakeAi {
    fn chat(&self, _: &str, _: &[crate::ChatMessage], _: &[Value]) -> Result<Reply, String> {
        Err("no model in engine tests".into())
    }
    fn start_video(&self, _: &str, prompt: &str, _: u32, _: &str, _: &str, _: bool) -> Result<VideoJob, String> {
        Ok(VideoJob { job: prompt.to_string(), credits: 18 })
    }
    fn video_status(&self, job: &str, into: &Path) -> Result<VideoStatus, String> {
        let name: String = job.chars().filter(char::is_ascii_alphanumeric).take(24).collect();
        let path = into.join(format!("{name}.png"));
        std::fs::write(&path, PNG).map_err(|e| e.to_string())?;
        Ok(VideoStatus::Done(path))
    }
    fn web(&self, _: &str, _: Value) -> Result<Value, String> {
        Err("offline".into())
    }
    fn download_media(&self, _: &str, _: &Path) -> Result<PathBuf, String> {
        Err("offline".into())
    }
    fn speech(&self, _: &str, _: &str, _: Option<&str>, _: Option<&str>, _: &Path) -> Result<(PathBuf, u32), String> {
        Err("offline".into())
    }
}

fn catalogue() -> Catalogue {
    Catalogue {
        chat: vec![],
        video: vec![VideoModel {
            id: "v".into(),
            label: "Test video".into(),
            vendor: String::new(),
            durations: vec![4, 6, 8],
            resolutions: vec!["720p".into()],
            aspects: vec!["16:9".into(), "9:16".into()],
            audio: false,
            price_per_second: [("720p".to_string(), 0.03)].into_iter().collect(),
            audio_price_per_second: Default::default(),
            default: true,
        }],
        voice: vec![],
        credits: None,
    }
}

fn run_tool(editor: &SessionEditor, name: &str, args: Value) -> tools::Outcome {
    let shared = Mutex::new(Shared::default());
    let cat = catalogue();
    let repaint = || {};
    let ctx = Ctx {
        editor,
        model: &FakeAi,
        shared: &shared,
        cancel: &Cancel::default(),
        mode: Mode::AutoEdits,
        catalogue: &cat,
        video_model: "v",
        repaint: &repaint,
        spent: std::cell::Cell::new(0),
        sound: Sound::Auto,
        external: None,
        seen: Default::default(),
    };
    let step = tools::begin_step(&shared, name, &args);
    tools::run(&ctx, step, name, &args)
}

fn demo_items(editor: &SessionEditor) -> Vec<u64> {
    fn walk(v: &Value, out: &mut Vec<u64>) {
        for c in v["children"].as_array().into_iter().flatten() {
            if c.get("children").is_some() {
                walk(c, out);
            } else if c["type"] == "Movie"
                && let Some(i) = c["item"].as_u64()
            {
                out.push(i);
            }
        }
    }
    let mut out = Vec::new();
    walk(&editor.run("project.inspect", json!({}))["root"], &mut out);
    out
}

#[test]
fn assemble_puts_trimmed_clips_back_to_back_after_the_edit() {
    let ed = SessionEditor::demo("assemble");
    let before = ed.clips(0);
    let end = before.iter().map(|c| c.3).fold(0.0, f64::max);
    let items = demo_items(&ed);
    assert!(items.len() >= 3, "the demo project has footage");
    let out = run_tool(
        &ed,
        "assemble",
        json!({"clips": [
            {"item": items[0], "in_seconds": 1.0, "out_seconds": 3.0},
            {"item": items[1], "in_seconds": 0.5, "out_seconds": 2.0},
            {"item": items[2], "in_seconds": 0.0, "out_seconds": 2.5}
        ]}),
    );
    assert!(out.ok, "{}", out.text);
    let after = ed.clips(0);
    let added: Vec<_> = after.iter().filter(|c| !before.iter().any(|b| b.0 == c.0)).copied().collect();
    assert_eq!(added.len(), 3, "three clips added: {added:?}\n{}", out.text);
    let lens: Vec<f64> = added.iter().map(|c| ((c.3 - c.2) * 10.0).round() / 10.0).collect();
    assert_eq!(lens, vec![2.0, 1.5, 2.5], "each trimmed to its range");
    assert!((added[0].2 - end).abs() < 0.05, "starts where the edit ended ({end}), got {}", added[0].2);
    for w in added.windows(2) {
        assert!((w[1].2 - w[0].3).abs() < 0.05, "back to back: {w:?}");
    }
}

#[test]
fn a_long_title_in_a_vertical_timeline_wraps_and_is_centred() {
    let ed = SessionEditor::demo("title");
    ed.run("file.newSequence", json!({"name": "Reel", "width": 1080, "height": 1920, "fps": 30.0}));
    let out = run_tool(&ed, "add_title", json!({"text": "Nike Dunk Low Panda", "start_seconds": 0.0, "duration_seconds": 3.0, "position": "center"}));
    assert!(out.ok, "{}", out.text);
    assert!(out.text.contains("Nike Dunk / Low Panda"), "two balanced lines: {}", out.text);
    let clip = out
        .text
        .split("(clip ")
        .nth(1)
        .and_then(|r| r.split(')').next())
        .and_then(|c| c.parse::<u64>().ok())
        .unwrap_or_else(|| panic!("clip id in {}", out.text));
    let layers = ed.run("graphics.list", json!({"clip": clip}));
    let text = layers.to_string();
    assert!(text.contains("Nike Dunk\\nLow Panda"), "the graphic holds the wrapped text: {text}");
}

#[test]
fn generated_shots_are_imported_and_laid_out_in_order_with_their_recipes() {
    let ed = SessionEditor::demo("shots");
    let before = ed.clips(0).len();
    let out = run_tool(
        &ed,
        "generate_shots",
        json!({"aspect": "16:9", "shots": [
            {"prompt": "hook shot", "seconds": 4},
            {"prompt": "hero shot", "seconds": 6},
            {"prompt": "end card", "seconds": 4}
        ]}),
    );
    assert!(out.ok, "{}", out.text);
    assert_eq!(out.credits, 3 * 18);
    let after = ed.clips(0);
    assert_eq!(after.len(), before + 3, "{}", out.text);
    // every generated file carries its recipe, so it can be regenerated
    let recipes = std::fs::read_dir(&ed.dir).map(|r| r.flatten().filter(|e| e.path().to_string_lossy().ends_with(".jbd.json")).count()).unwrap_or(0);
    assert_eq!(recipes, 3);
}

#[test]
fn regenerating_a_clip_replaces_it_in_place() {
    let ed = SessionEditor::demo("regen");
    let out = run_tool(&ed, "generate_shots", json!({"aspect": "16:9", "place_at_seconds": 0.0, "shots": [{"prompt": "a quiet street", "seconds": 4}]}));
    assert!(out.ok, "{}", out.text);
    let first = ed.clips(0).into_iter().find(|c| c.2.abs() < 0.05).unwrap_or_else(|| panic!("a clip at 0 s"));
    let out = run_tool(&ed, "regenerate_clip", json!({"clip": first.0, "prompt": "a busy street"}));
    assert!(out.ok, "{}", out.text);
    let now = ed.clips(0).into_iter().find(|c| c.0 == first.0).unwrap_or_else(|| panic!("the clip is still there"));
    assert_ne!(now.1, first.1, "it now shows the new take (a new item)");
    assert!((now.2 - first.2).abs() < 0.01 && (now.3 - first.3).abs() < 0.01, "same position and length: {first:?} → {now:?}");
}

#[test]
fn the_finish_check_sees_a_gap_the_engine_really_left() {
    let ed = SessionEditor::demo("gap");
    let clips = ed.clips(0);
    assert!(clips.len() >= 3);
    assert!(crate::agent::finish_issues(&[], &ed).is_empty(), "the demo edit has no gaps");
    // lift (not ripple-delete) the second clip: it leaves black behind
    let second = clips[1].0;
    ed.run("edit.clear", json!({"clips": [second]}));
    let issues = crate::agent::finish_issues(&[], &ed);
    assert!(issues.iter().any(|i| i.contains("gaps")), "{issues:?}");
}

// ---- the composition bugs from the 15 Oct review, reproduced on the real engine -----------

/// The scale value of a clip's motion effect, as sequence.inspect reports it ("Float(150.0)").
fn motion_scale(ed: &SessionEditor, clip: u64) -> String {
    let seq = ed.run("sequence.inspect", json!({}));
    for t in seq["video"].as_array().into_iter().flatten() {
        for c in t["items"].as_array().into_iter().flatten() {
            if c["clip"].as_u64() == Some(clip) {
                for e in c["effects"].as_array().into_iter().flatten() {
                    if e["effect"] == "motion" {
                        return e["params"]["scale"]["value"].as_str().unwrap_or("").to_string();
                    }
                }
            }
        }
    }
    String::new()
}

#[test]
fn a_title_placed_by_its_left_edge_is_caught_and_add_title_fixes_it() {
    let ed = SessionEditor::demo("overflow");
    ed.run("file.newSequence", json!({"name": "Reel", "width": 1080, "height": 1920, "fps": 30.0}));
    // what the Director once did: raw text at the frame's centre, left-aligned, big
    let made = ed.run("graphics.newText", json!({"text": "EXPORT IN 4K", "size": 160, "position": [540, 330], "seconds": 3.0}));
    let clip = made["clip"].as_u64().unwrap_or_else(|| panic!("a clip: {made}"));
    let issues = crate::agent::finish_issues(&[], &ed);
    assert!(issues.iter().any(|i| i.contains("runs outside the frame")), "the finish check must see it: {issues:?}");
    // the fix the finish check suggests
    let out = run_tool(&ed, "add_title", json!({"clip": clip}));
    assert!(out.ok, "{}", out.text);
    let issues = crate::agent::finish_issues(&[], &ed);
    assert!(!issues.iter().any(|i| i.contains("outside the frame")), "fixed: {issues:?}\n{}", out.text);
}

#[test]
fn raw_text_commands_are_redirected_to_add_title() {
    let ed = SessionEditor::demo("redirect");
    let out = run_tool(&ed, "run_command", json!({"id": "graphics.newText", "params": {"text": "x"}}));
    assert!(!out.ok && out.text.contains("add_title"), "{}", out.text);
}

#[test]
fn generated_footage_is_scaled_to_fill_the_frame() {
    let ed = SessionEditor::demo("fill");
    ed.run("file.newSequence", json!({"name": "Reel", "width": 1080, "height": 1920, "fps": 30.0}));
    let out = run_tool(&ed, "generate_shots", json!({"aspect": "9:16", "place_at_seconds": 0.0, "shots": [{"prompt": "a street", "seconds": 4}]}));
    assert!(out.ok, "{}", out.text);
    let clip = ed.clips(0).first().map(|c| c.0).unwrap_or_else(|| panic!("a clip on V1"));
    let scale = motion_scale(&ed, clip);
    assert!(!scale.is_empty() && !scale.contains("(100.0)"), "the (1×1 px) clip must be scaled up to fill the frame, scale is {scale:?}");
}

#[test]
fn animation_times_are_from_the_clips_own_start() {
    let ed = SessionEditor::demo("animate");
    // a title that starts 12 s into the timeline
    let out = run_tool(&ed, "add_title", json!({"text": "Classic. Clean.", "start_seconds": 12.0, "duration_seconds": 3.0}));
    assert!(out.ok, "{}", out.text);
    let clip = out
        .text
        .split("(clip ")
        .nth(1)
        .and_then(|r| r.split(')').next())
        .and_then(|c| c.parse::<u64>().ok())
        .unwrap_or_else(|| panic!("clip id in {}", out.text));
    let a = run_tool(&ed, "animate", json!({"clip": clip, "preset": "pop_in"}));
    assert!(a.ok, "pop_in must leave real keyframes: {}", a.text);
    let seq = ed.run("sequence.inspect", json!({}));
    // two distinct keyframes (keyframes at the same time would replace each other: 1)
    assert!(crate::production::keyframes(&seq, clip, "motion", "scale") >= 2, "{}", a.text);
    assert!(crate::production::keyframes(&seq, clip, "opacity", "opacity") >= 2, "{}", a.text);
}

#[test]
fn inspect_returns_whole_json_or_a_summary_never_half() {
    let ed = SessionEditor::demo("inspect");
    let summary = run_tool(&ed, "inspect", json!({"what": "sequence"}));
    assert!(summary.ok && summary.text.contains("Open timeline"), "{}", summary.text);
    let clip = ed.clips(0).first().map(|c| c.0).unwrap_or(0);
    let one = run_tool(&ed, "inspect", json!({"what": "sequence", "clip": clip}));
    assert!(serde_json::from_str::<Value>(&one.text).is_ok(), "one clip is valid JSON: {}", one.text);
    let big = json!({"items": (0..5000).map(|i| json!({"id": i, "name": "x".repeat(20)})).collect::<Vec<_>>()});
    let text = crate::tools::complete_json(&big);
    assert!(text.starts_with("(The full result is") && text.contains("items"), "{text}");
}

#[test]
fn look_shows_the_model_a_real_contact_sheet_of_the_edit() {
    let ed = SessionEditor::demo("look");
    let shared = Mutex::new(Shared::default());
    let cat = catalogue();
    let repaint = || {};
    let ctx = Ctx {
        editor: &ed,
        model: &FakeAi,
        shared: &shared,
        cancel: &Cancel::default(),
        mode: Mode::Plan,
        catalogue: &cat,
        video_model: "v",
        repaint: &repaint,
        spent: std::cell::Cell::new(0),
        sound: Sound::Auto,
        external: None,
        seen: Default::default(),
    };
    let clip = ed.clips(0).first().map(|c| c.0).unwrap_or_else(|| panic!("a clip"));
    // allowed in Plan mode: looking changes nothing
    let out = tools::run(&ctx, 1, "look", &json!({"clip": clip}));
    assert!(out.ok, "{}", out.text);
    let seen = ctx.seen.borrow().clone();
    assert_eq!(seen.len(), 1, "one contact sheet for the model");
    use base64::Engine as _;
    let b64 = seen[0].strip_prefix("data:image/png;base64,").unwrap_or_else(|| panic!("a PNG data URL"));
    let png = base64::engine::general_purpose::STANDARD.decode(b64).unwrap_or_default();
    let img = image::load_from_memory(&png).unwrap_or_else(|e| panic!("{e}"));
    // three frames of a 1920x1080 edit at 384 px: two columns, two rows
    assert!(img.width() > 700 && img.height() > 400, "{}x{}", img.width(), img.height());
    // and it isn't all black: the demo footage really rendered
    let rgb = img.to_rgb8();
    assert!(rgb.pixels().any(|p| p.0.iter().any(|c| *c > 40)), "the frames show something");
}

// ---- the 15 Oct assessment, reproduced on the real engine ----------------------------------

/// One parameter's keyframes as (seconds from the clip's start, value).
fn keys_of(ed: &SessionEditor, clip: u64, effect: &str, param: &str) -> Vec<(f64, Value)> {
    let v = ed.run("effects.keyframes", json!({"clip": clip, "effect": effect, "param": param}));
    v["keyframes"].as_array().into_iter().flatten().map(|k| (k["clipSeconds"].as_f64().unwrap_or(-1.0), k["value"].clone())).collect()
}

#[test]
fn zooming_a_filled_clip_never_brings_the_borders_back() {
    let ed = SessionEditor::demo("zoomfill");
    ed.run("file.newSequence", json!({"name": "Reel", "width": 1080, "height": 1920, "fps": 30.0}));
    let out = run_tool(&ed, "generate_shots", json!({"aspect": "9:16", "place_at_seconds": 0.0, "shots": [{"prompt": "a street", "seconds": 4}]}));
    assert!(out.ok, "{}", out.text);
    let clip = ed.clips(0).first().map(|c| c.0).unwrap_or_else(|| panic!("a clip on V1"));
    let fill = ed.run("effects.keyframes", json!({"clip": clip, "param": "scale"}))["value"].as_f64().unwrap_or(0.0);
    assert!(fill > 100.0, "filled first: {fill}");
    for preset in ["zoom_in", "zoom_out"] {
        let a = run_tool(&ed, "animate", json!({"clip": clip, "preset": preset}));
        assert!(a.ok, "{preset}: {}", a.text);
        let keys = keys_of(&ed, clip, "motion", "scale");
        assert!(keys.len() >= 2, "{keys:?}");
        for (t, v) in &keys {
            let s = v.as_f64().unwrap_or(0.0);
            assert!(s >= fill - 0.5, "{preset}: scale {s} at {t:.2} s is below the fill ({fill}): the borders would come back");
        }
    }
}

#[test]
fn an_animation_that_only_half_worked_says_so() {
    let ed = SessionEditor::demo("partial");
    let clip = ed.clips(0).first().map(|c| c.0).unwrap_or_else(|| panic!("a clip"));
    let a = run_tool(
        &ed,
        "animate",
        json!({"clip": clip, "keyframes": [
            {"param": "scale", "at_seconds": 0.0, "value": 100},
            {"param": "scale", "at_seconds": 1.0, "value": 120},
            // a position needs [x, y]: this one can't be set
            {"param": "position", "at_seconds": 0.0, "value": "left"},
            {"param": "position", "at_seconds": 1.0, "value": "right"}
        ]}),
    );
    assert!(!a.ok, "not a success: {}", a.text);
    assert!(a.text.starts_with(crate::tools::PARTLY) && a.text.contains("scale") && a.text.contains("position"), "{}", a.text);
    // the part that worked is really there, at its times
    let keys = keys_of(&ed, clip, "motion", "scale");
    assert!(keys.iter().any(|(t, v)| t.abs() < 0.05 && v.as_f64() == Some(100.0)), "{keys:?}");
    assert!(keys.iter().any(|(t, v)| (t - 1.0).abs() < 0.05 && v.as_f64() == Some(120.0)), "{keys:?}");
}

#[test]
fn a_title_moved_off_the_frame_is_not_finished() {
    let ed = SessionEditor::demo("offscreen");
    ed.run("file.newSequence", json!({"name": "Reel", "width": 1080, "height": 1920, "fps": 30.0}));
    let out = run_tool(&ed, "add_title", json!({"text": "JUST DO IT.", "start_seconds": 0.0, "duration_seconds": 3.0}));
    assert!(out.ok, "{}", out.text);
    let clip = out
        .text
        .split("(clip ")
        .nth(1)
        .and_then(|r| r.split(')').next())
        .and_then(|c| c.parse::<u64>().ok())
        .unwrap_or_else(|| panic!("clip id in {}", out.text));
    assert!(crate::agent::finish_issues(&[], &ed).iter().all(|i| !i.contains("outside the frame")), "a fresh title is inside");
    // the clip's own Motion pushes it far off the right edge: the text layer itself is unchanged
    ed.run("effects.setParam", json!({"clip": clip, "effect": "motion", "param": "position", "value": [4000, 960]}));
    let issues = crate::agent::finish_issues(&[], &ed);
    assert!(issues.iter().any(|i| i.contains("outside the frame")), "the finish check must see it: {issues:?}");
}

#[test]
fn add_title_brings_a_title_moved_off_the_frame_back() {
    let ed = SessionEditor::demo("offscreen-fix");
    ed.run("file.newSequence", json!({"name": "Reel", "width": 1080, "height": 1920, "fps": 30.0}));
    let out = run_tool(&ed, "add_title", json!({"text": "JUST DO IT.", "start_seconds": 0.0, "duration_seconds": 3.0}));
    let clip = out
        .text
        .split("(clip ")
        .nth(1)
        .and_then(|r| r.split(')').next())
        .and_then(|c| c.parse::<u64>().ok())
        .unwrap_or_else(|| panic!("clip id in {}", out.text));
    ed.run("effects.setParam", json!({"clip": clip, "effect": "motion", "param": "position", "value": [4000, 960]}));
    // the fix the finish check suggests
    let fixed = run_tool(&ed, "add_title", json!({"clip": clip}));
    assert!(fixed.ok, "{}", fixed.text);
    let issues = crate::agent::finish_issues(&[], &ed);
    assert!(!issues.iter().any(|i| i.contains("outside the frame")), "{issues:?}\n{}", fixed.text);
}

#[test]
fn a_logo_placed_on_a_moving_thing_follows_it() {
    let ed = SessionEditor::demo("onobject");
    let (clip, _, start, end) = ed.clips(0).first().copied().unwrap_or_else(|| panic!("a shot"));
    let len = end - start;
    // the shot slides from left to right across the frame (its own Motion), so whatever is at
    // the centre of the picture moves 600 px while it plays
    ed.run("playhead.set", json!({"seconds": start}));
    ed.run("effects.toggleAnimation", json!({"clip": clip, "effect": "motion", "param": "position"}));
    ed.run("effects.setParam", json!({"clip": clip, "effect": "motion", "param": "position", "value": [660, 540], "seconds": start}));
    ed.run("effects.setParam", json!({"clip": clip, "effect": "motion", "param": "position", "value": [1260, 540], "seconds": end - 0.05}));
    // the logo
    let logo = ed.dir.join("logo.png");
    std::fs::write(&logo, PNG).unwrap_or_else(|e| panic!("{e}"));
    let item = ed.run("file.import", json!({"paths": [logo.to_string_lossy()]}))["items"][0].as_u64().unwrap_or_else(|| panic!("imported"));
    let mid = start + len / 2.0;
    let out = run_tool(&ed, "place_on_object", json!({"target_clip": clip, "item": item, "point": [50, 50], "at_seconds": mid, "size_pct": 10}));
    assert!(out.ok, "{}", out.text);
    // the overlay is on V2, over exactly the shot
    let overlay = ed.clips(1).into_iter().find(|c| c.1 == item).unwrap_or_else(|| panic!("the logo on V2: {}", out.text));
    assert!((overlay.2 - start).abs() < 0.05 && (overlay.3 - end).abs() < 0.05, "spans the shot: {overlay:?}");
    // it moves with the picture: left early, right late
    let keys = keys_of(&ed, overlay.0, "motion", "position");
    assert!(keys.len() >= 2, "position keyframes: {keys:?}\n{}", out.text);
    let x = |v: &Value| v[0].as_f64().unwrap_or(0.0);
    let (first, last) = (keys.first().map(|k| x(&k.1)).unwrap_or(0.0), keys.last().map(|k| x(&k.1)).unwrap_or(0.0));
    assert!(last - first > 300.0, "follows the shot's movement: {first:.0} → {last:.0}");
    // the helper mask is gone; the shot itself is untouched
    let masks = ed.run("masks.list", json!({"clip": clip}));
    assert!(masks["masks"].as_array().is_some_and(Vec::is_empty), "{masks}");
}

#[test]
fn clips_still_being_made_when_jbd_closed_can_be_fetched_later() {
    let ed = SessionEditor::demo("pending");
    let recipe = crate::production::Generation {
        prompt: "a quiet street".into(),
        model: "v".into(),
        model_label: "Test video".into(),
        seconds: 4,
        aspect: "16:9".into(),
        resolution: "720p".into(),
        audio: false,
    };
    let jobs = vec![crate::production::PendingJob { job: "street shot".into(), recipe }];
    std::fs::write(crate::production::jobs_file(&ed.dir), serde_json::to_string(&jobs).unwrap_or_default()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(crate::production::pending_jobs(&ed.dir).len(), 1);
    let (files, still, errors) = crate::production::fetch_pending(&FakeAi, &ed.dir);
    assert_eq!((files.len(), still), (1, 0), "{errors:?}");
    // downloaded with its recipe (so it can be regenerated), and no longer pending
    assert!(crate::production::read_recipe(&files[0]).is_some_and(|r| r.prompt == "a quiet street"));
    assert!(crate::production::pending_jobs(&ed.dir).is_empty());
    assert!(!crate::production::jobs_file(&ed.dir).exists());
}

#[test]
fn a_logo_on_white_loses_its_white_box_but_keeps_its_mark() {
    let ed = SessionEditor::demo("keylogo");
    let (clip, _, start, end) = ed.clips(0).first().copied().unwrap_or_else(|| panic!("a shot"));
    // a 64×64 logo: white background, a black square mark in the middle (like a JPEG logo)
    let mut img = image::RgbaImage::from_pixel(64, 64, image::Rgba([252, 252, 252, 255]));
    for y in 16..48 {
        for x in 16..48 {
            img.put_pixel(x, y, image::Rgba([10, 10, 10, 255]));
        }
    }
    let logo = ed.dir.join("logo-white.png");
    img.save(&logo).unwrap_or_else(|e| panic!("{e}"));
    let item = ed.run("file.import", json!({"paths": [logo.to_string_lossy()]}))["items"][0].as_u64().unwrap_or_else(|| panic!("imported"));
    let mid = (start + end) / 2.0;
    let out = run_tool(
        &ed,
        "place_on_object",
        json!({"target_clip": clip, "item": item, "point": [25, 75], "at_seconds": mid, "size_pct": 20, "remove_background": true, "track": false}),
    );
    assert!(out.ok, "{}", out.text);
    // render the frame: where the logo's white was, the footage shows; its mark stays dark
    let mut s = lock(&ed.s);
    s.set_playhead(filmcraft_engine::time::Tick::from_seconds_f64(mid));
    let frame = s.render_program(0.5).unwrap_or_else(|| panic!("a frame"));
    drop(s);
    let px = frame.over_black_rgba8();
    let (w, h) = (frame.w, frame.h);
    let at = |fx: f64, fy: f64| {
        let (x, y) = ((fx * w as f64) as usize, (fy * h as f64) as usize);
        let i = (y * w + x) * 4;
        [px[i], px[i + 1], px[i + 2]]
    };
    // the logo is 20 % of the width (384 px of 1920) centred at 25 %, 75 %: its white corner is
    // 0.08 of the width left of the centre, the mark at the centre
    let corner = at(0.25 - 0.085, 0.75 - 0.085 * 16.0 / 9.0);
    let mark = at(0.25, 0.75);
    assert!(corner.iter().any(|c| *c < 200), "the white box is keyed out: {corner:?}");
    assert!(mark.iter().all(|c| *c < 25), "the black mark stays (10, 10, 10): {mark:?}");
}
