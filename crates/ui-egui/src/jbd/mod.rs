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
//! [`style`] (drawing helpers shared by these screens).

pub mod account;
pub mod connections;
pub mod cube;
pub mod director;
pub mod director_history;
pub mod director_run;
pub mod settings;
pub mod sign_in;
pub mod splash;
pub mod style;
pub mod titlebar;
pub mod updates;
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
    /// JBD Settings is open, at this section, with this preferences schema (re-read after changes).
    pub settings_open: bool,
    pub settings_section: String,
    pub settings_schema: Option<serde_json::Value>,
    /// A start-screen choice waiting for the "unsaved changes" answer.
    pub welcome_pending: Option<welcome::Pending>,
    /// Window focus last frame (regaining it re-checks the plan, so a purchase shows at once).
    was_focused: Option<bool>,
    /// Settings ▸ Connections and Connectors (`connections.rs`).
    pub connections: connections::State,
    /// The AI Director: its conversation, its thread, its pickers (`director_run.rs`).
    pub director: director_run::Director,
    /// Update check, What's New and Report a Problem (`updates.rs`).
    pub updates: updates::Updates,
    repaint_hooked: bool,
}

impl JbdState {
    pub fn new(backend: Arc<dyn AccountBackend>, splash: bool) -> Self {
        let mut s = JbdState { status: backend.status(), backend: Some(backend), splash: splash.then(splash::Splash::default), ..Default::default() };
        // a real launch (with the animation) checks for updates; tests and tools don't
        s.updates.active = splash;
        s
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
    // any layout button leaves Director view
    a.focus = false;
    match which {
        "media" => a.media = !a.media,
        "side" => a.side = !a.side,
        _ => a.timeline = !a.timeline,
    }
    app.ui.workspace = "JBD".into();
    app.ui.dock = jbd_workspace(app.jbd.areas);
}

/// Director view: only the Preview and the Director (half the window each), to watch the
/// Director work. Turning it off brings the normal layout back.
pub fn director_view(app: &mut FilmcraftApp, on: bool) {
    app.jbd.areas = if on { JbdAreas { media: false, side: true, timeline: false, focus: true } } else { JbdAreas::default() };
    app.ui.workspace = "JBD".into();
    app.ui.dock = jbd_workspace(app.jbd.areas);
}

/// The panels JBD's layout uses, in the order the + menu lists them.
const JBD_PANELS: [crate::dock::PanelKind; 8] = {
    use crate::dock::PanelKind::*;
    [Properties, Text, Director, EffectControls, Effects, Project, Timeline, AudioMeters]
};

/// The tab strip's + was clicked: open the menu of closed panels by it.
pub fn open_add_panel_menu(ctx: &egui::Context, near: crate::dock::PanelKind, pos: egui::Pos2) {
    // the frame it opened in: that frame's click is the one on the +, not a click outside
    let opened = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("jbd-add-panel"), (near, pos, opened)));
}

