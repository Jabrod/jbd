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

/// The Director's prompt box sits at the bottom of its tab, whatever is above it: an empty
/// conversation, or a long plan opened out. (It once rendered at the top of the tab, leaving the
/// conversation no room.) Also saves pictures to target/tmp/ui-director-*.png to look at.
#[test]
fn the_director_prompt_box_sits_at_the_bottom_of_its_tab() {
    let mut d = Driver::new();
    d.ok("ui.set", json!({"mode": "Edit"}));
    d.frames(3);
    d.ok("ui.panel.show", json!({"panel": "Director"}));
    d.frames(6);
    let check = |d: &mut Driver, name: &str| {
        let els = d.ok("ui.elements", json!({"prefix": ""}));
        let rect = |id: &str| -> [f64; 4] {
            let e = els.as_array().and_then(|a| a.iter().find(|e| e["id"] == id)).unwrap_or_else(|| {
                panic!(
                    "no element {id}; have {:?}",
                    els.as_array().map(|a| a
                        .iter()
                        .filter_map(|e| e["id"].as_str())
                        .filter(|i| i.starts_with("panel") || i.starts_with("director"))
                        .collect::<Vec<_>>())
                )
            });
            let r = &e["rect"];
            [r[0].as_f64().unwrap(), r[1].as_f64().unwrap(), r[2].as_f64().unwrap(), r[3].as_f64().unwrap()]
        };
        let panel = rect("panel.Director");
        let input = rect("director.input");
        let panel_bottom = panel[1] + panel[3];
        let input_bottom = input[1] + input[3];
        assert!(panel_bottom - input_bottom < 90.0, "{name}: the prompt box ends {:.0} pt above the bottom of the tab", panel_bottom - input_bottom);
        assert!(input[1] > panel[1] + panel[3] * 0.4, "{name}: the prompt box is in the top part of the tab");
        if let Ok(img) = render(&mut d.harness) {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
            let _ = std::fs::create_dir_all(&dir);
            let ppp = img.width() as f64 / 1600.0;
            let (x, y, w, h) = ((panel[0] * ppp) as u32, (panel[1] * ppp) as u32, (panel[2] * ppp) as u32, (panel[3] * ppp) as u32);
            let w = w.min(img.width().saturating_sub(x));
            let h = h.min(img.height().saturating_sub(y));
            let _ = image::imageops::crop_imm(&img, x, y, w, h).to_image().save(dir.join(format!("ui-director-{name}.png")));
        }
    };
    check(&mut d, "empty");

    // a seven-step plan, opened out
    {
        let app = d.harness.state_mut();
        let mut s = jbd_director::lock(&app.jbd.director.shared);
        s.todos = (1..=7)
            .map(|i| jbd_director::Todo {
                text: format!("Generate shot {i}"),
                state: if i == 1 { jbd_director::TodoState::Doing } else { jbd_director::TodoState::Todo },
            })
            .collect();
        s.entries.push(jbd_director::Entry::User("Make a 30-second ad".into()));
        s.entries.push(jbd_director::Entry::Said("Plan: **four shots** at 9:16, with titles for the hook and the end card, which is a long line that has to wrap inside the panel instead of running off its edge.".into()));
        drop(s);
        app.jbd.director.plan_open = true;
    }
    d.frames(6);
    check(&mut d, "plan-open");
}

