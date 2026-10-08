//! The Director tab (right side, next to Inspector and Transcript).
//!
//! The AI Director (an agent that edits the timeline through the same commands you use) is the
//! next milestone (J2 in docs/jbd/PLAN.md). Until it ships, this panel says plainly what it will
//! do, and shows the AI credits the account already has, so nothing here pretends to work.

use egui::{Rect, Sense, pos2, vec2};

use super::style;
use crate::FilmcraftApp;
use crate::theme::Tokens;

/// What the Director will do (shown as a list).
const COMING: [&str; 4] =
    ["Make a 45-second reel from your footage", "Remove pauses and filler words", "Add Hinglish or English captions", "Cut to the beat of your music"];

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().clone();
    p.rect_filled(rect, 0.0, t.panel_bg);
    let inner = rect.shrink2(vec2(18.0, 16.0));
    let mut y = inner.min.y + 4.0;
    p.text(pos2(inner.min.x, y), egui::Align2::LEFT_TOP, "Director", Tokens::semibold(15.0), t.text);
    let pill = style::pill(&p, pos2(inner.min.x + 74.0, y + 9.0), "NEXT UPDATE", false, &t);
    app.auto.add("director.badge", pill, "Coming in the next update");
    y += 30.0;
    let intro = "Tell JBD what you want and the Director edits your timeline for you, one undoable step at a time.";
    let g = p.layout(intro.to_string(), Tokens::ui(12.0), t.text_dim, inner.width());
    let h = g.size().y;
    p.galley(pos2(inner.min.x, y), g, t.text_dim);
    y += h + 14.0;
    for line in COMING {
        p.circle_filled(pos2(inner.min.x + 4.0, y + 8.0), 2.5, style::BLUE);
        p.text(pos2(inner.min.x + 16.0, y), egui::Align2::LEFT_TOP, line, Tokens::ui(12.5), t.text);
        y += 24.0;
    }
    y += 10.0;
    // the prompt box, visibly not available yet
    let field = Rect::from_min_size(pos2(inner.min.x, y), vec2(inner.width(), 64.0));
    p.rect_filled(field, 8.0, t.field_bg);
    p.rect_stroke(field, 8.0, egui::Stroke::new(1.0, t.separator), egui::StrokeKind::Inside);
    p.text(field.min + vec2(12.0, 12.0), egui::Align2::LEFT_TOP, "“Make a 30-second reel, remove the pauses…”", Tokens::ui(12.0), t.text_faint);
    let resp = ui.interact(field, egui::Id::new("director-prompt"), Sense::hover());
    resp.on_hover_text("The Director arrives in the next update.");
    y = field.max.y + 14.0;
    // the account's AI credits (real, from jabrod.com)
    if let Some(u) = app.jbd.status.usage {
        p.text(pos2(inner.min.x, y), egui::Align2::LEFT_TOP, "Your AI credits this month", Tokens::ui(12.0), t.text_dim);
        y += 20.0;
        style::meter(&p, Rect::from_min_size(pos2(inner.min.x, y), vec2(inner.width(), 6.0)), u.fraction(), &t, None);
        y += 12.0;
        p.text(
            pos2(inner.min.x, y),
            egui::Align2::LEFT_TOP,
            format!("{} of {} left", style::thousands(u.left()), style::thousands(u.limit)),
            Tokens::ui(11.5),
            t.text_faint,
        );
    }
}
