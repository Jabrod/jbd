//! Headless UI tests of JBD's layer (signed in with the demo account, no network), for bugs users
//! hit in the first 1.0 build:
//!
//! - a project with no open timeline made Text / Shape / Motion fail ("no sequence is open");
//! - double-clicking media opened a Source monitor JBD's layout doesn't have, so nothing showed;
//! - File ▸ New Project made a project without a timeline;
//! - Close Project stayed in the editor (a new timeline was made at once);
//! - the menus showed Ctrl+N / Ctrl+, for New Project / Settings, but those keys ran New
//!   Timeline / the full preferences.

use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use jbd_account::DemoAccount;
use serde_json::{Value, json};

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
}

impl Driver {
    fn new() -> Self {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).expect("demo project");
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(session).with_account(Arc::new(DemoAccount::new("Test", "test@example.com")), false).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx };
        d.frames(4);
        d
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            // feed the control channel's synthetic input (clicks, keys) as eframe would
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut raw);
            *self.harness.input_mut() = raw;
            self.harness.step();
        }
    }

    fn ok(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                assert_eq!(v["ok"], json!(true), "{method} {params} failed: {v}");
                return v["result"].clone();
            }
        }
        panic!("no reply to {method} {params}");
    }

    fn active_sequence(&mut self) -> Value {
        self.ok("ui.inspect", json!({}))["activeSequence"].clone()
    }
}

#[test]
fn a_project_without_a_timeline_gets_one_and_text_works() {
    let mut d = Driver::new();
    d.ok("ui.set", json!({"mode": "Edit"}));
    // File ▸ New Project's engine command leaves no timeline open…
    d.ok("engine.execute", json!({"command": "file.newProject", "params": {"name": "Untitled"}}));
    d.frames(3);
    // …JBD opens one at once
    assert!(d.active_sequence().is_u64(), "JBD should open a timeline");
    // and the Media toolbar's Text button works
    d.frames(3);
    d.ok("ui.click", json!({"id": "media.text"}));
    d.frames(3);
    let status = d.ok("ui.inspect", json!({}))["ui"]["status"].as_str().unwrap_or("").to_string();
    assert!(!status.contains("not available"), "Text failed: {status}");
}

#[test]
fn double_clicking_media_adds_it_to_the_timeline() {
    let mut d = Driver::new();
    d.ok("ui.set", json!({"mode": "Edit"}));
    d.ok("engine.execute", json!({"command": "file.newProject", "params": {"name": "Untitled"}}));
    d.frames(3);
    let seq = d.active_sequence();
    let items = d.ok("engine.execute", json!({"command": "file.importDemoFootage", "params": {"scene": "OceanSunset"}}));
    let item = items["items"].as_array().and_then(|a| a.first()).and_then(Value::as_u64).or_else(|| items["item"].as_u64()).expect("an item");
    d.frames(3);
    // what the Media panel's double-click does in JBD
    let ctx = d.harness.ctx.clone();
    let app = d.harness.state_mut();
    filmcraft_ui_egui::jbd::add_to_timeline(app, item).expect("added");
    let _ = ctx;
    d.frames(3);
    assert_eq!(d.active_sequence(), seq);
    let clips = d.ok("engine.execute", json!({"command": "sequence.inspect", "params": {}}));
    let text = clips.to_string();
    assert!(text.contains(&format!("\"item\":{item}")), "the clip should be on the timeline: {text}");
}

impl Driver {
    fn mode(&self) -> filmcraft_ui_egui::state::Mode {
        self.harness.state().ui.mode
    }

    /// Answer JBD's "Save changes?" with Don't save, if it is showing.
    fn discard_if_asked(&mut self) {
        self.frames(2);
        if self.harness.state().jbd.welcome_pending.is_some() {
            self.ok("ui.click", json!({"id": "welcome.confirm.discard"}));
            self.frames(3);
        }
    }
}

#[test]
fn close_project_shows_the_start_screen_without_a_new_timeline() {
    use filmcraft_ui_egui::state::Mode;
    let mut d = Driver::new();
    d.ok("ui.set", json!({"mode": "Edit"}));
    d.frames(3);
    // the shortcut the menu shows (Ctrl+Shift+W; Ctrl is "Cmd" on Windows)
    d.ok("ui.key", json!({"key": "Cmd+Shift+W"}));
    d.discard_if_asked();
    d.frames(3);
    assert_eq!(d.mode(), Mode::Import, "Close Project should show the start screen");
    assert!(d.active_sequence().is_null(), "no timeline should be made on the start screen");
    assert!(d.harness.state().session.project.items.is_empty(), "the project should be closed");
}

#[test]
fn the_shortcuts_jbd_menus_show_do_what_they_say() {
    use filmcraft_ui_egui::state::Mode;
    let mut d = Driver::new();
    d.ok("ui.set", json!({"mode": "Edit"}));
    d.frames(3);
    let timelines =
        |d: &Driver| d.harness.state().session.project.items.values().filter(|it| matches!(it.kind, filmcraft_engine::project::ItemKind::Sequence(_))).count();
    let before = timelines(&d);
    // Ctrl+N: New Project… (the start screen), not a new timeline
    d.ok("ui.key", json!({"key": "Cmd+N"}));
    d.frames(3);
    assert_eq!(d.mode(), Mode::Import);
    assert_eq!(timelines(&d), before, "Ctrl+N must not add a timeline");
    // Ctrl+,: JBD Settings
    d.ok("ui.key", json!({"key": "Cmd+,"}));
    d.frames(3);
    assert!(d.harness.state().jbd.settings_open, "Ctrl+, should open Settings");
}
