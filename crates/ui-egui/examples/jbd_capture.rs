//! Renders JBD's real screens offscreen at high resolution: the launch animation frame by frame,
//! and stills of the start screen, sign-in, editor, account menu, Director and Export. Used for the
//! website, the Microsoft Store listing and the launch video (jbd-media).
//!
//!   cargo run --release -p filmcraft-ui-egui --example jbd_capture -- <out dir> [pixels per point]
//!
//! The UI is laid out at 1920×1080 points and drawn at `pixels per point` (default 2: 3840×2160).
//! It uses the signed-in demo account (no network) and the built-in demo project.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::dock::PanelKind;
use filmcraft_ui_egui::state::Mode;
use jbd_account::DemoAccount;
use serde_json::json;

const SIZE: egui::Vec2 = egui::vec2(1920.0, 1080.0);
const FPS: f32 = 60.0;

fn harness(app: FilmcraftApp, ppp: f32) -> Harness<'static, FilmcraftApp> {
    Harness::builder().with_size(SIZE).with_pixels_per_point(ppp).with_step_dt(1.0 / FPS).with_max_steps(100_000).wgpu().build_eframe(move |_cc| app)
}

fn save(h: &mut Harness<'static, FilmcraftApp>, path: &Path) {
    match h.render() {
        Ok(img) => {
            if let Err(e) = img.save(path) {
                eprintln!("{}: {e}", path.display());
            } else {
                println!("{}", path.display());
            }
        }
        Err(e) => eprintln!("render {}: {e}", path.display()),
    }
}

/// Run frames with real time in between, so background work (decoding, thumbnails) finishes.
fn settle(h: &mut Harness<'static, FilmcraftApp>, frames: usize) {
    for _ in 0..frames {
        h.step();
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn demo_app(splash: bool, signed_in: bool) -> FilmcraftApp {
    let mut session = Session::default();
    let _ = session.execute("file.openDemoProject", json!({}));
    let account = DemoAccount::new("Riya Sharma", "riya@example.com");
    if !signed_in {
        jbd_account::AccountBackend::sign_out(&account);
    }
    FilmcraftApp::new(session).with_account(Arc::new(account), splash)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| "target/jbd-capture".into()));
    let ppp: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2.0);
    let only: Option<String> = args.get(3).cloned();
    let want = |name: &str| only.as_deref().is_none_or(|o| o.split(',').any(|x| x == name));
    let _ = std::fs::create_dir_all(&out);

    // 1. the launch animation, every frame for 2.6 s
    if want("splash") {
        let dir = out.join("splash");
        let _ = std::fs::create_dir_all(&dir);
        let mut h = harness(demo_app(true, true), ppp);
        // Building the harness runs the app until it settles, which plays the whole splash. Start
        // it again, and drive the clock explicitly, 1/60 s a frame (rendering a 4K frame takes far
        // longer than that in real time). The splash clock starts on its first drawn frame.
        const T0: f64 = 1000.0;
        h.state_mut().jbd.splash = Some(filmcraft_ui_egui::jbd::splash::Splash::default());
        let frames = (2.6 * FPS) as usize;
        for i in 0..frames {
            h.input_mut().time = Some(T0 + i as f64 / FPS as f64);
            h.step();
            save(&mut h, &dir.join(format!("splash_{i:04}.png")));
        }
    }

    // 2. sign-in
    if want("signin") {
        let mut h = harness(demo_app(false, false), ppp);
        settle(&mut h, 8);
        save(&mut h, &out.join("signin.png"));
    }

    // 3. the start screen
    if want("welcome") {
        let mut h = harness(demo_app(false, true), ppp);
        h.state_mut().ui.mode = Mode::Import;
        settle(&mut h, 8);
        save(&mut h, &out.join("welcome.png"));
    }

    // 4. the editor, then its other views
    if want("editor") || want("account") || want("director") || want("export") || want("transcript") {
        let mut h = harness(demo_app(false, true), ppp);
        h.state_mut().ui.mode = Mode::Edit;
        settle(&mut h, 60);
        if want("editor") {
            save(&mut h, &out.join("editor.png"));
        }
        if want("account") {
            h.state_mut().jbd.menu_open = true;
            settle(&mut h, 6);
            save(&mut h, &out.join("account.png"));
            h.state_mut().jbd.menu_open = false;
        }
        if want("director") {
            h.state_mut().ui.dock.activate(PanelKind::Director);
            settle(&mut h, 6);
            save(&mut h, &out.join("director.png"));
        }
        if want("transcript") {
            h.state_mut().ui.dock.activate(PanelKind::Text);
            settle(&mut h, 6);
            save(&mut h, &out.join("transcript.png"));
            h.state_mut().ui.dock.activate(PanelKind::Properties);
        }
        if want("export") {
            h.state_mut().ui.mode = Mode::Export;
            settle(&mut h, 30);
            save(&mut h, &out.join("export.png"));
        }
    }

    // 5. more of the editor for the launch films: a title template, colour, effects, the mixer
    if want("graphics") || want("color") || want("effects") || want("mixer") {
        let mut h = harness(demo_app(false, true), ppp);
        h.state_mut().ui.mode = Mode::Edit;
        settle(&mut h, 40);
        let app = h.state_mut();
        let _ = app.session.execute("edit.selectAll", json!({}));
        if want("graphics") {
            let app = h.state_mut();
            let _ = app.session.execute("graphics.template.apply", json!({"template": "builtin:lower-third-slab"}));
            app.ui.dock.activate(PanelKind::EssentialGraphics);
            settle(&mut h, 30);
            save(&mut h, &out.join("graphics.png"));
        }
        if want("color") {
            h.state_mut().show_panel(PanelKind::LumetriColor);
            settle(&mut h, 30);
            save(&mut h, &out.join("color.png"));
        }
        if want("effects") {
            h.state_mut().show_panel(PanelKind::Effects);
            settle(&mut h, 20);
            save(&mut h, &out.join("effects.png"));
        }
        if want("mixer") {
            h.state_mut().show_panel(PanelKind::AudioTrackMixer);
            settle(&mut h, 20);
            save(&mut h, &out.join("mixer.png"));
        }
    }

    // 6. a vertical (9:16) project with a title, as made from the start screen's Vertical template
    if want("vertical") {
        let mut h = harness(demo_app(false, true), ppp);
        let app = h.state_mut();
        let _ = app.session.execute("file.newProject", json!({"name": "Reel"}));
        let _ = app.session.execute("file.newSequence", json!({"name": "Reel", "width": 1080, "height": 1920, "fps": 30.0}));
        if let Ok(v) = app.session.execute("file.importDemoFootage", json!({"scene": "OceanSunset"})) {
            let item = v["items"].as_array().and_then(|a| a.first()).and_then(serde_json::Value::as_u64).or_else(|| v["item"].as_u64());
            if let Some(item) = item {
                let _ = filmcraft_ui_egui::jbd::add_to_timeline(app, item);
            }
        }
        let app = h.state_mut();
        app.session.set_playhead(filmcraft_time::Tick(0));
        let _ = app.session.execute("graphics.newText", json!({"text": "Golden hour", "newClip": true}));
        app.ui.mode = Mode::Edit;
        app.ui.dock.activate(PanelKind::EssentialGraphics);
        settle(&mut h, 60);
        save(&mut h, &out.join("vertical.png"));
    }
}
