//! JBD's Settings (Settings ▸ Settings…, Ctrl+,): the first JBD's layout, with sections on the left
//! and each setting as a row on the right, applied at once.
//!
//! Sections: General, Account, Appearance, Editing, Playback, Audio, Transcription, Media & cache,
//! Keyboard. The rows come from the engine's preferences schema (`prefs.schema`), and only settings
//! the editor really honours (`wired`) are shown, so every switch here does something. Everything
//! else stays in "All preferences…" (the full advanced dialog).
//!
//! Automation ids: `settings.section.<id>`, `settings.field.<key>`, `settings.all`,
//! `settings.close`, `settings.signOut`, `settings.upgrade`, `settings.shortcuts`.

use egui::{Color32, Rect, RichText, Sense, Stroke, pos2, vec2};
use serde_json::{Value, json};

use super::style;
use crate::FilmcraftApp;
use crate::theme::Tokens;

/// JBD's sections: id, title, what it's for, and the schema pages whose wired settings it shows.
const SECTIONS: [(&str, &str, &str, &[&str]); 9] = [
    ("general", "General", "What JBD does when it opens, and saving your work.", &["general", "autoSave"]),
    ("account", "Account", "Your Jabrod account, plan and AI credits.", &[]),
    ("appearance", "Appearance", "Theme and contrast.", &["appearance"]),
    ("editing", "Editing", "The timeline and trimming.", &["timeline", "trim"]),
    ("playback", "Playback", "Previewing your edit.", &["playback", "memory"]),
    ("audio", "Audio", "Sound devices and audio editing.", &["audio", "audioHardware"]),
    ("transcription", "Transcription", "Speech to text on this computer.", &["mediaAnalysis"]),
    ("media", "Media & cache", "Importing media and the disk cache.", &["media", "mediaCache"]),
    ("keyboard", "Keyboard", "Keyboard shortcuts.", &[]),
];

/// Clearer names for a few engine labels.
fn relabel(key: &str, label: &str) -> String {
    let l = match key {
        "general.atStartup" => "When JBD opens",
        "autoSave.enabled" => "Save projects automatically",
        "playback.hardwareDecoding" => "Use the graphics card to decode video",
        _ => label,
    };
    crate::jbd::platform_keys(l)
}

fn rechoice(key: &str, label: &str) -> String {
    match (key, label) {
        ("general.atStartup", "Show Home") => "Show the start screen".into(),
        ("general.atStartup", "Open Most Recent") => "Open the last project".into(),
        ("general.atStartup", "Start with an Empty Project") => "Start with an empty project".into(),
        _ => label.into(),
    }
}

