//! JBD's own layer over the editor: the launch animation, signing in with a Jabrod account, the
//! account menu (plan, AI credits, upgrade) and the JBD Free limits.
//!
//! Turned on by [`crate::FilmcraftApp::with_account`] (the JBD app does it in `main.rs`). Without
//! an account backend (UI tests, tools) none of this runs and the editor opens directly.
//!
//! Each frame, [`gate`] runs before the editor:
//!
//!   splash (≥ 2.3 s, click to skip) → sign-in screen (until signed in) → the editor
//!
//! and keeps `Session::plan` in step with the account, so exports follow the plan's limits
//! (enforced in the engine: `export_tools::spawn_export`).
//!
//! Files: [`cube`] (the 3D logo and JBD's one loader), [`splash`], [`sign_in`], [`account`] (the
//! account chip and its menu), [`titlebar`] (JBD's title bar and menus), [`welcome`] (the start
//! screen), [`director`] (the Director tab), [`window`] (resizing the frameless window),
//! [`licences`] (third-party notices), [`style`] (drawing helpers shared by these screens).

pub mod account;
pub mod cube;
pub mod director;
pub mod licences;
pub mod settings;
pub mod sign_in;
pub mod splash;
pub mod style;
pub mod titlebar;
pub mod welcome;
pub mod window;

use std::sync::Arc;

use jbd_account::{AccountBackend, AccountStatus, Page, Phase};

use crate::FilmcraftApp;
use crate::dock::{JbdAreas, jbd_workspace};

/// Live CPU / GPU load for the title-bar meters (percent, whole computer).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SystemUsage {
    pub cpu: Option<f32>,
    pub gpu: Option<f32>,
}

/// JBD state kept by the app.
#[derive(Default)]
pub struct JbdState {
    /// The Jabrod account. None = JBD layer off (tests, tools).
    pub backend: Option<Arc<dyn AccountBackend>>,
    /// The account as of this frame.
    pub status: AccountStatus,
    /// The launch animation, while it plays.
    pub splash: Option<splash::Splash>,
    /// The account menu under the header chip is open.
    pub menu_open: bool,
    /// The last sign-in link (for "Copy link" when the browser didn't open).
    pub sign_in_url: Option<String>,
    /// Which side areas the JBD layout shows (title-bar layout buttons).
    pub areas: JbdAreas,
    /// Where the title-bar meters get their numbers (set by the host; None = no meters).
    pub usage_source: Option<Box<dyn FnMut() -> SystemUsage + Send>>,
    /// The meters' last reading and when it was taken (UI time, s).
    pub usage: SystemUsage,
    usage_at: f64,
    /// The third-party licences window is open.
    pub licences_open: bool,
    /// JBD Settings is open, at this section, with this preferences schema (re-read after changes).
    pub settings_open: bool,
    pub settings_section: String,
    pub settings_schema: Option<serde_json::Value>,
    /// A start-screen choice waiting for the "unsaved changes" answer.
    pub welcome_pending: Option<welcome::Pending>,
    /// Window focus last frame (regaining it re-checks the plan, so a purchase shows at once).
    was_focused: Option<bool>,
    repaint_hooked: bool,
}

impl JbdState {
    pub fn new(backend: Arc<dyn AccountBackend>, splash: bool) -> Self {
        JbdState { status: backend.status(), backend: Some(backend), splash: splash.then(splash::Splash::default), ..Default::default() }
    }

    /// The JBD layer is on.
    pub fn enabled(&self) -> bool {
        self.backend.is_some()
    }

    /// Free-plan limits apply right now.
    pub fn free_limits(&self) -> bool {
        self.enabled() && !self.status.is_pro()
    }
}

/// Show or hide one of the layout's side areas ("media", "side", "timeline").
pub fn toggle_area(app: &mut FilmcraftApp, which: &str) {
    let a = &mut app.jbd.areas;
    match which {
        "media" => a.media = !a.media,
        "side" => a.side = !a.side,
        _ => a.timeline = !a.timeline,
    }
    app.ui.workspace = "JBD".into();
    app.ui.dock = jbd_workspace(app.jbd.areas);
}

/// Every area back, at its default size.
pub fn reset_layout(app: &mut FilmcraftApp) {
    app.jbd.areas = JbdAreas::default();
    app.ui.workspace = "JBD".into();
    app.ui.dock = jbd_workspace(app.jbd.areas);
}

