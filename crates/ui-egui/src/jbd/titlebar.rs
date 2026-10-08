//! JBD's title bar (the first JBD's `TitleBar.tsx`), drawn by the app because the window has no
//! system frame:
//!
//!   [cube] JBD  File Edit View Timeline Settings Help  ↶ ↷      Untitled ✓ Saved      CPU ▬ 34%  GPU ▬ 10%  [▯][▭][▯]  [Export]  (AT PRO)  — ☐ ✕
//!
//! - The menus are JBD's six; each item is a real engine/UI command (labels, shortcuts and
//!   enabled states come from the command registry, `menus::menu_items`). FilmCraft's full Clip,
//!   Sequence, Markers, Graphics and Window menus sit under Timeline and View for power users.
//! - The CPU / GPU meters come from the host (`JbdState::usage_source`, see the app's main.rs).
//! - The three layout buttons show or hide Media, the Inspector side and the Timeline.
//! - The empty bar moves the window; a double-click maximizes it.
//!
//! Automation ids: `titlebar.undo`, `titlebar.redo`, `titlebar.layout.{media,timeline,side}`,
//! `titlebar.export`, `header.account`, `window.{minimize,maximize,close}`.

use egui::{Align2, Color32, Pos2, Rect, Sense, Shape, Stroke, pos2, vec2};
use serde_json::json;

use super::{cube, style};
use crate::FilmcraftApp;
use crate::menus::{self, MenuItem};
use crate::state::Mode;
use crate::theme::Tokens;

/// The title bar's height.
pub const HEIGHT: f32 = 44.0;