/// Open JBD Settings at a section.
pub fn open(app: &mut FilmcraftApp, section: &str) {
    app.jbd.settings_open = true;
    app.jbd.settings_section = section.to_string();
    app.jbd.settings_schema = None;
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if !app.jbd.settings_open {
        return;
    }
    if app.jbd.settings_schema.is_none() {
        app.jbd.settings_schema = app.session.execute("prefs.schema", json!({})).ok();
    }
    let t = app.tokens;
    let screen = ctx.content_rect();
    let size = vec2(940.0f32.min(screen.width() - 40.0), 640.0f32.min(screen.height() - 40.0));
    let mut close = false;
    let mut changes: Vec<(String, Value)> = Vec::new();
    egui::Modal::new(egui::Id::new("jbd-settings"))
        .frame(egui::Frame::new().fill(t.panel_bg).stroke(Stroke::new(1.0, t.field_border)).corner_radius(14.0))
        .show(ctx, |ui| {
            ui.set_min_size(size);
            ui.set_max_size(size);
            let full = ui.max_rect();
            let nav = Rect::from_min_size(full.min, vec2(230.0, full.height()));
            let body = Rect::from_min_max(pos2(nav.max.x, full.min.y), full.max);
            let p = ui.painter().clone();
            p.rect_filled(nav, egui::CornerRadius { nw: 14, sw: 14, ne: 0, se: 0 }, Color32::from_rgb(0x0e, 0x0e, 0x10));
            p.line_segment([nav.right_top(), nav.right_bottom()], Stroke::new(1.0, t.separator));
            p.text(nav.min + vec2(22.0, 26.0), egui::Align2::LEFT_CENTER, "Settings", Tokens::semibold(17.0), t.text);
            // sections
            let mut y = nav.min.y + 58.0;
            for (id, title, _, _) in SECTIONS {
                let r = Rect::from_min_size(pos2(nav.min.x + 10.0, y), vec2(nav.width() - 20.0, 32.0));
                let resp = ui.interact(r, egui::Id::new(("jbd-settings-nav", id)), Sense::click());
                app.auto.add(&format!("settings.section.{id}"), r, title);
                let active = app.jbd.settings_section == id;
                if active || resp.hovered() {
                    p.rect_filled(r, 7.0, if active { t.hover } else { Color32::from_rgb(0x17, 0x17, 0x1a) });
                }
                p.text(pos2(r.min.x + 12.0, r.center().y), egui::Align2::LEFT_CENTER, title, Tokens::medium(13.0), if active { t.text } else { t.text_dim });
                if resp.clicked() {
                    app.jbd.settings_section = id.to_string();
                }
                y += 34.0;
            }
            // footer buttons in the nav
            let all = Rect::from_min_size(pos2(nav.min.x + 10.0, nav.max.y - 48.0), vec2(nav.width() - 20.0, 32.0));
            app.auto.add("settings.all", all, "All preferences");
            if style::ghost_button_at(ui, all, "settings-all", "All preferences…", &t).clicked() {
                close = true;
                let _ = crate::menus::invoke(app, ctx, "app.settings.general", json!({"jbdAdvanced": true}));
            }
            // close (×)
            let x = Rect::from_center_size(pos2(body.max.x - 26.0, body.min.y + 26.0), vec2(28.0, 28.0));
            let xr = ui.interact(x, egui::Id::new("jbd-settings-close"), Sense::click());
            app.auto.add("settings.close", x, "Close");
            if xr.hovered() {
                p.rect_filled(x, 6.0, t.hover);
            }
            p.line_segment([x.center() + vec2(-5.0, -5.0), x.center() + vec2(5.0, 5.0)], Stroke::new(1.4, t.text_dim));
            p.line_segment([x.center() + vec2(-5.0, 5.0), x.center() + vec2(5.0, -5.0)], Stroke::new(1.4, t.text_dim));
            if xr.clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                close = true;
            }
            // the section
            let Some(&(sid, title, about, pages)) = SECTIONS.iter().find(|s| s.0 == app.jbd.settings_section) else { return };
            let inner = body.shrink2(vec2(32.0, 22.0));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
            child.label(RichText::new(title).font(Tokens::semibold(20.0)).color(t.text));
            child.label(RichText::new(about).font(Tokens::ui(12.5)).color(t.text_dim));
            child.add_space(14.0);
            egui::ScrollArea::vertical().id_salt(("jbd-settings-scroll", sid)).auto_shrink([false, false]).show(&mut child, |ui| {
                ui.set_width(inner.width() - 12.0);
                match sid {
                    "account" => account(app, ui, ctx, &t),
                    "keyboard" => {
                        ui.label(
                            RichText::new("See and change every keyboard shortcut, or switch to Premiere Pro, Final Cut Pro or Avid keys.").color(t.text_dim),
                        );
                        ui.add_space(10.0);
                        let b = style::small_primary_button(ui, "Keyboard Shortcuts…", &t);
                        app.auto.add("settings.shortcuts", b.rect, "Keyboard Shortcuts");
                        if b.clicked() {
                            close = true;
                            let _ = crate::menus::invoke(app, ctx, "app.keyboardShortcuts", json!({}));
                        }
                    }
                    _ => {
                        let fields = schema_fields(app.jbd.settings_schema.as_ref(), pages);
                        if fields.is_empty() {
                            ui.label(RichText::new("Nothing to set here yet.").color(t.text_faint));
                        }
                        for f in fields {
                            if let Some(v) = field_row(app, ui, &f, &t) {
                                changes.push(v);
                            }
                        }
                    }
                }
            });
        });
    for (key, value) in changes {
        match app.session.execute("prefs.set", json!({"key": key, "value": value})) {
            Ok(_) => {
                app.jbd.settings_schema = None; // re-read: values and enabled states change
                app.apply_prefs(ctx);
            }
            Err(e) => app.ui.status = e.to_string(),
        }
    }
    if close {
        app.jbd.settings_open = false;
    }
}

/// The wired fields of the given schema pages, in order.
fn schema_fields(schema: Option<&Value>, pages: &[&str]) -> Vec<Value> {
    let Some(all) = schema.and_then(|s| s.as_object()).and_then(|o| o.values().next()).and_then(|v| v.as_array()) else { return Vec::new() };
    pages
        .iter()
        .flat_map(|pid| all.iter().filter(move |p| p["id"] == *pid))
        .flat_map(|p| p["fields"].as_array().cloned().unwrap_or_default())
        .filter(|f| f["wired"] == json!(true))
        .collect()
}

