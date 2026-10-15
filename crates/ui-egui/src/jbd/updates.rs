//! Updates, What's New and Report a Problem (Help menu and the title bar's "Update" chip).
//!
//! - **Update check**: at launch and every few hours JBD asks jabrod.com for the latest release
//!   (`/api/releases/jbd`, a background thread). When it is newer than this build, an "Update"
//!   chip appears in the title bar. "Update" opens the Microsoft Store page for copies installed
//!   from the Store (the Store installs it), and the installer download for everyone else.
//! - **What's New**: the changelog bundled into the app (`docs/jbd/CHANGELOG.md`, so it works
//!   offline). It opens by itself once, the first time a new version starts.
//! - **Report a Problem**: a short form sent to jabrod.com as a support ticket (it is also
//!   emailed to contact@jabrod.com, and the answer comes back by email), with the end of JBD's log
//!   if the person allows it. Signed out, it opens an email instead.
//!
//! To change how often JBD checks: [`CHECK_EVERY`]. The changelog text: docs/jbd/CHANGELOG.md.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use egui::{Align, Layout, RichText};
use jbd_account::{Release, Report, is_newer};

use crate::FilmcraftApp;
use crate::theme::Tokens;

/// This build's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// How often a running JBD checks for a new version (seconds of UI time).
const CHECK_EVERY: f64 = 6.0 * 3600.0;
/// JBD's Microsoft Store listing.
const STORE_ID: &str = "9P0KS6185QSX";
/// The changelog, bundled into the app.
pub const CHANGELOG: &str = include_str!("../../../../docs/jbd/CHANGELOG.md");

/// Which window is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialog {
    WhatsNew,
    Update,
    Report,
}

/// What the person is typing into Report a Problem.
#[derive(Clone, Debug)]
pub struct Draft {
    pub topic: &'static str,
    pub subject: String,
    pub message: String,
    pub include_log: bool,
}

impl Default for Draft {
    fn default() -> Self {
        Draft { topic: "bug", subject: String::new(), message: String::new(), include_log: true }
    }
}

/// The update check, the dialogs and a report being sent.
#[derive(Default)]
pub struct Updates {
    latest: Arc<Mutex<Option<Release>>>,
    checking: Arc<AtomicBool>,
    /// The last check's error (shown only after "Check for Updates…").
    error: Arc<Mutex<Option<String>>>,
    last_check: f64,
    pub dialog: Option<Dialog>,
    pub draft: Draft,
    sending: Arc<AtomicBool>,
    sent: Arc<Mutex<Option<Result<u64, String>>>>,
    whats_new_checked: bool,
    /// What's New is waiting for the launch animation to finish.
    whats_new_pending: bool,
    /// Checking and What's New run only in a real launch (not in tests or tools, which must not
    /// touch the network or the person's data folder). The dialogs work everywhere.
    pub active: bool,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Updates {
    /// Tell JBD about a release as if jabrod.com had answered (tests, tools).
    pub fn offer(&self, r: Release) {
        *lock(&self.latest) = Some(r);
    }

    /// A newer release than this build, when there is one.
    pub fn available(&self) -> Option<Release> {
        lock(&self.latest).clone().filter(|r| is_newer(&r.version, VERSION))
    }
}

/// Was this copy installed from the Microsoft Store? (Store apps run from WindowsApps.)
fn from_store() -> bool {
    std::env::current_exe().ok().is_some_and(|p| p.to_string_lossy().to_ascii_lowercase().contains("\\windowsapps\\"))
}

/// Ask jabrod.com for the latest release, in the background.
pub fn check_now(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let u = &mut app.jbd.updates;
    u.last_check = ctx.input(|i| i.time).max(0.001);
    #[cfg(not(target_arch = "wasm32"))]
    {
        if u.checking.swap(true, Ordering::Relaxed) {
            return;
        }
        let (latest, checking, error, ctx) = (u.latest.clone(), u.checking.clone(), u.error.clone(), ctx.clone());
        let spawned = std::thread::Builder::new().name("jbd-update-check".into()).spawn(move || {
            let got = std::panic::catch_unwind(jbd_account::native::latest_release).unwrap_or_else(|_| Err("the check failed".into()));
            match got {
                Ok(r) => {
                    *lock(&latest) = Some(r);
                    *lock(&error) = None;
                }
                Err(e) => *lock(&error) = Some(e),
            }
            checking.store(false, Ordering::Relaxed);
            ctx.request_repaint();
        });
        if spawned.is_err() {
            u.checking.store(false, Ordering::Relaxed);
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = ctx;
    }
}

/// Each frame: the periodic check, What's New after an update, a report's answer, the windows.
pub fn tick(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let now = ctx.input(|i| i.time);
    if app.jbd.updates.active {
        let u = &app.jbd.updates;
        if u.last_check == 0.0 || now - u.last_check > CHECK_EVERY {
            check_now(app, ctx);
        }
        if !app.jbd.updates.whats_new_checked {
            app.jbd.updates.whats_new_checked = true;
            app.jbd.updates.whats_new_pending = first_run_of_this_version();
        }
        // after the launch animation, so it isn't hidden behind it
        if app.jbd.updates.whats_new_pending && app.jbd.splash.is_none() && app.jbd.updates.dialog.is_none() {
            app.jbd.updates.whats_new_pending = false;
            app.jbd.updates.dialog = Some(Dialog::WhatsNew);
        }
    }
    if let Some(result) = lock(&app.jbd.updates.sent).take() {
        match result {
            Ok(id) => {
                app.jbd.updates.dialog = None;
                app.jbd.updates.draft = Draft::default();
                app.ui.status = format!("Report #{id} sent. We'll answer by email.");
            }
            Err(e) => app.ui.status = format!("Couldn't send the report: {e}. Try again, or email contact@jabrod.com."),
        }
    }
    show(app, ctx);
}

/// True once per new version: the first launch after installing or updating (remembered in
/// JBD's data folder). A brand-new install doesn't count as "new", so What's New stays quiet.
fn first_run_of_this_version() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(dir) = filmcraft_engine::autosave::default_data_dir() else { return false };
        let file = dir.join("last-version.txt");
        let before = std::fs::read_to_string(&file).ok().map(|s| s.trim().to_string());
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(&file, VERSION);
        matches!(before, Some(v) if v != VERSION)
    }
    #[cfg(target_arch = "wasm32")]
    false
}