/// Read the CPU / GPU meters about once a second.
pub fn poll_usage(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let now = ctx.input(|i| i.time);
    if now - app.jbd.usage_at < 1.0 {
        return;
    }
    app.jbd.usage_at = now;
    if let Some(src) = app.jbd.usage_source.as_mut() {
        app.jbd.usage = src();
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(1000));
}

/// After the editor has drawn: JBD windows that float over everything, and the frameless
/// window's resize edges (last, so they win over the panels at the very edge).
pub fn after_frame(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    if !app.jbd.enabled() {
        return;
    }
    licences::show(app, ui.ctx());
    settings::show(app, ui.ctx());
    window::resize_edges(ui);
}

/// JBD's clip and bin colours: each label still at its Premiere default gets the JBD colour
/// (movie clips blue, video violet, audio cyan, stills violet, timelines teal, bins grey). Labels
/// the user has changed keep their colour. Runs at launch (not saved unless the user saves
/// Settings ▸ Labels).
pub fn apply_palette(session: &mut filmcraft_engine::Session) {
    use filmcraft_project::Label;
    const JBD: [(Label, &str); 7] = [
        (Label::Iris, "#2c3aa8"),
        (Label::Violet, "#4a2bc4"),
        (Label::Caribbean, "#0a5f73"),
        (Label::Lavender, "#5f2fb8"),
        (Label::Forest, "#1d5c6e"),
        (Label::Mango, "#3a3a44"),
        (Label::Rose, "#7a2048"),
    ];
    for (label, color) in JBD {
        let key = filmcraft_engine::settings::label_id(label);
        let premiere = filmcraft_engine::settings::hex(label.rgb());
        if let Some(c) = session.prefs.labels.colors.get_mut(&key)
            && c.color.eq_ignore_ascii_case(&premiere)
        {
            c.color = color.to_string();
        }
    }
}

/// The first JBD's Media toolbar, in the Media panel's tab strip: Text, Shape, Motion (graphics
/// templates) and Import. `strip` is the tab strip's rect.
pub fn media_toolbar(app: &mut FilmcraftApp, ui: &mut egui::Ui, strip: egui::Rect) {
    use egui::{Align2, Sense, pos2, vec2};
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let mut x = strip.max.x - 8.0;
    let cy = strip.center().y;
    for (id, label, icon, boxed) in [
        ("import", "Import", crate::icons::Icon::Export, true),
        ("motion", "Motion", crate::icons::Icon::Fx, false),
        ("shape", "Shape", crate::icons::Icon::Rectangle, false),
        ("text", "Text", crate::icons::Icon::Type, false),
    ] {
        let g = ui.painter().layout_no_wrap(label.to_string(), crate::theme::Tokens::medium(12.0), t.text);
        let w = g.size().x + 32.0;
        let r = egui::Rect::from_min_size(pos2(x - w, cy - 12.0), vec2(w, 24.0));
        x -= w + 4.0;
        if r.min.x < strip.min.x + 90.0 {
            break; // no room: the narrowest layouts keep only what fits
        }
        let resp = ui.interact(r, egui::Id::new(("jbd-media-tool", id)), Sense::click());
        app.auto.add(&format!("media.{id}"), r, label);
        let hot = resp.hovered();
        if boxed {
            ui.painter().rect_stroke(r, 6.0, egui::Stroke::new(1.0, if hot { t.text_dim } else { t.field_border }), egui::StrokeKind::Inside);
        }
        if hot {
            ui.painter().rect_filled(r, 6.0, t.hover);
        }
        crate::icons::paint(
            ui.painter(),
            egui::Rect::from_center_size(pos2(r.min.x + 13.0, cy), vec2(13.0, 13.0)),
            icon,
            if hot { t.text } else { t.text_dim },
        );
        ui.painter().galley(pos2(r.min.x + 24.0, cy - g.size().y / 2.0), g, if hot || boxed { t.text } else { t.text_dim });
        let _ = Align2::LEFT_CENTER;
        if resp.clicked() {
            let r = match id {
                "import" => crate::menus::invoke(app, &ctx, "file.import", serde_json::json!({})),
                "text" => crate::menus::invoke(app, &ctx, "graphics.newText", serde_json::json!({"text": "Your text", "newClip": true})),
                "shape" => crate::menus::invoke(app, &ctx, "graphics.newRectangle", serde_json::json!({})),
                _ => {
                    app.show_panel(crate::dock::PanelKind::EssentialGraphics);
                    Ok(serde_json::Value::Null)
                }
            };
            if let Err(e) = r {
                app.ui.status = e;
            }
        }
    }
}