/// One setting: its label on the left, its control on the right. Returns a change to apply.
fn field_row(app: &mut FilmcraftApp, ui: &mut egui::Ui, f: &Value, t: &Tokens) -> Option<(String, Value)> {
    let key = f["key"].as_str()?.to_string();
    let label = relabel(&key, f["label"].as_str().unwrap_or(&key));
    let kind = &f["kind"];
    let value = &f["value"];
    let mut change = None;
    let row = ui.horizontal(|ui| {
        ui.set_min_height(38.0);
        ui.label(RichText::new(&label).font(Tokens::ui(13.0)).color(t.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match kind["type"].as_str().unwrap_or("") {
            "bool" => {
                let mut on = value.as_bool().unwrap_or(false);
                if toggle(ui, &mut on, t).clicked() {
                    change = Some((key.clone(), json!(on)));
                }
            }
            "choice" | "label" | "device" | "font" => {
                let choices: Vec<(String, Value)> = kind["choices"]
                    .as_array()
                    .map(|cs| cs.iter().map(|c| (rechoice(&key, c["label"].as_str().unwrap_or("")), c["value"].clone())).collect())
                    .unwrap_or_default();
                if choices.is_empty() {
                    ui.label(RichText::new(value.as_str().unwrap_or("—")).color(t.text_dim));
                } else {
                    let current = choices.iter().find(|(_, v)| v == value).map_or_else(|| value.to_string(), |(l, _)| l.clone());
                    egui::ComboBox::from_id_salt(("jbd-set", &key)).selected_text(current).width(240.0).show_ui(ui, |ui| {
                        for (l, v) in &choices {
                            if ui.selectable_label(v == value, l).clicked() {
                                change = Some((key.clone(), v.clone()));
                            }
                        }
                    });
                }
            }
            "int" | "float" | "duration" => {
                let mut v = value.as_f64().unwrap_or(0.0);
                let (min, max) = (kind["min"].as_f64().unwrap_or(f64::MIN), kind["max"].as_f64().unwrap_or(f64::MAX));
                let decimals = if kind["type"] == "int" { 0 } else { kind["decimals"].as_u64().unwrap_or(2) as usize };
                let mut dv = egui::DragValue::new(&mut v).range(min..=max).max_decimals(decimals);
                if let Some(unit) = kind["unit"].as_str() {
                    dv = dv.suffix(format!(" {unit}"));
                }
                let r = ui.add(dv);
                if r.drag_stopped() || r.lost_focus() {
                    change = Some((key.clone(), if decimals == 0 { json!(v.round() as i64) } else { json!(v) }));
                }
            }
            "color" => {
                let hex = value.as_str().unwrap_or("#3d5cff");
                let mut c = filmcraft_engine::settings::parse_hex(hex).unwrap_or([0x3d, 0x5c, 0xff]);
                if ui.color_edit_button_srgb(&mut c).changed() {
                    change = Some((key.clone(), json!(filmcraft_engine::settings::hex(c))));
                }
            }
            _ => {
                let shown = value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string());
                ui.label(RichText::new(shown).font(Tokens::ui(12.0)).color(t.text_dim));
            }
        });
    });
    app.auto.add(&format!("settings.field.{key}"), row.response.rect, &label);
    ui.painter().line_segment([row.response.rect.left_bottom(), row.response.rect.right_bottom()], Stroke::new(1.0, t.separator));
    change
}

/// The first JBD's switch.
fn toggle(ui: &mut egui::Ui, on: &mut bool, t: &Tokens) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(40.0, 22.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let k = ui.ctx().animate_bool(resp.id, *on);
    let bg = if *on { Color32::from_rgb(0xfa, 0xfa, 0xfa) } else { t.field_border };
    ui.painter().rect_filled(rect, 11.0, bg);
    let x = egui::lerp(rect.min.x + 11.0..=rect.max.x - 11.0, k);
    ui.painter().circle_filled(pos2(x, rect.center().y), 8.0, if *on { Color32::from_rgb(0x0b, 0x0b, 0x0c) } else { t.text });
    resp
}

/// The Account section: the same facts as the account menu, with room to breathe.
fn account(app: &mut FilmcraftApp, ui: &mut egui::Ui, ctx: &egui::Context, t: &Tokens) {
    let s = app.jbd.status.clone();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if let Some(a) = &s.account {
        ui.label(RichText::new(a.display_name()).font(Tokens::semibold(15.0)).color(t.text));
        ui.label(RichText::new(&a.email).color(t.text_dim));
    }
    ui.add_space(14.0);
    ui.label(RichText::new(s.plan_line(now)).font(Tokens::medium(14.0)).color(t.text));
    if let Some(u) = s.usage {
        ui.add_space(8.0);
        ui.label(RichText::new("AI credits this month").color(t.text_dim));
        let (bar, _) = ui.allocate_exact_size(vec2(360.0, 6.0), Sense::hover());
        style::meter(ui.painter(), bar, u.fraction(), t, None);
        ui.label(RichText::new(format!("{} of {} used", style::thousands(u.calls), style::thousands(u.limit))).font(Tokens::ui(12.0)).color(t.text_dim));
    }
    ui.add_space(16.0);
    ui.horizontal(|ui| {
        if !s.is_pro() || s.trial {
            let up = style::small_primary_button(ui, "Upgrade to Pro", t);
            app.auto.add("settings.upgrade", up.rect, "Upgrade to Pro");
            if up.clicked() {
                super::open_page(ctx, jbd_account::Page::Upgrade);
            }
        }
        let out = ui.button("Sign out");
        app.auto.add("settings.signOut", out.rect, "Sign out");
        if out.clicked()
            && let Some(b) = &app.jbd.backend
        {
            b.sign_out();
            app.jbd.settings_open = false;
        }
    });
}