/// "Update": the Store page (Store copies) or the installer download.
pub fn install(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(r) = app.jbd.updates.available() else { return };
    let url = if from_store() {
        format!("ms-windows-store://pdp/?productid={STORE_ID}")
    } else if !r.download_url.is_empty() {
        r.download_url.clone()
    } else {
        r.page_url.clone()
    };
    ctx.open_url(egui::OpenUrl::new_tab(url));
    app.ui.status = if from_store() {
        "Opened JBD in the Microsoft Store: choose Update there.".into()
    } else {
        format!("Downloading JBD {}: run the installer when it finishes.", r.version)
    };
}

/// The title bar's "Update" chip, right-aligned ending at `right`. Returns its rect when shown.
pub fn chip(app: &mut FilmcraftApp, ui: &mut egui::Ui, right: f32, cy: f32) -> Option<egui::Rect> {
    let r = app.jbd.updates.available()?;
    let t = app.tokens;
    let p = ui.painter();
    let g = p.layout_no_wrap("Update".to_string(), Tokens::semibold(11.5), t.text);
    let rect = egui::Rect::from_min_max(egui::pos2(right - g.size().x - 30.0, cy - 12.0), egui::pos2(right, cy + 12.0));
    let resp = ui.interact(rect, egui::Id::new("jbd-update-chip"), egui::Sense::click()).on_hover_text(format!("JBD {} is available", r.version));
    app.auto.add("titlebar.update", rect, "Update available");
    p.rect_filled(rect, 12.0, if resp.hovered() { t.hover } else { t.field_bg });
    p.rect_stroke(rect, 12.0, egui::Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
    // the logo blue marks what needs to be seen
    p.circle_filled(egui::pos2(rect.min.x + 12.0, cy), 3.5, super::style::BLUE);
    p.galley(egui::pos2(rect.min.x + 21.0, cy - g.size().y / 2.0), g, t.text);
    if resp.clicked() {
        app.jbd.updates.dialog = Some(Dialog::Update);
    }
    Some(rect)
}

/// The JBD log's end (the newest file in the Logs folder), for a report.
fn log_tail() -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let dir = filmcraft_engine::autosave::default_data_dir()?.join("Logs");
        let newest = std::fs::read_dir(dir).ok()?.flatten().filter(|e| e.path().is_file()).max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())?;
        let text = std::fs::read_to_string(newest.path()).ok()?;
        let tail: String = text.chars().rev().take(20_000).collect::<Vec<_>>().into_iter().rev().collect();
        Some(tail)
    }
    #[cfg(target_arch = "wasm32")]
    None
}