/// One entry in a JBD menu.
enum Entry {
    /// A registry command (its label from the registry).
    Cmd(&'static str),
    /// A registry command shown under another label.
    As(&'static str, &'static str),
    /// A JBD action handled here (id, label, shortcut).
    Jbd(&'static str, &'static str, Option<&'static str>),
    /// A submenu holding every registry item under a FilmCraft menu path.
    Sub(&'static str, &'static [&'static str]),
    Sep,
}

use Entry::*;

/// JBD's menus (the first JBD's File / Edit / View / Timeline / Settings / Help).
const MENUS: [(&str, &[Entry]); 6] = [
    (
        "File",
        &[
            Jbd("jbd.home", "New Project…", Some("Ctrl+N")),
            As("file.open", "Open Project…"),
            Sep,
            Cmd("file.save"),
            Cmd("file.saveAs"),
            Sep,
            As("file.import", "Import Media…"),
            Jbd("jbd.export", "Export…", Some("Ctrl+M")),
            Sep,
            Cmd("file.closeProject"),
            Sub("More", &["File"]),
        ],
    ),
    (
        "Edit",
        &[
            Cmd("edit.undo"),
            Cmd("edit.redo"),
            Sep,
            Cmd("edit.cut"),
            Cmd("edit.copy"),
            Cmd("edit.paste"),
            Cmd("edit.duplicate"),
            As("edit.clear", "Delete"),
            Sep,
            Cmd("edit.selectAll"),
            Sub("More", &["Edit"]),
        ],
    ),
    (
        "View",
        &[
            Jbd("jbd.toggle.media", "Media Panel", None),
            Jbd("jbd.toggle.side", "Inspector Panel", None),
            Jbd("jbd.toggle.timeline", "Timeline", None),
            Jbd("jbd.layout.reset", "Reset Layout", None),
            Sep,
            Cmd("view.zoomIn"),
            Cmd("view.zoomOut"),
            Cmd("view.zoomToSequence"),
            Sep,
            Sub("Panels and Workspaces", &["Window"]),
            Sub("More", &["View"]),
        ],
    ),
    (
        "Timeline",
        &[
            As("sequence.addEdit", "Split at Playhead"),
            As("sequence.addTracks", "Add Tracks…"),
            As("markers.add", "Add Marker"),
            As("sequence.settings", "Timeline Settings…"),
            Sep,
            Sub("Clip", &["Clip"]),
            Sub("Sequence", &["Sequence"]),
            Sub("Markers", &["Markers"]),
            Sub("Graphics and Titles", &["Graphics and Titles"]),
        ],
    ),
    ("Settings", &[Jbd("jbd.settings", "Settings…", Some("Ctrl+,")), Cmd("app.keyboardShortcuts"), Sep, Sub("Preferences", &["Edit", "Preferences"])]),
    ("Help", &[Cmd("help.appPage"), Cmd("help.support"), Cmd("help.revealLogFiles"), Sep, As("app.about", "About JBD")]),
];

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let p = ui.painter().clone();
    p.rect_filled(rect, 0.0, t.app_bg);
    p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, t.separator));
    // the empty bar moves the window; double-click maximizes
    let drag = ui.interact(rect, egui::Id::new("jbd-titlebar-drag"), Sense::click_and_drag());
    if drag.drag_started() {
        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    if drag.double_clicked() {
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    }
    let cy = rect.center().y;

    // ── left: logo, menus, undo / redo ──────────────────────────────────────────────────────
    let mut x = rect.min.x + 14.0;
    cube::paint(&p, pos2(x + 9.0, cy), 11.0, cube::Pose::LOGO, cube::Reveal::ALL, 1.0);
    x += 24.0;
    let word = p.text(pos2(x, cy), Align2::LEFT_CENTER, "JBD", Tokens::display(15.0), t.text);
    let home = Rect::from_min_max(pos2(rect.min.x + 8.0, rect.min.y + 6.0), pos2(word.max.x + 4.0, rect.max.y - 6.0));
    let hresp = ui.interact(home, egui::Id::new("jbd-titlebar-home"), Sense::click()).on_hover_text("Start screen");
    app.auto.add("titlebar.home", home, "Start screen");
    if hresp.clicked() {
        app.ui.mode = Mode::Import;
    }
    x = word.max.x + 14.0;
    // the menus' width: each label plus its button padding and the spacing between them
    let menus_w: f32 =
        MENUS.iter().map(|(name, _)| p.layout_no_wrap((*name).to_string(), Tokens::ui(13.0), t.text).size().x + 18.0 + ui.spacing().item_spacing.x).sum();
    let menu_rect = Rect::from_min_max(pos2(x, rect.min.y + 8.0), pos2(x + menus_w + 4.0, rect.max.y - 8.0));
    let mut mu = ui.new_child(egui::UiBuilder::new().max_rect(menu_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
    {
        let st = mu.style_mut();
        st.visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
        st.visuals.widgets.inactive.bg_stroke = Stroke::NONE;
        st.visuals.override_text_color = Some(t.text);
        st.text_styles.insert(egui::TextStyle::Button, Tokens::ui(13.0));
        st.spacing.button_padding = vec2(9.0, 4.0);
    }
    let clicked = menu_bar(app, &mut mu);
    let mut lx = menu_rect.max.x + 8.0;
    for (id, tip, cmd, redo) in [("undo", "Undo (Ctrl+Z)", "edit.undo", false), ("redo", "Redo (Ctrl+Shift+Z)", "edit.redo", true)] {
        let r = Rect::from_center_size(pos2(lx + 13.0, cy), vec2(26.0, 26.0));
        lx += 30.0;
        let resp = ui.interact(r, egui::Id::new(("jbd-titlebar", id)), Sense::click()).on_hover_text(tip);
        app.auto.add(&format!("titlebar.{id}"), r, tip);
        if resp.hovered() {
            p.rect_filled(r, 6.0, t.hover);
        }
        undo_icon(&p, r.center(), redo, if resp.hovered() { t.text } else { t.text_dim });
        if resp.clicked() {
            let _ = menus::invoke(app, &ctx, cmd, json!({}));
        }
    }
    let left_end = lx;

    // ── right: window controls, account, Export, layout, meters ─────────────────────────────
    let mut rx = rect.max.x;
    for (id, tip) in [("close", "Close"), ("maximize", if maximized { "Restore" } else { "Maximize" }), ("minimize", "Minimize")] {
        let r = Rect::from_min_max(pos2(rx - 46.0, rect.min.y), pos2(rx, rect.max.y - 1.0));
        rx -= 46.0;
        let resp = ui.interact(r, egui::Id::new(("jbd-window", id)), Sense::click()).on_hover_text(tip);
        app.auto.add(&format!("window.{id}"), r, tip);
        let hot = resp.hovered();
        if hot {
            p.rect_filled(r, 0.0, if id == "close" { Color32::from_rgb(0xc4, 0x2b, 0x1c) } else { t.hover });
        }
        window_icon(&p, r.center(), id, maximized, if hot { Color32::WHITE } else { t.text_dim });
        if resp.clicked() {
            match id {
                "close" => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                "maximize" => ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized)),
                _ => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            }
        }
    }
    rx -= 10.0;
    if let Some(chip) = super::account::header_chip(app, ui, rx, cy) {
        rx = chip.min.x - 10.0;
    }
    // Export: the white button
    let g = p.layout_no_wrap("Export".to_string(), Tokens::semibold(12.5), style::PRIMARY_TEXT);
    let ex = Rect::from_min_size(pos2(rx - g.size().x - 38.0, cy - 14.0), vec2(g.size().x + 38.0, 28.0));
    let eresp = ui.interact(ex, egui::Id::new("jbd-titlebar-export"), Sense::click()).on_hover_text("Export (Ctrl+M)");
    app.auto.add("titlebar.export", ex, "Export");
    p.rect_filled(ex, 7.0, if eresp.hovered() { style::PRIMARY_BG_HOVER } else { style::PRIMARY_BG });
    crate::icons::paint(&p, Rect::from_center_size(pos2(ex.min.x + 16.0, cy), vec2(14.0, 14.0)), crate::icons::Icon::Export, style::PRIMARY_TEXT);
    p.galley(pos2(ex.min.x + 28.0, cy - g.size().y / 2.0), g, style::PRIMARY_TEXT);
    if eresp.clicked() {
        app.ui.mode = if app.ui.mode == Mode::Export { Mode::Edit } else { Mode::Export };
    }
    rx = ex.min.x - 14.0;
    // layout: Media | Timeline | Inspector side
    let areas = app.jbd.areas;
    for (id, tip, on) in [("side", "Inspector panel", areas.side), ("timeline", "Timeline", areas.timeline), ("media", "Media panel", areas.media)] {
        let r = Rect::from_center_size(pos2(rx - 14.0, cy), vec2(28.0, 28.0));
        rx -= 32.0;
        let resp = ui.interact(r, egui::Id::new(("jbd-layout", id)), Sense::click()).on_hover_text(format!("Show or hide the {tip}"));
        app.auto.add(&format!("titlebar.layout.{id}"), r, tip);
        if resp.hovered() || on {
            p.rect_filled(r, 6.0, if resp.hovered() { t.hover } else { Color32::from_rgb(0x18, 0x18, 0x1b) });
        }
        layout_icon(&p, r.center(), id, on, if on || resp.hovered() { t.text } else { t.text_faint });
        if resp.clicked() {
            super::toggle_area(app, id);
        }
    }
    rx -= 10.0;
    // CPU / GPU meters
    super::poll_usage(app, &ctx);
    let usage = app.jbd.usage;
    for (label, value) in [("GPU", usage.gpu), ("CPU", usage.cpu)] {
        let Some(v) = value else { continue };
        let pct = p.layout_no_wrap(format!("{:>3.0}%", v.clamp(0.0, 100.0)), Tokens::mono(11.0), t.text_dim);
        let px = rx - pct.size().x;
        p.galley(pos2(px, cy - pct.size().y / 2.0), pct, t.text_dim);
        let bar = Rect::from_min_size(pos2(px - 50.0, cy - 2.5), vec2(42.0, 5.0));
        let f = (v / 100.0).clamp(0.0, 1.0);
        let col = if f > 0.85 {
            t.danger
        } else if f > 0.6 {
            Color32::from_rgb(0xf5, 0xb4, 0x00)
        } else {
            Color32::from_rgb(0x16, 0xc9, 0x6f)
        };
        style::meter(&p, bar, f, &t, Some(col));
        let lab = p.text(pos2(bar.min.x - 6.0, cy), Align2::RIGHT_CENTER, label, Tokens::mono(10.5), t.text_faint);
        let whole = Rect::from_min_max(pos2(lab.min.x, rect.min.y + 8.0), pos2(rx, rect.max.y - 8.0));
        ui.interact(whole, egui::Id::new(("jbd-meter", label)), Sense::hover())
            .on_hover_text(format!("{label}: {v:.0}% in use on this computer (all programs)"));
        rx = lab.min.x - 16.0;
    }
    let right_end = rx;

    // ── centre: project name and save state ─────────────────────────────────────────────────
    let name = if app.session.project.name.trim().is_empty() { "Untitled".to_string() } else { app.session.project.name.clone() };
    let (state, state_icon) = if app.session.is_dirty() { ("Edited", false) } else { ("Saved", true) };
    let ng = p.layout_no_wrap(name, Tokens::semibold(13.5), t.text);
    let sg = p.layout_no_wrap(state.to_string(), Tokens::ui(13.0), t.text_dim);
    let total = ng.size().x + 14.0 + 16.0 + sg.size().x;
    let mut cx = rect.center().x - total / 2.0;
    if cx < left_end + 16.0 {
        cx = left_end + 16.0;
    }
    if cx + total < right_end - 8.0 {
        p.galley(pos2(cx, cy - ng.size().y / 2.0), ng.clone(), t.text);
        let ix = cx + ng.size().x + 14.0;
        if state_icon {
            check_icon(&p, pos2(ix + 6.0, cy), t.text_dim);
        } else {
            p.circle_filled(pos2(ix + 6.0, cy), 3.0, t.text_dim);
        }
        p.galley(pos2(ix + 16.0, cy - sg.size().y / 2.0), sg, t.text_dim);
    }

    if let Some(id) = clicked {
        run(app, &ctx, &id);
    }
}

/// Run a menu choice: JBD actions here, everything else through the command registry.
pub fn run(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str) {
    match id {
        "jbd.home" => app.ui.mode = Mode::Import,
        "jbd.export" => app.ui.mode = Mode::Export,
        "jbd.toggle.media" => super::toggle_area(app, "media"),
        "jbd.toggle.side" => super::toggle_area(app, "side"),
        "jbd.toggle.timeline" => super::toggle_area(app, "timeline"),
        "jbd.layout.reset" => super::reset_layout(app),
        "jbd.settings" => super::settings::open(app, "general"),
        _ => {
            if let Err(e) = menus::invoke(app, ctx, id, json!({})) {
                app.ui.status = e;
            }
        }
    }
}

/// The six JBD menus. Returns the chosen item's id.
fn menu_bar(app: &mut FilmcraftApp, ui: &mut egui::Ui) -> Option<String> {
    let items = menus::menu_items(app);
    let by_id = |id: &str| items.iter().find(|i| i.id == id);
    let areas = app.jbd.areas;
    let mut clicked: Option<String> = None;
    egui::MenuBar::new().ui(ui, |ui| {
        for (top, entries) in MENUS.iter() {
            ui.menu_button(*top, |ui| {
                ui.set_min_width(250.0);
                for e in entries.iter() {
                    match e {
                        Cmd(id) => {
                            if let Some(it) = by_id(id)
                                && menus::menu_entry(ui, it)
                            {
                                clicked = Some(it.id.clone());
                                ui.close();
                            }
                        }
                        As(id, label) => {
                            if let Some(it) = by_id(id) {
                                let renamed = MenuItem { label: (*label).to_string(), ..it.clone() };
                                if menus::menu_entry(ui, &renamed) {
                                    clicked = Some(it.id.clone());
                                    ui.close();
                                }
                            }
                        }
                        Jbd(id, label, shortcut) => {
                            let checked = match *id {
                                "jbd.toggle.media" => Some(areas.media),
                                "jbd.toggle.side" => Some(areas.side),
                                "jbd.toggle.timeline" => Some(areas.timeline),
                                _ => None,
                            };
                            let it = MenuItem {
                                id: (*id).into(),
                                label: (*label).into(),
                                path: vec![],
                                shortcut: shortcut.map(str::to_string),
                                enabled: true,
                                checked,
                            };
                            if menus::menu_entry(ui, &it) {
                                clicked = Some(it.id);
                                ui.close();
                            }
                        }
                        Sub(label, path) => {
                            let under: Vec<&MenuItem> =
                                items.iter().filter(|i| i.path.len() > path.len() - 1 && i.path.iter().zip(path.iter()).all(|(a, b)| a == b)).collect();
                            if !under.is_empty() {
                                ui.menu_button(*label, |ui| {
                                    ui.set_min_width(240.0);
                                    menus::menu_level(ui, &under, path.len(), &mut clicked);
                                });
                            }
                        }
                        Sep => {
                            ui.separator();
                        }
                    }
                }
            });
        }
    });
    clicked
}

// ── small icons drawn in code ─────────────────────────────────────────────────────────────────

fn undo_icon(p: &egui::Painter, c: Pos2, redo: bool, col: Color32) {
    let s = if redo { -1.0 } else { 1.0 };
    let st = Stroke::new(1.5, col);
    // a curved arrow: arc from the right, round to the left, head on the left end
    let pts: Vec<Pos2> = (0..=10)
        .map(|i| {
            let a = std::f32::consts::PI * (0.15 + 0.85 * i as f32 / 10.0);
            pos2(c.x + s * 5.5 * a.cos(), c.y + 1.5 - 4.5 * a.sin())
        })
        .collect();
    let head = pts[pts.len() - 1];
    p.add(Shape::line(pts, st));
    p.line_segment([head, head + vec2(s * 1.0, -3.5)], st);
    p.line_segment([head, head + vec2(s * 3.5, 1.0)], st);
}

fn check_icon(p: &egui::Painter, c: Pos2, col: Color32) {
    let st = Stroke::new(1.5, col);
    p.line_segment([c + vec2(-4.0, 0.0), c + vec2(-1.0, 3.0)], st);
    p.line_segment([c + vec2(-1.0, 3.0), c + vec2(4.5, -3.5)], st);
}

fn layout_icon(p: &egui::Painter, c: Pos2, which: &str, on: bool, col: Color32) {
    let r = Rect::from_center_size(c, vec2(16.0, 13.0));
    p.rect_stroke(r, 2.0, Stroke::new(1.3, col), egui::StrokeKind::Inside);
    let fill = match which {
        "media" => Rect::from_min_max(r.min, pos2(r.min.x + 5.5, r.max.y)),
        "side" => Rect::from_min_max(pos2(r.max.x - 5.5, r.min.y), r.max),
        _ => Rect::from_min_max(pos2(r.min.x, r.max.y - 4.5), r.max),
    };
    if on {
        p.rect_filled(fill, 1.5, col);
    } else {
        let edge = match which {
            "media" => [pos2(fill.max.x, r.min.y), pos2(fill.max.x, r.max.y)],
            "side" => [pos2(fill.min.x, r.min.y), pos2(fill.min.x, r.max.y)],
            _ => [pos2(r.min.x, fill.min.y), pos2(r.max.x, fill.min.y)],
        };
        p.line_segment(edge, Stroke::new(1.3, col));
    }
}

fn window_icon(p: &egui::Painter, c: Pos2, which: &str, maximized: bool, col: Color32) {
    let st = Stroke::new(1.0, col);
    match which {
        "close" => {
            p.line_segment([c + vec2(-5.0, -5.0), c + vec2(5.0, 5.0)], st);
            p.line_segment([c + vec2(-5.0, 5.0), c + vec2(5.0, -5.0)], st);
        }
        "maximize" if maximized => {
            p.rect_stroke(Rect::from_min_size(c + vec2(-5.0, -3.0), vec2(8.0, 8.0)), 1.0, st, egui::StrokeKind::Inside);
            p.line_segment([c + vec2(-3.0, -3.0), c + vec2(-3.0, -5.0)], st);
            p.line_segment([c + vec2(-3.0, -5.0), c + vec2(5.0, -5.0)], st);
            p.line_segment([c + vec2(5.0, -5.0), c + vec2(5.0, 3.0)], st);
            p.line_segment([c + vec2(5.0, 3.0), c + vec2(3.0, 3.0)], st);
        }
        "maximize" => {
            p.rect_stroke(Rect::from_center_size(c, vec2(10.0, 10.0)), 1.0, st, egui::StrokeKind::Inside);
        }
        _ => {
            p.line_segment([c + vec2(-5.0, 0.0), c + vec2(5.0, 0.0)], st);
        }
    }
}
