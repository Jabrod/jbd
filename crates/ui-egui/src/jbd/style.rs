//! Small drawing helpers for JBD's own screens (splash, sign-in, account menu), so they share one
//! look: white primary buttons with black text (Jabrod), quiet ghost buttons, the logo gradient
//! (violet → blue → cyan) only for showing progress, letter-spaced small caps for taglines.

use egui::{Color32, Mesh, Pos2, Rect, Response, Sense, Stroke, pos2, vec2};

use crate::theme::Tokens;

/// The logo colours, left to right.
pub const VIOLET: Color32 = Color32::from_rgb(0x96, 0x44, 0xfa);
pub const BLUE: Color32 = Color32::from_rgb(0x3d, 0x5c, 0xff);
pub const CYAN: Color32 = Color32::from_rgb(0x00, 0xd4, 0xfa);
/// Primary buttons: near-white with black text.
pub const PRIMARY_BG: Color32 = Color32::from_rgb(0xfa, 0xfa, 0xfa);
pub const PRIMARY_BG_HOVER: Color32 = Color32::from_rgb(0xe4, 0xe4, 0xe7);
pub const PRIMARY_TEXT: Color32 = Color32::from_rgb(0x0b, 0x0b, 0x0c);

/// Fill `rect` with the logo gradient (violet → blue → cyan), rounded ends drawn as a plain bar.
pub fn gradient_bar(painter: &egui::Painter, rect: Rect, alpha: f32) {
    let a = |c: Color32| c.gamma_multiply(alpha.clamp(0.0, 1.0));
    let mut m = Mesh::default();
    let xs = [rect.min.x, rect.center().x, rect.max.x];
    let cs = [a(VIOLET), a(BLUE), a(CYAN)];
    for (x, c) in xs.iter().zip(cs) {
        m.colored_vertex(pos2(*x, rect.min.y), c);
        m.colored_vertex(pos2(*x, rect.max.y), c);
    }
    for i in 0..2u32 {
        let b = i * 2;
        m.add_triangle(b, b + 1, b + 3);
        m.add_triangle(b, b + 3, b + 2);
    }
    painter.add(m);
}

/// A progress / meter track with the gradient filled to `fraction`.
pub fn meter(painter: &egui::Painter, rect: Rect, fraction: f32, t: &Tokens, color: Option<Color32>) {
    painter.rect_filled(rect, rect.height() / 2.0, t.field_bg);
    let f = fraction.clamp(0.0, 1.0);
    if f <= 0.0 {
        return;
    }
    let fill = Rect::from_min_size(rect.min, vec2(rect.width() * f, rect.height()));
    match color {
        Some(c) => {
            painter.rect_filled(fill, rect.height() / 2.0, c);
        }
        None => gradient_bar(&painter.with_clip_rect(fill), fill, 1.0),
    }
}

/// Text with extra letter spacing (taglines: "AI VIDEO EDITOR BY JABROD").
pub fn spaced(painter: &egui::Painter, pos: Pos2, anchor: egui::Align2, text: &str, font: egui::FontId, color: Color32, spacing: f32) -> Rect {
    let mut job = egui::text::LayoutJob::default();
    job.append(text, 0.0, egui::TextFormat { font_id: font, color, extra_letter_spacing: spacing, ..Default::default() });
    let galley = painter.layout_job(job);
    let rect = anchor.anchor_size(pos, galley.size());
    painter.galley(rect.min, galley, color);
    rect
}

/// A big white primary button drawn at `rect` (sign-in). Returns its response.
pub fn primary_button_at(ui: &mut egui::Ui, rect: Rect, id: &str, label: &str) -> Response {
    let resp = ui.interact(rect, egui::Id::new(("jbd-primary", id)), Sense::click());
    let bg = if resp.is_pointer_button_down_on() {
        Color32::from_rgb(0xd4, 0xd4, 0xd8)
    } else if resp.hovered() {
        PRIMARY_BG_HOVER
    } else {
        PRIMARY_BG
    };
    ui.painter().rect_filled(rect, 8.0, bg);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, label, Tokens::semibold(13.0), PRIMARY_TEXT);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// A quiet text button drawn at `rect` (Cancel, Copy link).