/// The + menu: JBD's panels that are closed; picking one opens it in that group.
pub fn add_panel_menu(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let id = egui::Id::new("jbd-add-panel");
    let Some((near, pos, opened)) = ctx.data(|d| d.get_temp::<(crate::dock::PanelKind, egui::Pos2, u64)>(id)) else { return };
    let closed: Vec<crate::dock::PanelKind> = JBD_PANELS.iter().copied().filter(|p| !app.ui.dock.contains(*p)).collect();
    let t = app.tokens;
    let mut pick = None;
    let mut close = false;
    let area = egui::Area::new(id.with("area")).order(egui::Order::Foreground).fixed_pos(pos + egui::vec2(0.0, 4.0)).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_min_width(170.0);
            if closed.is_empty() {
                ui.label(egui::RichText::new("Every panel is open").size(12.0).color(t.text_dim));
            }
            for p in &closed {
                let b = ui.add(egui::Button::new(egui::RichText::new(p.jbd_title()).size(12.5)).frame(false).min_size(egui::vec2(160.0, 22.0)));
                app.auto.add(&format!("panel.addMenu.{}", p.id()), b.rect, p.jbd_title());
                if b.clicked() {
                    pick = Some(*p);
                }
            }
        });
    });
    // a click anywhere else closes it
    if ctx.cumulative_pass_nr() > opened && ctx.input(|i| i.pointer.any_click()) && !area.response.hovered() {
        close = true;
    }
    if let Some(p) = pick {
        app.ui.dock.open_near(p, near);
        app.ui.dock.activate(p);
        app.ui.focused = p;
        close = true;
    }
    if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        ctx.data_mut(|d| d.remove::<(crate::dock::PanelKind, egui::Pos2, u64)>(id));
    }
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
    connections::autostart(app);
    updates::tick(app, ui.ctx());
    settings::show(app, ui.ctx());
    welcome::confirm_window(app, ui.ctx());
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
            ensure_timeline(app);
            let r = match id {
                "import" => crate::menus::invoke(app, &ctx, "file.import", serde_json::json!({})),
                "text" => crate::menus::invoke(app, &ctx, "graphics.newText", serde_json::json!({"text": "Your text", "newClip": true})),
                "shape" => crate::menus::invoke(app, &ctx, "graphics.newRectangle", serde_json::json!({})),
                _ => {
                    // beside the Inspector, never on top of the Preview
                    use crate::dock::PanelKind;
                    if !app.jbd.areas.side {
                        toggle_area(app, "side");
                    }
                    app.ui.dock.open_near(PanelKind::EssentialGraphics, PanelKind::Properties);
                    app.ui.dock.activate(PanelKind::EssentialGraphics);
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
    if app.ui.mode == crate::state::Mode::Edit {
        ensure_timeline(app);
    }
    false
}

/// JBD's own meaning of some editor commands, for the menus, the shortcuts and the Media panel
/// (calls with parameters, from agents and the control channel, keep the engine's meaning):
/// - File ▸ Close Project asks about unsaved changes, closes and shows the start screen;
/// - File ▸ New Project (Ctrl+Alt+N) shows the start screen's templates.
///
/// Returns true when JBD handled `id`.
pub fn intercept(app: &mut FilmcraftApp, id: &str) -> bool {
    match id {
        "file.closeProject" | "file.closeAllProjects" => welcome::request(app, welcome::Pending::Close),
        "file.newProject" => app.ui.mode = crate::state::Mode::Import,
        _ => return false,
    }
    true
}

/// The shortcuts JBD's menus show for its own items (the editor's bindings for these keys mean
/// something else: Ctrl+N is New Timeline, Ctrl+, the full preferences). Runs before the
/// editor's shortcuts; a key matches only with exactly these modifiers.
pub fn shortcuts(app: &mut FilmcraftApp, ctx: &egui::Context) {
    use egui::{Key, Modifiers};
    const KEYS: [(Key, &str); 2] = [(Key::N, "jbd.home"), (Key::Comma, "jbd.settings")];
    let mut fire = Vec::new();
    ctx.input_mut(|i| {
        i.events.retain(|e| match e {
            egui::Event::Key { key, pressed, modifiers, .. } if modifiers.matches_exact(Modifiers::COMMAND) => match KEYS.iter().find(|(k, _)| k == key) {
                Some((_, id)) => {
                    if *pressed {
                        fire.push(*id);
                    }
                    false
                }
                None => true,
            },
            _ => true,
        });
    });
    for id in fire {
        titlebar::run(app, ctx, id);
    }
}

/// JBD always has a timeline open, as the first JBD did: Text, Shape, Motion, import and the
/// preview all work on it. When the project has none open (File ▸ New Project, closing the last
/// one, a project saved without one), open its first timeline, or make "Main" (1920×1080, 30 fps).
pub fn ensure_timeline(app: &mut FilmcraftApp) {
    if app.session.state.active_sequence.is_some() {
        return;
    }
    let first = app.session.project.items.iter().find(|(_, it)| matches!(it.kind, filmcraft_engine::project::ItemKind::Sequence(_))).map(|(id, _)| *id);
    let r = match first {
        Some(id) => app.session.execute("sequence.open", serde_json::json!({"item": id.0})),
        None => app.session.execute("file.newSequence", serde_json::json!({"name": "Main", "width": 1920, "height": 1080, "fps": 30.0})),
    };
    if let Err(e) = r {
        app.ui.status = e.to_string();
    }
}

/// Media ▸ double-click (the first JBD's behaviour): put the clip at the end of the timeline and
/// show it in the preview, instead of opening a Source monitor JBD's layout doesn't have.
pub fn add_to_timeline(app: &mut FilmcraftApp, item: u64) -> Result<(), String> {
    ensure_timeline(app);
    let end = app.session.active_sequence().map(|q| q.duration()).unwrap_or_default();
    app.session.set_playhead(end);
    app.session
        .execute(
            "clip.automateToSequence",
            serde_json::json!({"items": [item], "ordering": "selection", "placement": "sequentially", "method": "overwrite", "videoTransition": false, "audioTransition": false}),
        )
        .map_err(|e| e.to_string())?;
    app.session.set_playhead(end);
    app.ui.mode = crate::state::Mode::Edit;
    Ok(())
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
