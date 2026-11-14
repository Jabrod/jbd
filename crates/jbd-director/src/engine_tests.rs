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
            // what the app answers on its footage thread (director_run.rs read_footage)
            "jbd.analyze" | "jbd.transcribe" => {
                let seq = s.state.active_sequence.ok_or("no sequence")?;
                let project = s.project.clone();
                let provider = s.media.provider(project.clone(), s.services.clone());
                if method == "jbd.analyze" {
                    crate::footage::analyze(&project, seq, &provider, &params, &|| false)
                } else {
                    let items: Vec<u64> = params["items"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect();
                    let t = crate::footage::transcriber(s.transcriber.clone(), "whisper-base")?;
                    let list = crate::footage::transcribe(&project, &provider, &items, t.as_ref(), None, &|| false)?;
                    Ok(json!({"transcripts": list.into_iter().map(|(item, t)| json!({"item": item, "transcript": t})).collect::<Vec<_>>()}))
                }
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

// ---- editing the person's own footage (read_footage, edit_timeline) ----------------------------

/// A 16-bit mono WAV at 48 kHz: a voice-like tone where `pattern` says loud, quiet room noise
/// elsewhere. Real bytes on disk, imported and decoded by the engine like any recording.
fn write_wav(path: &Path, pattern: &[(f64, bool)]) {
    let rate = 48_000u32;
    let mut samples: Vec<i16> = Vec::new();
    let mut n = 0u64;
    for &(secs, loud) in pattern {
        for _ in 0..(secs * f64::from(rate)) as usize {
            let t = n as f64 / f64::from(rate);
            let noise = ((n.wrapping_mul(2_654_435_761) >> 7) % 1000) as f64 / 1000.0 - 0.5;
            let voice = if loud { 0.3 * (t * 220.0 * std::f64::consts::TAU).sin() * (1.0 + 0.3 * (t * 3.0 * std::f64::consts::TAU).sin()) } else { 0.0 };
            samples.push(((voice + 0.002 * noise) * 32767.0) as i16);
            n += 1;
        }
    }
    let data = (samples.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + data as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, b).unwrap_or_else(|e| panic!("{e}"));
}

/// Talk 0–2 s, pause 2–3.5 s, talk 3.5–6.5 s, a breath, talk 6.6–7.6 s, then 1.9 s of nothing.
const TALK: [(f64, bool); 6] = [(2.0, true), (1.5, false), (3.0, true), (0.1, false), (1.0, true), (1.9, false)];

impl SessionEditor {
    /// A fresh 1280x720 30 fps timeline like a talking-head recording: a picture on V1 and the
    /// recording on A1, both 9.5 s.
    fn talking_head(name: &str) -> Self {
        let mut s = Session::default();
        s.execute("file.newSequence", json!({"name": "Talk", "width": 1280, "height": 720, "fps": 30.0})).unwrap_or_else(|e| panic!("{e}"));
        let dir = std::env::temp_dir().join(format!("jbd-engine-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let ed = SessionEditor { s: Mutex::new(s), dir };
        let wav = ed.dir.join("talk.wav");
        write_wav(&wav, &TALK);
        let png = ed.dir.join("face.png");
        image::RgbaImage::from_pixel(64, 36, image::Rgba([90, 120, 160, 255])).save(&png).unwrap_or_else(|e| panic!("{e}"));
        let items = ed.run("file.import", json!({"paths": [png.to_string_lossy(), wav.to_string_lossy()]}));
        let (pic, rec) = (items["items"][0].as_u64().unwrap_or(0), items["items"][1].as_u64().unwrap_or(0));
        ed.run("timeline.place", json!({"item": pic, "seconds": 0.0, "track": "V1", "duration": (9.5 * T) as i64}));
        ed.run("timeline.place", json!({"item": rec, "seconds": 0.0, "track": "A1"}));
        ed
    }

    /// (clip, start s, end s) on audio track `track` (0 = A1).
    fn audio(&self, track: usize) -> Vec<(u64, f64, f64)> {
        let seq = self.run("sequence.inspect", json!({}));
        seq["audio"][track]["items"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| (c["clip"].as_u64().unwrap_or(0), c["start"].as_f64().unwrap_or(0.0) / T, c["end"].as_f64().unwrap_or(0.0) / T))
            .collect()
    }

    fn duration(&self) -> f64 {
        self.run("sequence.inspect", json!({}))["duration"].as_f64().unwrap_or(0.0) / T
    }

    fn undo_depth(&self) -> usize {
        lock(&self.s).history.undo.len()
    }
}

/// Run a tool with the person answering its review card (`yes`, or Skip). Returns what the tool
/// said and the preview the card showed.
fn run_reviewed(ed: &SessionEditor, name: &str, args: Value, mode: Mode, yes: bool) -> (tools::Outcome, Option<crate::EditPreview>) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let shared = Mutex::new(Shared::default());
    let shown = Mutex::new(None);
    let done = AtomicBool::new(false);
    let cat = catalogue();
    std::thread::scope(|sc| {
        sc.spawn(|| {
            while !done.load(Ordering::Relaxed) {
                if let Some(a) = lock(&shared).ask.as_mut().filter(|a| a.answer.is_none()) {
                    *lock(&shown) = Some(a.preview.clone());
                    a.answer = Some(yes);
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        });
        let repaint = || {};
        let ctx = Ctx {
            editor: ed,
            model: &FakeAi,
            shared: &shared,
            cancel: &Cancel::default(),
            mode,
            catalogue: &cat,
            video_model: "v",
            repaint: &repaint,
            spent: std::cell::Cell::new(0),
            sound: Sound::Auto,
            external: None,
            seen: Default::default(),
        };
        let step = tools::begin_step(&shared, name, &args);
        let out = tools::run(&ctx, step, name, &args);
        done.store(true, Ordering::Relaxed);
        (out, lock(&shown).take().flatten())
    })
}

fn silences(ed: &SessionEditor) -> Vec<(f64, f64)> {
    let v = ed.call("jbd.analyze", json!({"what": ["silences"]})).unwrap_or_else(|e| panic!("{e}"));
    v["silences"].as_array().into_iter().flatten().map(|p| (p[0].as_f64().unwrap_or(0.0), p[1].as_f64().unwrap_or(0.0))).collect()
}

#[test]
fn read_footage_finds_the_pauses_in_a_real_recording() {
    let ed = SessionEditor::talking_head("silences");
    let s = silences(&ed);
    // the 1.5 s pause (padded inside) and the quiet tail (to the very end); the 0.1 s breath
    // between two phrases is not a pause
    assert_eq!(s.len(), 2, "{s:?}");
    assert!((s[0].0 - 2.12).abs() < 0.08 && (s[0].1 - 3.38).abs() < 0.08, "{s:?}");
    assert!((s[1].0 - 7.72).abs() < 0.08 && (s[1].1 - 9.5).abs() < 0.05, "{s:?}");
    // and the model reads them as ranges it can paste into a cut
    let out = run_tool(&ed, "read_footage", json!({"what": ["silences"]}));
    assert!(out.ok, "{}", out.text);
    assert!(out.text.contains("SILENCES (2 pauses"), "{}", out.text);
}

#[test]
fn cutting_the_silences_shortens_picture_and_sound_together_and_undoes_cleanly() {
    let ed = SessionEditor::talking_head("cut");
    let before = ed.run("sequence.inspect", json!({}));
    let depth = ed.undo_depth();
    let s = silences(&ed);
    let removed: f64 = s.iter().map(|(a, b)| b - a).sum();
    let ranges: Vec<Value> = s.iter().map(|(a, b)| json!([a, b])).collect();
    let (out, preview) =
        run_reviewed(&ed, "edit_timeline", json!({"summary": "Cut the pauses", "operations": [{"op": "cut", "ranges": ranges}]}), Mode::AutoEdits, true);
    assert!(out.ok, "{}", out.text);
    // the person saw it first: what goes, and how long it will be
    let p = preview.unwrap_or_else(|| panic!("a cut is reviewed before it happens"));
    assert!((p.before - 9.5).abs() < 0.05 && (p.after - (9.5 - removed)).abs() < 0.05, "{p:?}");
    assert_eq!(p.removed.len(), 2);
    // it happened on the timeline: shorter by what was cut (to the frame), picture and sound alike
    let d = ed.duration();
    assert!((d - (9.5 - removed)).abs() < 0.07, "now {d}, expected {}\n{}", 9.5 - removed, out.text);
    let (v, a) = (ed.clips(0), ed.audio(0));
    assert_eq!(v.len(), 2, "the picture is cut where the pause was: {v:?}");
    assert_eq!(a.len(), 2, "and so is the sound: {a:?}");
    for (pv, pa) in v.iter().zip(&a) {
        assert!((pv.2 - pa.1).abs() < 1e-6 && (pv.3 - pa.2).abs() < 1e-6, "picture and sound stay in sync: {v:?} {a:?}");
    }
    assert!((v[1].2 - v[0].3).abs() < 1e-6, "no gap left where the pause was: {v:?}");
    // the pauses really are gone: what's left has no silence worth cutting
    assert!(silences(&ed).iter().all(|(a, b)| b - a < 0.5), "{:?}", silences(&ed));
    // every step it made is an ordinary undo step; undoing them gives back the exact timeline
    let steps = ed.undo_depth() - depth;
    assert!(steps > 0);
    for _ in 0..steps {
        lock(&ed.s).undo();
    }
    let back = ed.run("sequence.inspect", json!({}));
    assert_eq!(back["duration"], before["duration"]);
    assert_eq!(back["video"], before["video"], "picture restored");
    assert_eq!(back["audio"], before["audio"], "sound restored");
}

#[test]
fn skipping_the_preview_or_plan_mode_changes_nothing() {
    let ed = SessionEditor::talking_head("skip");
    let before = ed.run("sequence.inspect", json!({}));
    let plan = json!({"summary": "Cut the slow start", "operations": [{"op": "cut", "ranges": [[0, 1.5]]}]});
    let (out, preview) = run_reviewed(&ed, "edit_timeline", plan.clone(), Mode::AutoEdits, false);
    assert!(!out.ok && out.text.contains("chose not to apply"), "{}", out.text);
    assert!(preview.is_some());
    assert_eq!(ed.run("sequence.inspect", json!({}))["video"], before["video"]);
    // Plan mode: the plan comes back as text to propose; nothing is touched
    let (out, preview) = run_reviewed(&ed, "edit_timeline", plan, Mode::Plan, true);
    assert!(out.text.contains("This plan would") && out.text.contains("Cut 1 range (1.5 s)"), "{}", out.text);
    assert!(preview.is_none());
    assert_eq!(ed.run("sequence.inspect", json!({}))["audio"], before["audio"]);
}

#[test]
fn a_highlight_with_captions_music_and_levels_lands_on_the_timeline() {
    let ed = SessionEditor::talking_head("highlight");
    // speech recognition on this computer, faked with a fixed transcript of the recording
    let w = |text: &str, a: f64, b: f64| filmcraft_engine::project::Word {
        text: text.into(),
        start: filmcraft_engine::time::Tick::from_seconds_f64(a),
        end: filmcraft_engine::time::Tick::from_seconds_f64(b),
        speaker: None,
        confidence: 0.9,
    };
    let words =
        vec![w("Hi,", 0.1, 0.6), w("I'm", 0.7, 1.0), w("Ansh.", 1.1, 1.9), w("Today", 3.6, 4.2), w("we", 4.3, 4.6), w("edit.", 4.7, 6.3), w("Bye.", 6.7, 7.5)];
    lock(&ed.s).transcriber = Some(std::sync::Arc::new(filmcraft_speech::FixedTranscriber {
        transcript: filmcraft_engine::project::Transcript { language: "en".into(), source: String::new(), speakers: vec![], words },
        id: "fixed".into(),
    }));
    let out = run_tool(&ed, "read_footage", json!({"what": ["transcript"]}));
    assert!(out.ok, "{}", out.text);
    assert!(out.text.contains("transcribed just now") && out.text.contains("Hi, I'm Ansh.") && out.text.contains("Today we edit."), "{}", out.text);
    // music the person imported (a steady bed)
    let song = ed.dir.join("song.wav");
    write_wav(&song, &[(12.0, true)]);
    let music = ed.run("file.import", json!({"paths": [song.to_string_lossy()]}))["items"][0].as_u64().unwrap_or(0);
    let tracks = ed.run("sequence.inspect", json!({}))["audio"].as_array().map_or(0, Vec::len);
    let (out, _) = run_reviewed(
        &ed,
        "edit_timeline",
        json!({"summary": "A 5-second highlight", "operations": [
            {"op": "keep", "ranges": [[0.0, 2.0], [3.5, 6.5]]},
            {"op": "transition", "clips": "all", "frames": 6},
            {"op": "volume", "track": "A1", "db": -3},
            {"op": "music", "item": music, "db": -18, "duck": true},
            {"op": "captions"}
        ]}),
        Mode::AutoEdits,
        true,
    );
    assert!(out.ok, "{}", out.text);
    assert!((ed.duration() - 5.0).abs() < 0.05, "kept 2 s + 3 s: {}\n{}", ed.duration(), out.text);
    let seq = ed.run("sequence.inspect", json!({}));
    // the cut between the two moments got the dissolve
    let v1 = &seq["video"][0];
    assert!(v1["transitions"].as_array().is_some_and(|t| !t.is_empty()), "a transition at the cut: {v1}");
    // the speech is at -3 dB
    assert!(seq["audio"][0]["items"].as_array().into_iter().flatten().all(|c| (c["gainDb"].as_f64().unwrap_or(0.0) + 3.0).abs() < 1e-6), "{}", seq["audio"][0]);
    // the music sits on a new track of its own, cut where the picture ends, at -18 dB
    let a = seq["audio"].as_array().cloned().unwrap_or_default();
    assert_eq!(a.len(), tracks + 1, "a music track was added");
    let bed: Vec<&Value> = a.last().and_then(|t| t["items"].as_array()).into_iter().flatten().collect();
    assert_eq!(bed.len(), 1, "one music clip: {bed:?}");
    assert!((bed[0]["end"].as_f64().unwrap_or(0.0) / T - 5.0).abs() < 0.05, "ends with the picture: {}", bed[0]);
    assert!((bed[0]["gainDb"].as_f64().unwrap_or(0.0) + 18.0).abs() < 1e-6);
    // captions from what is said, timed to the edit
    let caps = ed.run("captions.list", json!({}));
    assert!(caps.to_string().contains("Ansh") && caps.to_string().contains("Today"), "{caps}");
}

#[test]
fn trim_order_and_delete_reshape_the_demo_edit() {
    let ed = SessionEditor::demo("reshape");
    let v = ed.clips(0);
    assert!(v.len() >= 3, "the demo has clips: {v:?}");
    let (a, b, c) = (v[0], v[1], v[2]);
    let len_b = b.3 - b.2;
    let (out, _) = run_reviewed(
        &ed,
        "edit_timeline",
        json!({"summary": "Tighten", "operations": [{"op": "trim", "clip": b.0, "head_seconds": 1.0}]}),
        Mode::AutoEdits,
        true,
    );
    assert!(out.ok, "{}", out.text);
    let after = ed.clips(0);
    // what's left of b starts where b started (the result names its id if it changed)
    let nb = after.iter().find(|x| (x.2 - b.2).abs() < 0.05).copied().unwrap_or_else(|| panic!("clip b is still there: {after:?}"));
    assert!(((nb.3 - nb.2) - (len_b - 1.0)).abs() < 0.05, "a second off its start: {nb:?}");
    assert!(nb.0 == b.0 || out.text.contains(&format!("now clip {}", nb.0)), "the model is told the new id: {}", out.text);
    let b = nb;
    let nc = after.iter().find(|x| x.0 == c.0).copied().unwrap_or_else(|| panic!("clip c"));
    assert!((nc.2 - (c.2 - 1.0)).abs() < 0.05, "and the clips after it moved up: {nc:?}");
    // swap the first two
    let (out, _) = run_reviewed(&ed, "edit_timeline", json!({"summary": "Swap", "operations": [{"op": "order", "clips": [b.0, a.0]}]}), Mode::AutoEdits, true);
    assert!(out.ok, "{}", out.text);
    let after = ed.clips(0);
    let na = after.iter().find(|x| x.0 == a.0).copied().unwrap_or(a);
    let nb = after.iter().find(|x| x.0 == b.0).copied().unwrap_or(b);
    assert!(nb.2 < na.2 && (na.2 - nb.3).abs() < 0.05 && nb.2.abs() < 0.05, "b first, then a, back to back: {after:?}");
    // remove the third, closing the gap
    let n = ed.clips(0).len();
    let (out, _) = run_reviewed(&ed, "edit_timeline", json!({"summary": "Drop one", "operations": [{"op": "delete", "clips": [c.0]}]}), Mode::AutoEdits, true);
    assert!(out.ok, "{}", out.text);
    let after = ed.clips(0);
    assert_eq!(after.len(), n - 1);
    let mut sorted = after.clone();
    sorted.sort_by(|x, y| x.2.total_cmp(&y.2));
    for w in sorted.windows(2) {
        assert!(w[1].2 - w[0].3 < 0.05, "no gap left: {sorted:?}");
    }
}

/// Real camera-like files (H.264 picture, AAC sound, decoded by JBD's own decoders), when they
/// are there. Make them with ffmpeg as the release checklist says (docs/jbd/J2-DIRECTOR.md,
/// "Testing on real clips"); media is never committed, so without them this test only notes it.
#[test]
fn real_clips_are_read_cut_in_sync_and_undone() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fixtures/director");
    let (a, b, music) = (dir.join("talk-a.mp4"), dir.join("talk-b.mp4"), dir.join("beat-120.m4a"));
    if !(a.exists() && b.exists() && music.exists()) {
        eprintln!("no real clips in {}: skipped", dir.display());
        return;
    }
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "Real", "width": 1280, "height": 720, "fps": 30.0})).unwrap_or_else(|e| panic!("{e}"));
    let tmp = std::env::temp_dir().join(format!("jbd-engine-test-real-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let ed = SessionEditor { s: Mutex::new(s), dir: tmp };
    let items = ed.run("file.import", json!({"paths": [a.to_string_lossy(), b.to_string_lossy(), music.to_string_lossy()]}));
    let ids: Vec<u64> = items["items"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect();
    assert_eq!(ids.len(), 3, "{items}");
    // the person's clips, back to back, picture and sound linked
    let out = run_tool(&ed, "assemble", json!({"clips": [{"item": ids[0]}, {"item": ids[1]}], "start_at_seconds": 0.0}));
    assert!(out.ok, "{}", out.text);
    assert!((ed.duration() - 16.5).abs() < 0.1, "9.5 s + 7 s: {}", ed.duration());
    // A: talk 0-2, 3.5-6.5, 6.6-7.6, quiet to 9.5. B (at 9.5): quiet 0-1.2, talk to 3, quiet 3-4.4, talk to 7
    let s = silences(&ed);
    let near = |x: f64, y: f64| s.iter().any(|(p, q)| (p - x).abs() < 0.12 && (q - y).abs() < 0.12);
    assert!(near(2.12, 3.38) && near(7.72, 10.58) && near(12.62, 13.78), "pauses in real AAC audio: {s:?}");
    // the music: a steady 120 bpm
    let placed = ed.run("sequence.addTracks", json!({"video": 0, "audio": 1}));
    let _ = placed;
    let m = ed.run("timeline.place", json!({"item": ids[2], "seconds": 0.0, "track": "A2"}));
    let mclip = m["clips"][0].as_u64().unwrap_or(0);
    let beats = ed.call("jbd.analyze", json!({"what": ["beats"], "clips": [mclip]})).unwrap_or_else(|e| panic!("{e}"));
    let bpm = beats["beats"][0]["bpm"].as_f64().unwrap_or(0.0);
    assert!((bpm - 120.0).abs() < 3.0 || (bpm - 240.0).abs() < 6.0 || (bpm - 60.0).abs() < 2.0, "tempo of the music: {beats}");
    ed.run("edit.clear", json!({"clips": [mclip]}));
    // cut the pauses through the Director's own tool, then undo it all
    let before = ed.run("sequence.inspect", json!({}));
    let depth = ed.undo_depth();
    let ranges: Vec<Value> = s.iter().map(|(p, q)| json!([p, q])).collect();
    let removed: f64 = s.iter().map(|(p, q)| q - p).sum();
    let (out, _) =
        run_reviewed(&ed, "edit_timeline", json!({"summary": "Cut the pauses", "operations": [{"op": "cut", "ranges": ranges}]}), Mode::AutoEdits, true);
    assert!(out.ok, "{}", out.text);
    assert!((ed.duration() - (16.5 - removed)).abs() < 0.1, "{} vs {}", ed.duration(), 16.5 - removed);
    let (v, au) = (ed.clips(0), ed.audio(0));
    assert_eq!(v.len(), au.len(), "every picture piece has its sound: {v:?} {au:?}");
    for (pv, pa) in v.iter().zip(&au) {
        assert!((pv.2 - pa.1).abs() < 1e-6 && (pv.3 - pa.2).abs() < 1e-6, "in sync: {v:?} {au:?}");
    }
    for _ in 0..ed.undo_depth() - depth {
        lock(&ed.s).undo();
    }
    let back = ed.run("sequence.inspect", json!({}));
    assert_eq!(back["video"], before["video"]);
    assert_eq!(back["audio"], before["audio"]);
}