/// The first JBD's badge over the preview: "1920×1080 · 30 fps".
pub fn preview_badge(app: &FilmcraftApp, ui: &egui::Ui, content: egui::Rect) {
    use egui::{Align2, pos2, vec2};
    let Some(q) = app.session.active_sequence() else { return };
    let t = app.tokens;
    let rate = q.settings.frame_rate;
    let fps = rate.num as f64 / rate.den.max(1) as f64;
    let fps = if (fps - fps.round()).abs() < 0.01 { format!("{:.0}", fps) } else { format!("{fps:.3}") };
    let text = format!("{}×{} · {} fps", q.settings.width, q.settings.height, fps);
    let g = ui.painter().layout_no_wrap(text, crate::theme::Tokens::mono(11.5), t.text_dim);
    let r = egui::Rect::from_min_size(pos2(content.max.x - g.size().x - 30.0, content.min.y + 10.0), g.size() + vec2(16.0, 8.0));
    ui.painter().rect_filled(r, 6.0, egui::Color32::from_black_alpha(170));
    ui.painter().rect_stroke(r, 6.0, egui::Stroke::new(1.0, t.separator), egui::StrokeKind::Inside);
    ui.painter().galley(r.center() - g.size() / 2.0, g, t.text_dim);
    let _ = Align2::CENTER_CENTER;
}

/// Mac key names in labels as Windows writes them: "+ Cmd" → "+ Ctrl", "Opt" → "Alt".
pub fn platform_keys(s: &str) -> String {
    if cfg!(target_os = "macos") {
        return s.to_string();
    }
    s.replace("Cmd", "Ctrl").replace("Opt,", "Alt,").replace("+ Opt", "+ Alt").replace("Option", "Alt")
}

/// Open one of JBD's jabrod.com pages in the browser.
pub fn open_page(ctx: &egui::Context, page: Page) {
    ctx.open_url(egui::OpenUrl::new_tab(page.url()));
}

/// Runs before the editor each frame. Returns true when a JBD screen (splash, sign-in) covers the
/// window this frame, so the editor isn't drawn.
pub fn gate(app: &mut FilmcraftApp, ui: &mut egui::Ui) -> bool {
    let Some(backend) = app.jbd.backend.clone() else { return false };
    let ctx = ui.ctx().clone();
    if !app.jbd.repaint_hooked {
        let c = ctx.clone();
        backend.set_repaint(Arc::new(move || c.request_repaint()));
        app.jbd.repaint_hooked = true;
    }
    let status = backend.status();
    // the plan's export limits (the engine enforces them for every route an export can take)
    app.session.plan = if status.is_pro() { filmcraft_engine::Plan::Unrestricted } else { filmcraft_engine::Plan::Free };
    // back from the browser (or anywhere): ask jabrod.com again, so an upgrade shows at once
    let focused = ctx.input(|i| i.focused);
    if focused && app.jbd.was_focused == Some(false) {
        backend.check();
    }
    app.jbd.was_focused = Some(focused);
    app.jbd.status = status.clone();

    if app.jbd.splash.is_some() {
        if splash::show(app, ui, &status) {
            app.jbd.splash = None;
        } else {
            return true;
        }
    }
    if status.phase != Phase::SignedIn {
        app.jbd.menu_open = false;
        sign_in::show(app, ui, &backend, &status);
        return true;
    }
    false
}

/// On the Export screen and in Quick Export: what JBD Free does to exports, with an Upgrade button.
/// Nothing on Pro (`free` = [`JbdState::free_limits`]). Returns the Upgrade button's response.
pub fn free_export_notice(ui: &mut egui::Ui, free: bool, t: &crate::theme::Tokens) -> Option<egui::Response> {
    if !free {
        return None;
    }
    let mut upgrade = None;
    egui::Frame::new().fill(t.field_bg).stroke(egui::Stroke::new(1.0, t.separator)).corner_radius(8.0).inner_margin(egui::Margin::symmetric(10, 8)).show(
        ui,
        |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("JBD Free").font(crate::theme::Tokens::semibold(12.0)).color(t.text));
                ui.label(egui::RichText::new("exports up to 720p with a small “Made with JBD” mark.").color(t.text_dim));
                upgrade = Some(style::small_primary_button(ui, "Upgrade to Pro", t));
            });
        },
    );
    if let Some(r) = &upgrade
        && r.clicked()
    {
        open_page(ui.ctx(), Page::Upgrade);
    }
    upgrade
}