/// Send the report: to jabrod.com when signed in, else as an email.
fn send(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let d = app.jbd.updates.draft.clone();
    let subject = if d.subject.trim().is_empty() { d.message.lines().next().unwrap_or("").chars().take(80).collect() } else { d.subject.trim().to_string() };
    let context = vec![
        ("App".to_string(), format!("JBD {VERSION}")),
        ("Installed from".to_string(), if from_store() { "Microsoft Store" } else { "jabrod.com installer" }.to_string()),
        ("System".to_string(), format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)),
    ];
    let report = Report {
        topic: d.topic.to_string(),
        subject: subject.clone(),
        message: d.message.trim().to_string(),
        context,
        log: if d.include_log { log_tail() } else { None },
    };
    let signed_in = app.jbd.status.is_signed_in();
    match app.jbd.backend.clone().filter(|_| signed_in) {
        #[cfg(not(target_arch = "wasm32"))]
        Some(backend) => {
            let u = &app.jbd.updates;
            if u.sending.swap(true, Ordering::Relaxed) {
                return;
            }
            let (sending, sent, ctx) = (u.sending.clone(), u.sent.clone(), ctx.clone());
            let _ = std::thread::Builder::new().name("jbd-report".into()).spawn(move || {
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    backend.access_token().and_then(|t| jbd_account::native::send_report(&t, &report))
                }))
                .unwrap_or_else(|_| Err("sending failed".into()));
                *lock(&sent) = Some(r);
                sending.store(false, Ordering::Relaxed);
                ctx.request_repaint();
            });
        }
        _ => {
            // signed out: the person's own email app, with everything filled in
            let body = format!("{}\n\n--\n{}", report.message, report.context.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\n"));
            let url = format!("mailto:contact@jabrod.com?subject={}&body={}", encode(&format!("JBD: {subject}")), encode(&body));
            ctx.open_url(egui::OpenUrl::new_tab(url));
            app.jbd.updates.dialog = None;
            app.ui.status = "Opened an email to contact@jabrod.com.".into();
        }
    }
}

