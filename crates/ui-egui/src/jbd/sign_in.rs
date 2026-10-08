//! The screen JBD shows until you're signed in with a Jabrod account (like the first JBD's):
//!
//!   signed out → "Sign in with Jabrod" (opens jabrod.com in your browser)
//!   waiting    → the turning cube, "Finish signing in in your browser", Cancel and Copy link
//!
//! The browser part and the tokens are in the `jbd-account` crate (`native.rs`); this screen only
//! shows the account status and calls it.
//!
//! Automation ids: `jbd.signIn`, `jbd.cancelSignIn`, `jbd.copySignInLink`.

use std::sync::Arc;

use egui::{Align2, Rect, Sense, pos2, vec2};
use jbd_account::{AccountBackend, AccountStatus, Phase};

use super::cube;
use super::style;
use crate::FilmcraftApp;
use crate::theme::Tokens;

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, backend: &Arc<dyn AccountBackend>, status: &AccountStatus) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let rect = ui.max_rect();
    let p = ui.painter().clone();
    p.rect_filled(rect, 0.0, t.app_bg);
    // the empty background moves the window (there's no title bar over this screen)
    let bg = ui.interact(rect, egui::Id::new("jbd-sign-in-bg"), Sense::click_and_drag());
    if bg.drag_started() {
        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }

    let cx = rect.center().x;
    let top = rect.min.y + rect.height() * 0.30;
    let edge = 52.0;
    cube::paint(&p, pos2(cx, top), edge, cube::Pose::LOGO, cube::Reveal::ALL, 1.0);
    p.text(pos2(cx, top + edge * 1.25), Align2::CENTER_TOP, "JBD", Tokens::display(40.0), t.text);
    style::spaced(&p, pos2(cx, top + edge * 1.25 + 52.0), Align2::CENTER_TOP, "AI VIDEO EDITOR BY JABROD", Tokens::medium(11.0), t.text_dim, 3.2);

    let col_top = top + edge * 1.25 + 110.0;
    let width = 320.0f32.min(rect.width() - 48.0);
    match status.phase {
        Phase::WaitingForBrowser => {
            cube::paint_loader(&p, pos2(cx, col_top + 14.0), 26.0, ctx.input(|i| i.time));
            ctx.request_repaint();
            p.text(pos2(cx, col_top + 40.0), Align2::CENTER_TOP, "Finish signing in in your browser", Tokens::medium(13.0), t.text);
            p.text(pos2(cx, col_top + 62.0), Align2::CENTER_TOP, "JBD continues on its own when you're done.", Tokens::ui(12.0), t.text_dim);
            let row = col_top + 92.0;
            let cancel = Rect::from_min_size(pos2(cx - 104.0, row), vec2(100.0, 30.0));
            let copy = Rect::from_min_size(pos2(cx + 4.0, row), vec2(100.0, 30.0));
            app.auto.add("jbd.cancelSignIn", cancel, "Cancel");
            app.auto.add("jbd.copySignInLink", copy, "Copy link");
            if style::ghost_button_at(ui, cancel, "cancel", "Cancel", &t).clicked() {
                backend.cancel_sign_in();
            }
            if style::ghost_button_at(ui, copy, "copy", "Copy link", &t).clicked()
                && let Some(url) = &app.jbd.sign_in_url
            {
                ctx.copy_text(url.clone());
                app.toast = Some(("Sign-in link copied. Paste it into your browser.".into(), ctx.input(|i| i.time)));
            }
        }
        _ => {
            let button = Rect::from_center_size(pos2(cx, col_top + 20.0), vec2(width, 42.0));
            app.auto.add("jbd.signIn", button, "Sign in with Jabrod");
            if style::primary_button_at(ui, button, "sign-in", "Sign in with Jabrod").clicked() {
                match backend.sign_in() {
                    Ok(url) => {
                        ctx.open_url(egui::OpenUrl::new_tab(url.clone()));
                        app.jbd.sign_in_url = Some(url);
                    }
                    Err(e) => app.toast = Some((e, ctx.input(|i| i.time))),
                }
            }
            let mut y = button.max.y + 14.0;
            if let Some(msg) = &status.message {
                y += style::centered_paragraph(&p, pos2(cx, y), msg, Tokens::ui(12.0), t.danger, width) + 10.0;
            }
            let note = "Opens jabrod.com in your browser. Your account is free, and new accounts get JBD Pro free for 7 days.";
            style::centered_paragraph(&p, pos2(cx, y), note, Tokens::ui(11.5), t.text_faint, width);
        }
    }
    p.text(pos2(cx, rect.max.y - 22.0), Align2::CENTER_BOTTOM, "© Jabrod", Tokens::ui(11.0), t.text_faint);
    show_toast(app, ui);
}

/// Toasts still show over the sign-in screen (the editor, which normally draws them, is hidden).
fn show_toast(app: &mut FilmcraftApp, ui: &egui::Ui) {
    let Some((msg, at)) = app.toast.clone() else { return };
    let now = ui.ctx().input(|i| i.time);
    if now - at > 4.0 {
        app.toast = None;
        return;
    }
    let t = app.tokens;
    let rect = ui.max_rect();
    let g = ui.painter().layout_no_wrap(msg, Tokens::ui(12.0), t.text);
    let r = Rect::from_center_size(pos2(rect.center().x, rect.max.y - 64.0), g.size() + vec2(24.0, 14.0));
    ui.painter().rect_filled(r, 8.0, t.field_bg);
    ui.painter().rect_stroke(r, 8.0, egui::Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
    ui.painter().galley(r.center() - g.size() / 2.0, g, t.text);
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
}