/// Steps fold into one line per run ("Worked · 3 steps"), and Director view gives the Director
/// half the window with no timeline. Saves target/tmp/ui-director-{steps,view}.png.
#[test]
fn director_steps_fold_into_one_line_and_director_view_hides_the_timeline() {
    let mut d = Driver::new();
    d.ok("ui.set", json!({"mode": "Edit"}));
    d.frames(3);
    d.ok("ui.panel.show", json!({"panel": "Director"}));
    {
        let app = d.harness.state_mut();
        let mut s = jbd_director::lock(&app.jbd.director.shared);
        s.entries.push(jbd_director::Entry::User("Tighten this to 15 seconds".into()));
        for (i, title) in ["Read the sequence", "timeline.razor seconds 4", "Assemble 5 clips"].iter().enumerate() {
            s.entries.push(jbd_director::Entry::Step(jbd_director::Step {
                id: i as u64 + 1,
                title: (*title).into(),
                detail: String::new(),
                result: String::new(),
                state: jbd_director::StepState::Ok,
                took: 2.0,
                credits: 0,
                progress: None,
                expanded: false,
                started: 0.0,
            }));
        }
        s.entries.push(jbd_director::Entry::Said("Cut it to **15 s**: kept the five strongest moments.".into()));
    }
    d.frames(6);
    let els = d.ok("ui.elements", json!({"prefix": ""}));
    let ids: Vec<String> = els.as_array().map(|a| a.iter().filter_map(|e| e["id"].as_str().map(str::to_string)).collect()).unwrap_or_default();
    assert!(ids.iter().any(|i| i == "director.view"), "the Director view link is missing");
    save_panel(&mut d, &els, "steps");

    d.ok("ui.click", json!({"id": "director.view"}));
    d.frames(8);
    let els = d.ok("ui.elements", json!({"prefix": ""}));
    let ids: Vec<String> = els.as_array().map(|a| a.iter().filter_map(|e| e["id"].as_str().map(str::to_string)).collect()).unwrap_or_default();
    assert!(!ids.iter().any(|i| i == "panel.Timeline"), "Director view should hide the timeline");
    let director_w = els.as_array().and_then(|a| a.iter().find(|e| e["id"] == "panel.Director")).and_then(|e| e["rect"][2].as_f64()).unwrap_or(0.0);
    assert!(director_w > 600.0, "the Director should get about half the window, got {director_w}");
    if let Ok(img) = render(&mut d.harness) {
        let _ = img.save(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/ui-director-view.png"));
    }
    // and back
    d.ok("ui.click", json!({"id": "director.view"}));
    d.frames(8);
    let els = d.ok("ui.elements", json!({"prefix": "panel.Timeline"}));
    assert!(els.as_array().is_some_and(|a| !a.is_empty()), "Back to editor should bring the timeline back");
}

fn save_panel(d: &mut Driver, els: &Value, name: &str) {
    let Some(r) = els.as_array().and_then(|a| a.iter().find(|e| e["id"] == "panel.Director")).map(|e| e["rect"].clone()) else { return };
    if let Ok(img) = render(&mut d.harness) {
        let ppp = img.width() as f64 / 1600.0;
        let v = |i: usize| r[i].as_f64().unwrap_or(0.0) * ppp;
        let (x, y) = (v(0) as u32, v(1) as u32);
        let w = (v(2) as u32).min(img.width().saturating_sub(x));
        let h = (v(3) as u32).min(img.height().saturating_sub(y));
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        let _ = std::fs::create_dir_all(&dir);
        let _ = image::imageops::crop_imm(&img, x, y, w, h).to_image().save(dir.join(format!("ui-director-{name}.png")));
    }
}

/// Settings ▸ Connections and ▸ Connectors open and show their controls. (Nothing is toggled:
/// these pages write to the real JBD data folder.) Saves target/tmp/ui-settings-*.png.
#[test]
fn settings_has_connections_and_connectors_pages() {
    let mut d = Driver::new();
    d.ok("ui.set", json!({"mode": "Edit"}));
    d.frames(3);
    for (section, must) in [("connections", "settings.connections.allow"), ("connectors", "settings.section.connectors")] {
        {
            let app = d.harness.state_mut();
            app.jbd.settings_open = true;
            app.jbd.settings_section = section.to_string();
        }
        d.frames(6);
        let els = d.ok("ui.elements", json!({"prefix": "settings."}));
        let ids: Vec<String> = els.as_array().map(|a| a.iter().filter_map(|e| e["id"].as_str().map(str::to_string)).collect()).unwrap_or_default();
        assert!(ids.iter().any(|i| i == must), "{section}: missing {must}; have {ids:?}");
        assert!(ids.iter().any(|i| i == &format!("settings.section.{section}")), "{section} isn't in the Settings list");
        if let Ok(img) = render(&mut d.harness) {
            let _ = img.save(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/tmp/ui-settings-{section}.png")));
        }
    }
}

/// Tabs close with their × and come back from the +; Director view shows the chats sidebar.
/// Saves target/tmp/ui-director-sidebar.png.
#[test]
fn tabs_close_and_come_back_and_director_view_has_a_chats_sidebar() {
    let mut d = Driver::new();
    d.ok("ui.set", json!({"mode": "Edit"}));
    d.frames(3);
    d.ok("ui.panel.show", json!({"panel": "Director"}));
    d.frames(4);
    let has = |d: &mut Driver, id: &str| {
        let els = d.ok("ui.elements", json!({"prefix": ""}));
        els.as_array().is_some_and(|a| a.iter().any(|e| e["id"] == id))
    };
    // close Transcript (the Text panel) with its × (hover it first: inactive tabs show × on hover)
    d.ok("ui.move", json!({"id": "panel.tab.Text"}));
    d.frames(3);
    assert!(has(&mut d, "panel.close.Text"), "hovered tab shows its ×");
    d.ok("ui.click", json!({"id": "panel.close.Text"}));
    d.frames(3);
    let after = d.ok("ui.elements", json!({"prefix": "panel."}));
    let ids: Vec<String> = after.as_array().map(|a| a.iter().filter_map(|e| e["id"].as_str().map(str::to_string)).collect()).unwrap_or_default();
    assert!(!ids.iter().any(|i| i == "panel.tab.Text"), "Transcript closed; still have {ids:?}");
    // bring it back from the +
    let add = d.ok("ui.elements", json!({"prefix": "panel.add."}));
    let add_id = add.as_array().and_then(|a| a.iter().find_map(|e| e["id"].as_str().map(str::to_string))).unwrap_or_else(|| panic!("a + on the strip"));
    d.ok("ui.click", json!({"id": add_id}));
    d.frames(3);
    assert!(has(&mut d, "panel.addMenu.Text"), "the + menu lists Transcript");
    d.ok("ui.click", json!({"id": "panel.addMenu.Text"}));
    d.frames(3);
    assert!(has(&mut d, "panel.tab.Text"), "Transcript is back");

    // Director view: the sidebar replaces the history button
    d.ok("ui.panel.show", json!({"panel": "Director"}));
    d.frames(3);
    d.ok("ui.click", json!({"id": "director.view"}));
    d.frames(8);
    assert!(has(&mut d, "director.sidebar.new"), "Director view has the chats sidebar");
    assert!(!has(&mut d, "director.history"), "and no separate history button");
    if let Ok(img) = render(&mut d.harness) {
        let _ = img.save(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/ui-director-sidebar.png"));
    }
    // the sidebar button hides it (the history button takes its place) and shows it again
    d.ok("ui.click", json!({"id": "director.sidebar.toggle"}));
    d.frames(4);
    assert!(!has(&mut d, "director.sidebar.new"), "hidden");
    assert!(has(&mut d, "director.history"), "history is still one click away");
    if let Ok(img) = render(&mut d.harness) {
        let _ = img.save(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/ui-director-sidebar-hidden.png"));
    }
    d.ok("ui.click", json!({"id": "director.sidebar.toggle"}));
    d.frames(4);
    assert!(has(&mut d, "director.sidebar.new"), "shown again");
}

/// Offscreen screenshots take turns: the GPU driver can fault when several tests render at the
/// same moment (the app itself renders one window from one thread).
fn render(h: &mut Harness<'static, FilmcraftApp>) -> Result<image::RgbaImage, String> {
    static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _turn = GPU.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    h.render().map_err(|e| e.to_string())
}

/// Selected timeline clips can be attached to the Director as context, like files.
#[test]
fn selected_timeline_clips_attach_to_the_director_as_context() {
    let mut d = Driver::new();
    d.ok("ui.set", json!({"mode": "Edit"}));
    d.frames(3);
    d.ok("ui.panel.show", json!({"panel": "Director"}));
    let seq = d.ok("engine.execute", json!({"command": "sequence.inspect", "params": {}}));
    let clip = seq["video"][0]["items"][1]["clip"].as_u64().unwrap_or_else(|| panic!("a clip"));
    d.ok("engine.execute", json!({"command": "timeline.select", "params": {"clips": [clip], "add": false, "toggle": false}}));
    d.frames(4);
    let els = d.ok("ui.elements", json!({"prefix": "director.addSelection"}));
    assert!(els.as_array().is_some_and(|a| !a.is_empty()), "the + Add selected clips pill shows");
    d.ok("ui.click", json!({"id": "director.addSelection"}));
    d.frames(3);
    let attached: Vec<u64> = d.harness.state().jbd.director.context.iter().map(|c| c.clip).collect();
    assert!(attached.contains(&clip), "the selected clip is attached: {attached:?}");
    // and the pill is gone (nothing new to add)
    let els = d.ok("ui.elements", json!({"prefix": "director.addSelection"}));
    assert!(els.as_array().is_some_and(|a| a.iter().all(|e| e["id"] != "director.addSelection")), "pill hidden once attached");
}

/// A very long prompt (a pasted brief) keeps the prompt box at the bottom and no taller than a
/// third of the tab; long sent messages fold. Saves target/tmp/ui-director-long.png.
#[test]
fn a_long_prompt_never_breaks_the_director() {
    let mut d = Driver::new();
    d.ok("ui.set", json!({"mode": "Edit"}));
    d.frames(3);
    d.ok("ui.panel.show", json!({"panel": "Director"}));
    d.frames(3);
    let brief: String = (1..=120)
        .map(|i| {
            format!(
                "Line {i} of a long brief: hook in the first second, cut on the beat, titles short.
"
            )
        })
        .collect();
    {
        let app = d.harness.state_mut();
        app.jbd.director.input = brief.clone();
        let mut s = jbd_director::lock(&app.jbd.director.shared);
        s.entries.push(jbd_director::Entry::User(brief.clone()));
        s.entries.push(jbd_director::Entry::Said("Got it.".into()));
    }
    d.frames(8);
    let els = d.ok("ui.elements", json!({"prefix": ""}));
    let rect = |id: &str| -> [f64; 4] {
        let e = els.as_array().and_then(|a| a.iter().find(|e| e["id"] == id)).unwrap_or_else(|| panic!("no element {id}"));
        let r = &e["rect"];
        [r[0].as_f64().unwrap_or(0.0), r[1].as_f64().unwrap_or(0.0), r[2].as_f64().unwrap_or(0.0), r[3].as_f64().unwrap_or(0.0)]
    };
    let panel = rect("panel.Director");
    let send = rect("director.send");
    // the send button (bottom row of the box) is still on screen, at the bottom of the tab
    assert!(
        send[1] + send[3] <= panel[1] + panel[3] + 1.0 && panel[1] + panel[3] - (send[1] + send[3]) < 90.0,
        "send button at the bottom: {send:?} in {panel:?}"
    );
    let input = rect("director.input");
    assert!(input[3] <= panel[3] * 0.4, "the text box is capped: {:.0} of {:.0}", input[3], panel[3]);
    assert!(input[1] > panel[1] + panel[3] * 0.4, "the box stays in the lower part of the tab");
    if let Ok(img) = render(&mut d.harness) {
        let _ = img.save(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/ui-director-long.png"));
    }
}

/// Selecting a clip selects its own sound with it (Linked Selection); the Director counts them as
/// one clip.
#[test]
fn a_clip_and_its_own_sound_are_one_context_clip() {
    let mut d = Driver::new();
    d.ok("ui.set", json!({"mode": "Edit"}));
    d.frames(3);
    d.ok("ui.panel.show", json!({"panel": "Director"}));
    let seq = d.ok("engine.execute", json!({"command": "sequence.inspect", "params": {}}));
    let linked = seq["video"][0]["items"].as_array().and_then(|a| a.iter().find(|c| !c["link"].is_null())).cloned();
    let Some(v) = linked else {
        // the demo has no linked clips: link one by hand
        panic!("the demo project needs a clip with its own sound for this test: {seq}");
    };
    let clip = v["clip"].as_u64().unwrap_or(0);
    d.ok("engine.execute", json!({"command": "timeline.select", "params": {"clips": [clip]}}));
    d.frames(3);
    let sel = d.ok("engine.execute", json!({"command": "state.inspect", "params": {}}))["selection"].as_array().map_or(0, Vec::len);
    assert!(sel >= 2, "linked selection picked the sound too ({sel})");
    let refs = filmcraft_ui_egui::jbd::director_run::selected_clips(d.harness.state());
    assert_eq!(refs.len(), 1, "one clip, not two: {refs:?}");
    assert!(refs[0].sound.is_some() && refs[0].label().ends_with("+ sound"), "{refs:?}");
    // Ctrl+Alt+click gives it to the Director in one go, and takes it back
    let id = filmcraft_project::ClipId(clip);
    filmcraft_ui_egui::jbd::director_run::toggle_context_clip(d.harness.state_mut(), id);
    let ctx = d.harness.state().jbd.director.context.clone();
    assert_eq!(ctx.len(), 1, "{ctx:?}");
    assert!(ctx[0].sound.is_some());
    filmcraft_ui_egui::jbd::director_run::toggle_context_clip(d.harness.state_mut(), id);
    assert!(d.harness.state().jbd.director.context.is_empty());
}

/// Clicks on a real timeline clip: a plain click selects it with its own sound; Alt+click selects
/// just the clip, even when it was already selected with its sound; Ctrl+Alt+click gives it to
/// the Director as context.
#[test]
fn alt_click_selects_one_side_and_ctrl_alt_click_gives_it_to_the_director() {
    let mut d = Driver::new();
    d.ok("ui.set", json!({"mode": "Edit"}));
    d.frames(3);
    let seq = d.ok("engine.execute", json!({"command": "sequence.inspect", "params": {}}));
    let v = seq["video"][0]["items"].as_array().and_then(|a| a.iter().find(|c| !c["link"].is_null())).cloned().unwrap_or_else(|| panic!("a linked clip"));
    let clip = v["clip"].as_u64().unwrap_or(0);
    let id = format!("timeline.clip.{clip}");
    let selection = |d: &mut Driver| d.ok("engine.execute", json!({"command": "state.inspect", "params": {}}))["selection"].as_array().map_or(0, Vec::len);
    d.ok("ui.click", json!({"id": id}));
    d.frames(3);
    assert!(selection(&mut d) >= 2, "a plain click takes the clip's sound too");
    d.ok("ui.click", json!({"id": id, "modifiers": {"alt": true}}));
    d.frames(3);
    let sel = d.ok("engine.execute", json!({"command": "state.inspect", "params": {}}))["selection"].clone();
    assert_eq!(sel, json!([clip]), "Alt+click: just the clip");
    d.ok("ui.click", json!({"id": id, "modifiers": {"alt": true, "ctrl": true, "command": true}}));
    d.frames(3);
    let ctx: Vec<u64> = d.harness.state().jbd.director.context.iter().map(|c| c.clip).collect();
    assert_eq!(ctx, vec![clip], "Ctrl+Alt+click gives it to the Director");
    // right-click another clip (not selected): it is selected, and "Add to Director" adds it
    let other = seq["video"][0]["items"]
        .as_array()
        .and_then(|a| a.iter().find(|c| c["clip"].as_u64() != Some(clip) && !c["link"].is_null()))
        .and_then(|c| c["clip"].as_u64())
        .unwrap_or_else(|| panic!("another clip"));
    d.ok("ui.click", json!({"id": format!("timeline.clip.{other}"), "button": "right"}));
    d.frames(3);
    d.ok("ui.click", json!({"id": "timeline.menu.addToDirector"}));
    d.frames(3);
    let ctx: Vec<u64> = d.harness.state().jbd.director.context.iter().map(|c| c.clip).collect();
    assert_eq!(ctx, vec![clip, other], "right-click ▸ Add to Director");
}

/// The editor in each colour theme, with a clip selected and the Director open: saves
/// target/tmp/ui-theme-{darkest,dark,light}.png to look at.
#[test]
fn every_theme_renders_the_whole_editor() {
    let mut d = Driver::new();
    d.ok("ui.set", json!({"mode": "Edit"}));
    d.frames(3);
    d.ok("ui.panel.show", json!({"panel": "Director"}));
    let seq = d.ok("engine.execute", json!({"command": "sequence.inspect", "params": {}}));
    let clip = seq["video"][0]["items"][1]["clip"].as_u64().unwrap_or(0);
    d.ok("engine.execute", json!({"command": "timeline.select", "params": {"clips": [clip]}}));
    {
        let app = d.harness.state_mut();
        let mut s = jbd_director::lock(&app.jbd.director.shared);
        s.entries.push(jbd_director::Entry::User("Make it punchier".into()));
        s.entries.push(jbd_director::Entry::Said("Tightened the cuts and added a **zoom-in** on the hero shot.".into()));
    }
    for theme in ["darkest", "dark", "light"] {
        d.ok("engine.execute", json!({"command": "prefs.set", "params": {"key": "appearance.colorTheme", "value": theme}}));
        d.frames(8);
        if let Ok(img) = render(&mut d.harness) {
            let _ = img.save(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/tmp/ui-theme-{theme}.png")));
        }
    }
}

/// A fresh Director: one click in the empty prompt box and typing works (the box used to need a
/// click on + first).
#[test]
fn the_empty_prompt_box_takes_a_click_and_typing() {
    let mut d = Driver::new();
    d.ok("ui.set", json!({"mode": "Edit"}));
    d.frames(3);
    d.ok("ui.panel.show", json!({"panel": "Director"}));
    d.frames(4);
    d.ok("ui.click", json!({"id": "director.input", "fx": 0.3, "fy": 0.5}));
    d.frames(3);
    d.ok("ui.type", json!({"text": "cut at 4 s"}));
    d.frames(3);
    assert_eq!(d.harness.state().jbd.director.input, "cut at 4 s");
}

/// While the Director works: the cube, an editor's word with the step count, and the newest
/// tool calls listed under it. Saves target/tmp/ui-director-live.png.
#[test]
fn a_running_request_shows_its_tool_calls_live() {
    let mut d = Driver::new();
    d.ok("ui.set", json!({"mode": "Edit"}));
    d.frames(3);
    d.ok("ui.panel.show", json!({"panel": "Director"}));
    {
        let app = d.harness.state_mut();
        let mut s = jbd_director::lock(&app.jbd.director.shared);
        s.entries.push(jbd_director::Entry::User("Put the logo on the ball".into()));
        let steps = [
            ("Looked at clip 53", jbd_director::StepState::Ok),
            ("Searched commands: mask", jbd_director::StepState::Ok),
            ("Put the overlay on the shot (clip 53)", jbd_director::StepState::Running),
            ("Read the sequence", jbd_director::StepState::Ok),
        ];
        for (i, (title, state)) in steps.iter().enumerate() {
            s.entries.push(jbd_director::Entry::Step(jbd_director::Step {
                id: i as u64 + 1,
                title: (*title).into(),
                detail: String::new(),
                result: String::new(),
                state: state.clone(),
                took: 1.5,
                credits: 0,
                progress: None,
                expanded: false,
                started: 0.0,
            }));
        }
        // the running one last, as it really is
        s.entries.swap(3, 4);
        s.busy = true;
        s.activity = Some("Thinking…".into());
    }
    d.frames(6);
    let els = d.ok("ui.elements", json!({"prefix": "panel.Director"}));
    if let (Ok(img), Some(r)) = (render(&mut d.harness), els.as_array().and_then(|a| a.first()).map(|e| e["rect"].clone())) {
        let ppp = img.width() as f64 / 1600.0;
        let (x, y, w, h) = (
            (r[0].as_f64().unwrap_or(0.0) * ppp) as u32,
            (r[1].as_f64().unwrap_or(0.0) * ppp) as u32,
            (r[2].as_f64().unwrap_or(0.0) * ppp) as u32,
            (r[3].as_f64().unwrap_or(0.0) * ppp * 0.45) as u32,
        );
        let _ = image::imageops::crop_imm(&img, x, y, w.min(img.width() - x), h.min(img.height() - y))
            .to_image()
            .save(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/ui-director-live.png"));
    }
}

/// Help ▸ What's New / Check for Updates / Report a Problem, and the title bar's Update chip
/// when jabrod.com has a newer JBD. Saves target/tmp/ui-update-{chip,report}.png.
#[test]
fn updates_whats_new_and_report_a_problem() {
    let mut d = Driver::new();
    d.ok("ui.set", json!({"mode": "Edit"}));
    d.frames(3);
    let has = |d: &mut Driver, id: &str| d.ok("ui.elements", json!({"prefix": id})).as_array().is_some_and(|a| a.iter().any(|e| e["id"] == id));
    // no newer version: no chip
    assert!(!has(&mut d, "titlebar.update"));
    // jabrod.com announces a newer one
    d.harness.state_mut().jbd.updates.offer(jbd_account::Release {
        version: "99.0.0".into(),
        title: "JBD 99".into(),
        notes: vec!["Everything is faster".into()],
        download_url: "https://jabrod.com/downloads/JBD-99.0.0-setup.exe".into(),
        page_url: "https://jabrod.com/jbd".into(),
    });
    d.frames(3);
    assert!(has(&mut d, "titlebar.update"), "the Update chip shows");
    d.ok("ui.click", json!({"id": "titlebar.update"}));
    d.frames(3);
    assert!(has(&mut d, "updates.install"), "the update window offers the download");
    if let Ok(img) = render(&mut d.harness) {
        let _ = img.save(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/ui-update-chip.png"));
    }
    // What's New: the bundled changelog
    d.harness.state_mut().jbd.updates.dialog = Some(filmcraft_ui_egui::jbd::updates::Dialog::WhatsNew);
    d.frames(3);
    assert!(has(&mut d, "updates.changelogOnline"));
    // Report a Problem: Send waits for a real message
    d.harness.state_mut().jbd.updates.dialog = Some(filmcraft_ui_egui::jbd::updates::Dialog::Report);
    d.frames(3);
    assert!(has(&mut d, "updates.report.send"));
    d.harness.state_mut().jbd.updates.draft.message = "The preview goes black after I add a title.".into();
    d.frames(3);
    if let Ok(img) = render(&mut d.harness) {
        let _ = img.save(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/ui-update-report.png"));
    }
}

/// The free plan sees what Pro gives in the Director instead of the chat. Saves
/// target/tmp/ui-director-prowall.png.
#[test]
fn the_free_plan_sees_the_pro_wall_in_the_director() {
    let mut session = Session::default();
    session.execute("file.openDemoProject", json!({})).expect("demo project");
    let (tx, rx) = channel();
    let app = FilmcraftApp::new(session).with_account(Arc::new(DemoAccount::free("Test", "test@example.com")), false).with_control(rx);
    let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
    let mut d = Driver { harness, tx };
    d.frames(4);
    d.ok("ui.set", json!({"mode": "Edit"}));
    d.frames(3);
    d.ok("ui.panel.show", json!({"panel": "Director"}));
    d.frames(4);
    let ids: Vec<String> = d
        .ok("ui.elements", json!({"prefix": "director."}))
        .as_array()
        .map(|a| a.iter().filter_map(|e| e["id"].as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    assert!(ids.iter().any(|i| i == "director.upgrade"), "the Upgrade button shows: {ids:?}");
    assert!(!ids.iter().any(|i| i == "director.input"), "no prompt box on the free plan: {ids:?}");
    if let Ok(img) = render(&mut d.harness) {
        let _ = img.save(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/ui-director-prowall.png"));
    }
}

/// Settings ▸ Usage with pay-as-you-go on. Saves target/tmp/ui-settings-usage.png.
#[test]
fn settings_usage_shows_credits_and_pay_as_you_go() {
    let mut d = Driver::new();
    d.ok("ui.set", json!({"mode": "Edit"}));
    d.frames(3);
    let report: jbd_account::UsageReport = serde_json::from_value(json!({
        "plan": "pro",
        "credits": {"used": 940, "limit": 1000},
        "byKind": [{"kind": "Video", "credits": 610, "calls": 9}, {"kind": "Thinking", "credits": 280, "calls": 212}, {"kind": "Voiceover", "credits": 50, "calls": 4}],
        "byModel": [{"model": "google/veo-3.1-lite", "kind": "Video", "credits": 610, "calls": 9}, {"model": "anthropic/claude-sonnet-5.5", "kind": "Thinking", "credits": 280, "calls": 212}],
        "days": [{"day": "2026-10-02", "credits": 120}, {"day": "2026-10-09", "credits": 410}, {"day": "2026-10-15", "credits": 410}],
        "payg": {"status": "active", "paused": false, "currency": "INR", "pricePerCredit": 150, "cap": 100000, "used": 120, "spent": 18000},
        "urls": {"upgrade": "https://jabrod.com/jbd/upgrade", "payg": "https://jabrod.com/jbd/payg"}
    }))
    .unwrap_or_default();
    {
        let app = d.harness.state_mut();
        app.jbd.ai_usage.preload(report);
        app.jbd.settings_open = true;
        app.jbd.settings_section = "usage".into();
    }
    d.frames(6);
    let ids: Vec<String> = d
        .ok("ui.elements", json!({"prefix": "usage."}))
        .as_array()
        .map(|a| a.iter().filter_map(|e| e["id"].as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    assert!(ids.iter().any(|i| i == "usage.payg.pause"), "pause shows: {ids:?}");
    assert!(ids.iter().any(|i| i == "usage.payg.cap.100000"), "the limit presets show: {ids:?}");
    if let Ok(img) = render(&mut d.harness) {
        let _ = img.save(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/ui-settings-usage.png"));
    }
}