/// Percent-encode for a mailto link.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(which) = app.jbd.updates.dialog else { return };
    let t = app.tokens;
    let mut open = true;
    let title = match which {
        Dialog::WhatsNew => "What's New in JBD",
        Dialog::Update => "Updates",
        Dialog::Report => "Report a Problem",
    };
    let mut action: Option<&str> = None;
    egui::Window::new(title)
        .id(egui::Id::new(("jbd-updates", title)))
        .open(&mut open)
        .collapsible(false)
        .resizable(which == Dialog::WhatsNew)
        .default_width(520.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            match which {
                Dialog::WhatsNew => {
                    ui.label(RichText::new(format!("You're on JBD {VERSION}")).size(11.5).color(t.text_faint));
                    egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                        super::director::markdown(ui, CHANGELOG, &t);
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let b = ui.button("Full changelog on jabrod.com");
                        app.auto.add("updates.changelogOnline", b.rect, "Full changelog");
                        if b.clicked() {
                            action = Some("changelog");
                        }
                    });
                }
                Dialog::Update => {
                    let checking = app.jbd.updates.checking.load(Ordering::Relaxed);
                    match app.jbd.updates.available() {
                        Some(r) => {
                            ui.label(RichText::new(format!("JBD {} is available", r.version)).font(Tokens::semibold(15.0)).color(t.text));
                            ui.label(RichText::new(format!("You have {VERSION}.")).size(11.5).color(t.text_dim));
                            ui.add_space(4.0);
                            for n in r.notes.iter().take(8) {
                                ui.add(egui::Label::new(RichText::new(format!("•  {n}")).size(12.0).color(t.text_dim)).wrap());
                            }
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                let label = if from_store() { "Update in Microsoft Store" } else { "Download the update" };
                                let b = super::style::small_primary_button(ui, label, &t);
                                app.auto.add("updates.install", b.rect, label);
                                if b.clicked() {
                                    action = Some("install");
                                }
                                if ui.button("What's new").clicked() {
                                    action = Some("whatsnew");
                                }
                            });
                        }
                        None if checking => {
                            ui.horizontal(|ui| {
                                super::cube::loader(ui, 14.0);
                                ui.label(RichText::new("Checking for updates…").size(12.0).color(t.text_dim));
                            });
                        }
                        None => {
                            match lock(&app.jbd.updates.error).clone() {
                                Some(e) => {
                                    ui.label(RichText::new("Couldn't check for updates.").size(12.5).color(t.text));
                                    ui.label(RichText::new(format!("{e}. Check your internet connection, then try again.")).size(11.5).color(t.text_dim));
                                }
                                None => {
                                    ui.label(RichText::new("JBD is up to date.").font(Tokens::semibold(14.0)).color(t.text));
                                    ui.label(RichText::new(format!("You have the latest version, {VERSION}.")).size(11.5).color(t.text_dim));
                                }
                            }
                            ui.add_space(6.0);
                            if ui.button("Check again").clicked() {
                                action = Some("check");
                            }
                        }
                    }
                }
                Dialog::Report => {
                    let sending = app.jbd.updates.sending.load(Ordering::Relaxed);
                    let signed_in = app.jbd.status.is_signed_in();
                    ui.label(
                        RichText::new("Tell us what went wrong or what would help. We read every report and answer by email.").size(11.5).color(t.text_dim),
                    );
                    ui.add_space(4.0);
                    let d = &mut app.jbd.updates.draft;
                    ui.horizontal(|ui| {
                        for (id, label) in [("bug", "Something's wrong"), ("idea", "An idea"), ("question", "A question")] {
                            ui.selectable_value(&mut d.topic, id, label);
                        }
                    });
                    ui.add_space(2.0);
                    let subject = ui.add(egui::TextEdit::singleline(&mut d.subject).hint_text("Short summary (optional)").desired_width(f32::INFINITY));
                    let message = ui.add(
                        egui::TextEdit::multiline(&mut d.message)
                            .hint_text("What happened? What did you expect? Steps to make it happen again help a lot.")
                            .desired_rows(7)
                            .desired_width(f32::INFINITY),
                    );
                    ui.checkbox(&mut d.include_log, "Include JBD's recent log (no project files or media)");
                    let ok = d.message.trim().chars().count() >= 10;
                    let (subject_rect, message_rect) = (subject.rect, message.rect);
                    ui.add_space(4.0);
                    // one row (a right-to-left layout on its own would take all the height left)
                    ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 28.0), Layout::right_to_left(Align::Center), |ui| {
                        let label = if signed_in { "Send report" } else { "Email report" };
                        let b = ui.add_enabled(ok && !sending, egui::Button::new(label));
                        app.auto.add("updates.report.send", b.rect, label);
                        if b.clicked() {
                            action = Some("send");
                        }
                        if sending {
                            super::cube::loader(ui, 14.0);
                        }
                        if !ok {
                            ui.label(RichText::new("A few words more (10 characters)").size(11.0).color(t.text_faint));
                        }
                    });
                    app.auto.add("updates.report.subject", subject_rect, "Report subject");
                    app.auto.add("updates.report.message", message_rect, "Report message");
                }
            }
        });
    if !open {
        app.jbd.updates.dialog = None;
    }
    match action {
        Some("install") => install(app, ctx),
        Some("whatsnew") => app.jbd.updates.dialog = Some(Dialog::WhatsNew),
        Some("check") => check_now(app, ctx),
        Some("changelog") => ctx.open_url(egui::OpenUrl::new_tab(format!("{}/jbd/changelog", jbd_account::platform_url()))),
        Some("send") => send(app, ctx),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mailto_text_is_encoded() {
        assert_eq!(encode("a b&c\n"), "a%20b%26c%0A");
    }

    #[test]
    fn the_changelog_names_this_version() {
        assert!(CHANGELOG.contains(&format!("## {VERSION}")), "docs/jbd/CHANGELOG.md needs a \"## {VERSION}\" section");
    }
}
