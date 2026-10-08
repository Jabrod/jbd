//! The launch screen, like the first JBD's: the 3D logo cube spins in and lands in the logo's
//! exact pose while JBD starts, one visible step at a time. As the steps pass, the cube's three
//! faces are "covered" with their designs: the film strip, then the timeline, then the play button.
//!
//! It stays at least [`MIN_SHOW`] seconds so the landing can finish (a click skips it after a
//! moment) and never ends before the saved Jabrod account has been read, so the sign-in screen or
//! the editor comes next without a flash.
//!
//! TO CHANGE THE STEPS: edit [`STEPS`] (label and when it starts, in seconds).

use egui::{Align2, Color32, Rect, Sense, pos2, vec2};
use jbd_account::{AccountStatus, Phase};

use super::cube::{self, ISO_PITCH, ISO_YAW, Pose, Reveal};
use super::style;
use crate::FilmcraftApp;
use crate::theme::Tokens;

/// The shortest time the screen shows (s).
pub const MIN_SHOW: f64 = 2.3;
/// How long the landing spin takes (s).
const LAND: f64 = 1.9;
/// When each face's design starts to wipe on (s), and how long the wipe takes.
const FACE_TIMES: [f64; 3] = [0.45, 0.95, 1.45];
const WIPE: f64 = 0.35;

/// What JBD is doing at launch, and when each line appears (s after the splash first shows).
pub const STEPS: [(&str, f64); 6] = [
    ("Reading your preferences", 0.0),
    ("Loading your account", 0.3),
    ("Starting the video engine", 0.7),
    ("Preparing the GPU", 1.1),
    ("Testing video encoders", 1.5),
    ("Opening JBD", 1.95),
];

/// The splash's clock.
#[derive(Default)]
pub struct Splash {
    start: Option<f64>,
}

/// The cube's pose at `t` seconds: starts smaller, turned 270° back and tilted further, and
/// eases into the logo pose (the first JBD's landing).
pub fn landing_pose(t: f64) -> Pose {
    let e = style::ease_out((t / LAND) as f32);
    Pose { pitch: ISO_PITCH + 12.0 * (1.0 - e), yaw: ISO_YAW - 270.0 * (1.0 - e), scale: 0.62 + 0.38 * e }
}

/// How far each face is covered at `t`.
pub fn reveal_at(t: f64) -> Reveal {
    let w = |start: f64| (((t - start) / WIPE).clamp(0.0, 1.0)) as f32;
    Reveal { film: w(FACE_TIMES[0]), timeline: w(FACE_TIMES[1]), play: w(FACE_TIMES[2]) }
}

/// Draw the splash. Returns true when it has finished (the caller then drops it).
pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, status: &AccountStatus) -> bool {
    let ctx = ui.ctx().clone();
    let now = ctx.input(|i| i.time);
    let Some(splash) = app.jbd.splash.as_mut() else { return true };
    let start = *splash.start.get_or_insert(now);
    let t = now - start;
    let t_tokens = app.tokens;
    let rect = ui.max_rect();
    let resp = ui.interact(rect, egui::Id::new("jbd-splash"), Sense::click_and_drag());
    if resp.drag_started() {
        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    app.auto.add("jbd.splash", rect, "JBD is starting");
    let account_read = status.phase != Phase::Starting;
    let done = account_read && (t >= MIN_SHOW || (resp.clicked() && t > 0.4));
    paint(ui.painter(), rect, t, &t_tokens);
    if !done {
        ctx.request_repaint();
    }
    done
}

/// Paint the splash at time `t` into `rect` (also used by the launch-video renderer). Laid out
/// like the first JBD's: the cube with a soft shadow, the wordmark, the tagline, then the progress
/// bar with the current step on the left and the percentage on the right.
pub fn paint(painter: &egui::Painter, rect: Rect, t: f64, tk: &Tokens) {
    painter.rect_filled(rect, 0.0, tk.app_bg);
    let edge = (rect.height() * 0.105).clamp(56.0, 124.0);
    let block_h = edge * 1.6 + edge * 1.05 + 30.0 + 64.0 + 40.0;
    let top = rect.center().y - block_h / 2.0;
    let center = pos2(rect.center().x, top + edge * 0.8);
    // the shadow: stacked soft ellipses (darker in the middle), under the landing cube
    let land = style::ease_out((t / LAND) as f32);
    let shadow_c = pos2(center.x, center.y + edge * 1.02);
    for i in 0..10 {
        let k = 1.0 - i as f32 / 10.0;
        let r = vec2(edge * (0.55 + 0.75 * k), edge * (0.07 + 0.11 * k));
        painter.add(egui::Shape::ellipse_filled(shadow_c, r, Color32::from_black_alpha((10.0 * land) as u8)));
    }
    cube::paint(painter, center, edge, landing_pose(t), reveal_at(t), 1.0);
    // the wordmark and tagline fade up once the cube has nearly landed
    let fade = (((t - 0.6) / 0.6).clamp(0.0, 1.0)) as f32;
    let rise = (1.0 - style::ease_out(fade)) * 10.0;
    let ink = |c: Color32| c.gamma_multiply(fade);
    let word_y = center.y + edge * 1.25 + rise;
    let word = painter.text(pos2(center.x, word_y), Align2::CENTER_TOP, "JBD", Tokens::display(edge * 0.92), ink(tk.text));
    style::spaced(painter, pos2(center.x, word.max.y + 16.0), Align2::CENTER_TOP, "AI VIDEO EDITOR BY JABROD", Tokens::medium(13.0), ink(tk.text_dim), 5.0);
    // the progress bar, the current step and the percentage
    let step = STEPS.iter().rposition(|(_, at)| t >= *at).unwrap_or(0);
    let label = STEPS.get(step).map_or("", |s| s.0);
    let bar_w = 360.0f32.min(rect.width() * 0.6);
    let bar = Rect::from_center_size(pos2(center.x, word.max.y + 84.0), vec2(bar_w, 4.0));
    let progress = (t / MIN_SHOW).clamp(0.0, 1.0) as f32;
    style::meter(painter, bar, progress, tk, None);
    painter.text(pos2(bar.min.x, bar.max.y + 14.0), Align2::LEFT_TOP, label, Tokens::ui(13.0), tk.text_dim);
    painter.text(pos2(bar.max.x, bar.max.y + 14.0), Align2::RIGHT_TOP, format!("{:.0}%", progress * 100.0), Tokens::ui(12.0), tk.text_faint);
    painter.text(
        pos2(center.x, rect.max.y - 22.0),
        Align2::CENTER_BOTTOM,
        format!("Version {}  ·  © Jabrod", env!("CARGO_PKG_VERSION")),
        Tokens::ui(11.0),
        tk.text_faint,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cube_lands_in_the_logo_pose() {
        let p = landing_pose(LAND);
        assert!((p.pitch - ISO_PITCH).abs() < 1e-3 && (p.yaw - ISO_YAW).abs() < 1e-3 && (p.scale - 1.0).abs() < 1e-3);
        let p0 = landing_pose(0.0);
        assert!((p0.yaw - (ISO_YAW - 270.0)).abs() < 1e-3 && (p0.scale - 0.62).abs() < 1e-3);
    }

    #[test]
    fn faces_arrive_one_after_another() {
        assert_eq!(reveal_at(0.0), Reveal::NONE);
        let r = reveal_at(1.0);
        assert!(r.film >= 1.0 && r.timeline > 0.0 && r.timeline < 1.0 && r.play == 0.0);
        assert_eq!(reveal_at(MIN_SHOW), Reveal::ALL);
    }
}
