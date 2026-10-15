//! JBD's start screen (the first JBD's Welcome): the logo, blank templates for the four shapes
//! creators post in, New / Open project, the recent projects list, and whether the video engine
//! and the GPU are ready.
//!
//!   ┌ [cube] JBD                         RECENT PROJECTS ┐
//!   │ AI video editor by Jabrod          Untitled 12      │
//!   │ START WITH A BLANK TEMPLATE        C:\…\Untitled 12 │
//!   │ [Landscape][Vertical][Square][Portrait]   …         │
//!   │ [+ New project… Ctrl+N] [Open project…]             │
//!   │ ✓ Video engine ready · …                            │
//!   └─────────────────────────────────────────────────────┘
//!
//! A template makes a new project with one timeline of that shape (30 fps) and opens the editor.
//! If the open project has unsaved changes, JBD asks first.
//!
//! Automation ids: `welcome.template.{landscape,vertical,square,portrait}`, `welcome.new`,
//! `welcome.open`, `welcome.demo`, `welcome.recent.<n>`, `welcome.confirm.{save,discard,cancel}`.

use egui::{Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use serde_json::json;

use super::{cube, style};
use crate::FilmcraftApp;
use crate::state::Mode;
use crate::theme::Tokens;

/// The blank templates: id, name, what it's for, width, height.
pub const TEMPLATES: [(&str, &str, &str, u32, u32); 4] = [
    ("landscape", "Landscape", "YouTube · 16:9", 1920, 1080),
    ("vertical", "Vertical", "Reels · Shorts · 9:16", 1080, 1920),
    ("square", "Square", "Feed post · 1:1", 1080, 1080),
    ("portrait", "Portrait", "Instagram · 4:5", 1080, 1350),
];

/// What waits for the "unsaved changes" answer.
#[derive(Clone, Debug, PartialEq)]
pub enum Pending {
    Template(u32, u32),
    Open(String),
    /// File ▸ Close Project: close, then show this screen.
    Close,
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let p = ui.painter().clone();
    p.rect_filled(rect, 0.0, t.app_bg);

    // two columns, centred: start (left) and recent projects (right)
    let w = rect.width().min(1220.0);
    let left = rect.center().x - w / 2.0 + 24.0;
    let right_col = left + w * 0.66;
    let top = rect.min.y + (rect.height() * 0.2).clamp(40.0, 200.0);

    // the logo
    cube::paint(&p, pos2(left + 30.0, top + 30.0), 26.0, cube::Pose::LOGO, cube::Reveal::ALL, 1.0);
    p.text(pos2(left + 76.0, top + 22.0), Align2::LEFT_CENTER, "JBD", Tokens::display(44.0), t.text);
    p.text(pos2(left + 78.0, top + 56.0), Align2::LEFT_CENTER, "AI video editor by Jabrod", Tokens::ui(17.0), t.text_dim);

    // templates
    let mut y = top + 116.0;
    style::spaced(&p, pos2(left, y), Align2::LEFT_TOP, "START WITH A BLANK TEMPLATE", Tokens::semibold(12.0), t.text_dim, 1.6);
    y += 28.0;
    let card_w = ((right_col - left - 60.0 - 3.0 * 15.0) / 4.0).clamp(130.0, 180.0);
    let card_h = 168.0;
    for (i, (id, name, use_for, tw, th)) in TEMPLATES.iter().enumerate() {
        let r = Rect::from_min_size(pos2(left + i as f32 * (card_w + 15.0), y), vec2(card_w, card_h));
        let resp = ui.interact(r, egui::Id::new(("welcome-template", *id)), Sense::click()).on_hover_text(format!("New project, {tw}×{th}"));
        app.auto.add(&format!("welcome.template.{id}"), r, name);
        let hot = resp.hovered();
        p.rect_filled(r, 10.0, if hot { t.hover } else { t.panel_bg });
        p.rect_stroke(r, 10.0, Stroke::new(1.0, if hot { t.field_border } else { t.separator }), egui::StrokeKind::Inside);
        // the shape, drawn to its aspect ratio
        let max = vec2(56.0, 56.0);
        let k = (max.x / *tw as f32).min(max.y / *th as f32);
        let shape = Rect::from_center_size(pos2(r.center().x, r.min.y + 52.0), vec2(*tw as f32 * k, *th as f32 * k));
        p.rect_stroke(shape, 5.0, Stroke::new(1.5, if hot { t.text } else { t.text_dim }), egui::StrokeKind::Inside);
        p.text(pos2(r.center().x, r.min.y + 106.0), Align2::CENTER_CENTER, *name, Tokens::semibold(14.5), t.text);
        p.text(pos2(r.center().x, r.min.y + 138.0), Align2::CENTER_CENTER, *use_for, Tokens::ui(12.5), t.text_dim);
        if hot {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked() {
            request(app, Pending::Template(*tw, *th));
        }
    }
    y += card_h + 14.0;
    p.text(pos2(left, y), Align2::LEFT_TOP, "Opens straight away as “Untitled”. Save it with Ctrl+S whenever you like.", Tokens::ui(12.0), t.text_faint);
    y += 46.0;

    // New / Open
    let new_r = Rect::from_min_size(pos2(left, y), vec2(190.0, 38.0));
    let nresp = ui.interact(new_r, egui::Id::new("welcome-new"), Sense::click());
    app.auto.add("welcome.new", new_r, "New project");
    p.rect_filled(new_r, 8.0, if nresp.hovered() { style::PRIMARY_BG_HOVER } else { style::PRIMARY_BG });
    p.text(pos2(new_r.min.x + 16.0, new_r.center().y), Align2::LEFT_CENTER, "+   New project…", Tokens::semibold(13.5), style::PRIMARY_TEXT);
    let kbd = Rect::from_min_size(pos2(new_r.max.x - 58.0, new_r.center().y - 11.0), vec2(48.0, 22.0));
    p.rect_filled(kbd, 5.0, Color32::from_rgb(0xe4, 0xe4, 0xe7));
    p.text(kbd.center(), Align2::CENTER_CENTER, "Ctrl+N", Tokens::ui(11.0), Color32::from_rgb(0x52, 0x52, 0x5b));
    if nresp.clicked() {
        request(app, Pending::Template(1920, 1080));
    }
    let open_r = Rect::from_min_size(pos2(new_r.max.x + 14.0, y), vec2(158.0, 38.0));
    let oresp = ui.interact(open_r, egui::Id::new("welcome-open"), Sense::click());
    app.auto.add("welcome.open", open_r, "Open project");
    p.rect_filled(open_r, 8.0, if oresp.hovered() { t.hover } else { t.panel_bg });
    p.rect_stroke(open_r, 8.0, Stroke::new(1.0, t.separator), egui::StrokeKind::Inside);
    crate::icons::paint(&p, Rect::from_center_size(pos2(open_r.min.x + 22.0, open_r.center().y), vec2(15.0, 15.0)), crate::icons::Icon::Folder, t.text);
    p.text(pos2(open_r.min.x + 38.0, open_r.center().y), Align2::LEFT_CENTER, "Open project…", Tokens::semibold(13.5), t.text);
    if oresp.clicked() {
        let picked = app.hooks.pick_open_project.as_mut().and_then(|f| f());
        if let Some(path) = picked {
            request(app, Pending::Open(path));
        }
    }
    let demo_r = Rect::from_min_size(pos2(open_r.max.x + 14.0, y), vec2(150.0, 38.0));
    let dresp = ui.interact(demo_r, egui::Id::new("welcome-demo"), Sense::click()).on_hover_text("A short edited sequence to explore JBD with");
    app.auto.add("welcome.demo", demo_r, "Try the demo");
    p.text(
        demo_r.left_center() + vec2(8.0, 0.0),
        Align2::LEFT_CENTER,
        "Try the demo project",
        Tokens::ui(13.0),
        if dresp.hovered() { t.text } else { t.text_dim },
    );
    if dresp.clicked() {
        request(app, Pending::Open(String::new()));
    }
    y += 80.0;

    // engine status
    let gpu = app.gpu_adapter_name();
    let status = match &gpu {
        Some(name) => format!("Video engine ready · H.264, HEVC, ProRes, AV1 and more · GPU: {name}"),
        None => "Video engine ready · H.264, HEVC, ProRes, AV1 and more · drawing on the CPU".to_string(),
    };
    p.circle_stroke(pos2(left + 7.0, y + 8.0), 6.5, Stroke::new(1.3, Color32::from_rgb(0x16, 0xc9, 0x6f)));
    p.line_segment([pos2(left + 4.0, y + 8.0), pos2(left + 6.5, y + 10.5)], Stroke::new(1.3, Color32::from_rgb(0x16, 0xc9, 0x6f)));
    p.line_segment([pos2(left + 6.5, y + 10.5), pos2(left + 10.5, y + 5.5)], Stroke::new(1.3, Color32::from_rgb(0x16, 0xc9, 0x6f)));
    p.text(pos2(left + 22.0, y + 8.0), Align2::LEFT_CENTER, status, Tokens::ui(12.0), t.text_faint);

    // recent projects
    let mut ry = top;
    style::spaced(&p, pos2(right_col + 20.0, ry), Align2::LEFT_TOP, "RECENT PROJECTS", Tokens::semibold(12.0), t.text_dim, 1.6);
    ry += 34.0;
    let recents: Vec<String> = app.session.prefs.general.recent_projects.iter().filter(|p| std::path::Path::new(p).exists()).take(8).cloned().collect();
    if recents.is_empty() {
        p.text(pos2(right_col + 20.0, ry), Align2::LEFT_TOP, "Projects you save show up here.", Tokens::ui(12.5), t.text_faint);
    }
    let col_w = rect.max.x - right_col - 60.0;
    for (i, path) in recents.iter().enumerate() {
        let r = Rect::from_min_size(pos2(right_col + 8.0, ry), vec2(col_w.max(200.0), 64.0));
        let resp = ui.interact(r, egui::Id::new(("welcome-recent", i)), Sense::click()).on_hover_text(path.as_str());
        app.auto.add(&format!("welcome.recent.{i}"), r, path);
        if resp.hovered() {
            p.rect_filled(r, 8.0, t.panel_bg);
        }
        let name = std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| path.clone());
        p.text(pos2(r.min.x + 12.0, r.min.y + 20.0), Align2::LEFT_CENTER, name, Tokens::semibold(13.5), t.text);
        let shown = elide(path, 52);
        p.text(pos2(r.min.x + 12.0, r.min.y + 44.0), Align2::LEFT_CENTER, shown, Tokens::ui(12.0), t.text_faint);
        if resp.clicked() {
            request(app, Pending::Open(path.clone()));
        }
        ry += 70.0;
    }
}

/// "C:\Users\…\Untitled 12" when longer than `max` characters.
fn elide(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let tail: String = s.chars().skip(n - (max - 12)).collect();
    let head: String = s.chars().take(10).collect();
    format!("{head}…{tail}")
}

/// Do it now, or ask about unsaved changes first.
pub(crate) fn request(app: &mut FilmcraftApp, what: Pending) {
    if app.session.is_dirty() {
        app.jbd.welcome_pending = Some(what);
    } else {
        go(app, what);
    }
}

/// Make the new project / open the chosen one, then show the editor.
fn go(app: &mut FilmcraftApp, what: Pending) {
    let result = match what {
        Pending::Template(w, h) => app
            .session
            .execute("file.newProject", json!({"name": "Untitled"}))
            .and_then(|_| app.session.execute("file.newSequence", json!({"name": "Main", "width": w, "height": h, "fps": 30.0}))),
        Pending::Open(path) if path.is_empty() => app.session.execute("file.openDemoProject", json!({})),
        Pending::Open(path) => app.session.execute("file.open", json!({"path": path})),
        Pending::Close => {
            if let Err(e) = app.session.execute("file.closeProject", json!({"force": true})) {
                app.ui.status = e.to_string();
            }
            app.ui.mode = Mode::Import;
            return;
        }
    };
    match result {
        Ok(_) => app.ui.mode = Mode::Edit,
        Err(e) => app.ui.status = e.to_string(),
    }
}

/// "Save changes to <project>?" before replacing or closing it (drawn after every frame, so it
/// also shows over the editor for File ▸ Close Project).
pub(crate) fn confirm_window(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(what) = app.jbd.welcome_pending.clone() else { return };
    let t = app.tokens;
    let name = app.session.project.name.clone();
    let mut answer: Option<&str> = None;
    egui::Modal::new(egui::Id::new("jbd-unsaved")).show(ctx, |ui| {
        ui.set_width(380.0);
        ui.label(egui::RichText::new(format!("Save changes to “{name}”?")).font(Tokens::semibold(15.0)).color(t.text));
        ui.add_space(4.0);
        ui.label(egui::RichText::new("Your changes are lost if you don't save them.").font(Tokens::ui(12.5)).color(t.text_dim));
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            let s = style::small_primary_button(ui, "Save", &t);
            app.auto.add("welcome.confirm.save", s.rect, "Save");
            if s.clicked() {
                answer = Some("save");
            }
            let d = ui.button("Don't save");
            app.auto.add("welcome.confirm.discard", d.rect, "Don't save");
            if d.clicked() {
                answer = Some("discard");
            }
            let c = ui.button("Cancel");
            app.auto.add("welcome.confirm.cancel", c.rect, "Cancel");
            if c.clicked() {
                answer = Some("cancel");
            }
        });
    });
    match answer {
        Some("save") => {
            let saved = crate::menus::invoke(app, ctx, "file.save", json!({})).is_ok() && !app.session.is_dirty();
            app.jbd.welcome_pending = None;
            if saved {
                go(app, what);
            }
        }
        Some("discard") => {
            app.jbd.welcome_pending = None;
            go(app, what);
        }
        Some(_) => app.jbd.welcome_pending = None,
        None => {}
    }
}
