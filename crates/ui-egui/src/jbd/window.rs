//! Resizing JBD's frameless window by its edges and corners (the window has no system frame, so
//! the app does what the frame would: show the resize cursor and hand the drag to the OS).

use egui::viewport::ResizeDirection;
use egui::{CursorIcon, Rect, Sense, pos2};

/// How far from the edge a resize starts (points).
const EDGE: f32 = 5.0;

pub fn resize_edges(ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    if ctx.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)) {
        return;
    }
    let r = ui.max_rect();
    let (l, t, rr, b) = (r.min.x, r.min.y, r.max.x, r.max.y);
    let c = EDGE * 2.0;
    let zones = [
        (Rect::from_min_max(pos2(l, t), pos2(l + c, t + c)), ResizeDirection::NorthWest, CursorIcon::ResizeNorthWest),
        (Rect::from_min_max(pos2(rr - c, t), pos2(rr, t + c)), ResizeDirection::NorthEast, CursorIcon::ResizeNorthEast),
        (Rect::from_min_max(pos2(l, b - c), pos2(l + c, b)), ResizeDirection::SouthWest, CursorIcon::ResizeSouthWest),
        (Rect::from_min_max(pos2(rr - c, b - c), pos2(rr, b)), ResizeDirection::SouthEast, CursorIcon::ResizeSouthEast),
        (Rect::from_min_max(pos2(l + c, t), pos2(rr - c, t + EDGE)), ResizeDirection::North, CursorIcon::ResizeNorth),
        (Rect::from_min_max(pos2(l + c, b - EDGE), pos2(rr - c, b)), ResizeDirection::South, CursorIcon::ResizeSouth),
        (Rect::from_min_max(pos2(l, t + c), pos2(l + EDGE, b - c)), ResizeDirection::West, CursorIcon::ResizeWest),
        (Rect::from_min_max(pos2(rr - EDGE, t + c), pos2(rr, b - c)), ResizeDirection::East, CursorIcon::ResizeEast),
    ];
    for (i, (zone, dir, cursor)) in zones.into_iter().enumerate() {
        let resp = ui.interact(zone, egui::Id::new(("jbd-resize", i)), Sense::drag());
        if resp.hovered() || resp.dragged() {
            ctx.set_cursor_icon(cursor);
        }
        if resp.drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
        }
    }
}