pub fn ghost_button_at(ui: &mut egui::Ui, rect: Rect, id: &str, label: &str, t: &Tokens) -> Response {
    let resp = ui.interact(rect, egui::Id::new(("jbd-ghost", id)), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 8.0, t.hover);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, label, Tokens::medium(12.0), if resp.hovered() { t.text } else { t.text_dim });
    resp
}

/// A compact white button in the normal layout flow (Upgrade to Pro in notices and menus).
pub fn small_primary_button(ui: &mut egui::Ui, label: &str, _t: &Tokens) -> Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), Tokens::semibold(12.0), PRIMARY_TEXT);
    let size = vec2(galley.size().x + 22.0, 26.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let bg = if resp.hovered() { PRIMARY_BG_HOVER } else { PRIMARY_BG };
    ui.painter().rect_filled(rect, 6.0, bg);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, PRIMARY_TEXT);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// A full-width row button in a menu, with an optional colour for the label.
pub fn menu_row(ui: &mut egui::Ui, label: &str, t: &Tokens, color: Option<Color32>) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, t.hover);
    }
    ui.painter().text(pos2(rect.min.x + 10.0, rect.center().y), egui::Align2::LEFT_CENTER, label, Tokens::ui(12.5), color.unwrap_or(t.text));
    resp
}

/// The avatar: a circle with the person's initials.
pub fn avatar(painter: &egui::Painter, center: Pos2, radius: f32, initials: &str, t: &Tokens) {
    painter.circle(center, radius, t.hover, Stroke::new(1.0, t.field_border));
    painter.text(center, egui::Align2::CENTER_CENTER, initials, Tokens::semibold(radius * 0.8), t.text);
}

/// A small pill label ("PRO", "FREE", "TRIAL").
pub fn pill(painter: &egui::Painter, left_center: Pos2, label: &str, strong: bool, t: &Tokens) -> Rect {
    let galley = painter.layout_no_wrap(label.to_string(), Tokens::semibold(9.5), if strong { PRIMARY_TEXT } else { t.text });
    let rect = Rect::from_min_size(pos2(left_center.x, left_center.y - 8.0), vec2(galley.size().x + 12.0, 16.0));
    if strong {
        painter.rect_filled(rect, 8.0, PRIMARY_BG);
    } else {
        painter.rect_stroke(rect, 8.0, Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
    }
    painter.galley(rect.center() - galley.size() / 2.0, galley, if strong { PRIMARY_TEXT } else { t.text });
    rect
}

/// "2,000" (thousands separators).
pub fn thousands(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// easeOutExpo, close to the first JBD's cubic-bezier(0.16, 1, 0.3, 1).
pub fn ease_out(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x >= 1.0 { 1.0 } else { 1.0 - 2f32.powf(-10.0 * x) }
}

/// A paragraph wrapped at `width` with every line centred on `top_center`. Returns its height.
pub fn centered_paragraph(painter: &egui::Painter, top_center: Pos2, text: &str, font: egui::FontId, color: Color32, width: f32) -> f32 {
    let mut job = egui::text::LayoutJob::simple(text.to_string(), font, color, width);
    job.halign = egui::Align::Center;
    let galley = painter.layout_job(job);
    let h = galley.size().y;
    // with centred alignment the galley is laid out around x = 0
    painter.galley(top_center, galley, color);
    h
}

#[cfg(test)]
mod tests {
    #[test]
    fn thousands() {
        assert_eq!(super::thousands(0), "0");
        assert_eq!(super::thousands(999), "999");
        assert_eq!(super::thousands(2000), "2,000");
        assert_eq!(super::thousands(1234567), "1,234,567");
    }

    #[test]
    fn ease_runs_from_0_to_1() {
        assert_eq!(super::ease_out(0.0), 0.0);
        assert_eq!(super::ease_out(1.0), 1.0);
        assert!(super::ease_out(0.5) > 0.9);
    }
}
