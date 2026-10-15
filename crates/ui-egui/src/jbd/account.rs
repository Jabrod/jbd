//! The account chip in the header (avatar + plan badge) and its menu: who is signed in, the plan
//! ("Pro until 7 Oct 2027", "Pro trial: 5 days left", "Free plan"), this month's AI credits, and
//! Upgrade / Refresh / Sign out.
//!
//! Automation ids: `header.account`, `account.upgrade`, `account.refresh`, `account.manage`,
//! `account.signOut`.

use egui::{Align2, Color32, Rect, Sense, pos2, vec2};
use jbd_account::{Page, Tier};

use super::{open_page, style};
use crate::FilmcraftApp;
use crate::theme::Tokens;

const MENU_W: f32 = 296.0;

/// Draw the chip with its right edge at `right`, vertically centred on `cy`. Returns the chip's
/// rect (None when the JBD layer is off).
pub fn header_chip(app: &mut FilmcraftApp, ui: &mut egui::Ui, right: f32, cy: f32) -> Option<Rect> {
    if !app.jbd.enabled() {
        return None;
    }
    let t = app.tokens;
    let status = &app.jbd.status;
    let (badge, strong) = match (status.tier, status.trial) {
        (Tier::Pro, false) => ("PRO", true),
        (Tier::Pro, true) => ("TRIAL", false),
        (Tier::Free, _) => ("FREE", false),
    };
    let badge_w = ui.painter().layout_no_wrap(badge.to_string(), Tokens::semibold(9.5), t.text).size().x + 12.0;
    let w = 26.0 + 6.0 + badge_w + 8.0;
    let rect = Rect::from_min_size(pos2(right - w, cy - 14.0), vec2(w, 28.0));
    let resp = ui.interact(rect, egui::Id::new("hdr-account"), Sense::click()).on_hover_text("Your Jabrod account");
    app.auto.add("header.account", rect, "Account");
    if resp.hovered() || app.jbd.menu_open {
        ui.painter().rect_filled(rect, 14.0, t.hover);
    }
    let initials = status.account.as_ref().map_or_else(|| "?".to_string(), |a| a.initials());
    style::avatar(ui.painter(), pos2(rect.min.x + 14.0, cy), 11.0, &initials, &t);
    style::pill(ui.painter(), pos2(rect.min.x + 30.0, cy), badge, strong, &t);
    if resp.clicked() {
        app.jbd.menu_open = !app.jbd.menu_open;
        if app.jbd.menu_open
            && let Some(b) = &app.jbd.backend
        {
            b.check();
        }
    }
    if app.jbd.menu_open {
        menu(app, ui.ctx(), rect);
    }
    Some(rect)
}

/// The menu under the chip.
fn menu(app: &mut FilmcraftApp, ctx: &egui::Context, chip: Rect) {
    let t = app.tokens;
    let status = app.jbd.status.clone();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let anchor = pos2(chip.max.x - MENU_W, chip.max.y + 6.0);
    let mut close = false;
    let area = egui::Area::new(egui::Id::new("jbd-account-menu")).order(egui::Order::Foreground).fixed_pos(anchor).show(ctx, |ui| {
        egui::Frame::new()
            .fill(t.panel_bg)
            .stroke(egui::Stroke::new(1.0, t.field_border))
            .corner_radius(10.0)
            .inner_margin(egui::Margin::same(12))
            .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(160) })
            .show(ui, |ui| {
                ui.set_width(MENU_W - 24.0);
                // who
                let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
                let initials = status.account.as_ref().map_or_else(|| "?".to_string(), |a| a.initials());
                style::avatar(ui.painter(), pos2(head.min.x + 20.0, head.center().y), 18.0, &initials, &t);
                if let Some(a) = &status.account {
                    ui.painter().text(pos2(head.min.x + 48.0, head.center().y - 8.0), Align2::LEFT_CENTER, a.display_name(), Tokens::semibold(13.0), t.text);
                    ui.painter().text(pos2(head.min.x + 48.0, head.center().y + 9.0), Align2::LEFT_CENTER, &a.email, Tokens::ui(11.5), t.text_dim);
                }
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                // plan
                row(ui, "Plan", &status.plan_line(now), &t);
                if status.offline {
                    ui.label(egui::RichText::new("Offline: using your saved sign-in.").font(Tokens::ui(11.0)).color(t.text_faint));
                }
                ui.add_space(6.0);
                // AI credits
                ui.label(egui::RichText::new("AI credits this month").font(Tokens::ui(12.0)).color(t.text_dim));
                ui.add_space(2.0);
                match status.usage {
                    Some(u) => {
                        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
                        let f = u.fraction();
                        let color = if f >= 1.0 {
                            Some(t.danger)
                        } else if f >= 0.8 {
                            Some(Color32::from_rgb(0xf5, 0xb4, 0x00))
                        } else {
                            None
                        };
                        style::meter(ui.painter(), bar, f, &t, color);
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(format!(
                                "{} of {} used · {} left",
                                style::thousands(u.calls),
                                style::thousands(u.limit),
                                style::thousands(u.left())
                            ))
                            .font(Tokens::ui(11.5))
                            .color(t.text_dim),
                        );
                    }
                    None => {
                        ui.label(
                            egui::RichText::new(if status.checking { "Checking with jabrod.com…" } else { "Shown once JBD reaches jabrod.com." })
                                .font(Tokens::ui(11.5))
                                .color(t.text_faint),
                        );
                    }
                }
                ui.add_space(10.0);
                // actions
                if !status.is_pro() || status.trial {
                    let r = style::small_primary_button(ui, "Upgrade to Pro", &t);
                    app.auto.add("account.upgrade", r.rect, "Upgrade to Pro");
                    if r.clicked() {
                        open_page(ctx, Page::Upgrade);
                        close = true;
                    }
                    ui.label(
                        egui::RichText::new("1080p and 4K exports without the mark, every preset, and monthly AI credits for the Director.")
                            .font(Tokens::ui(11.0))
                            .color(t.text_faint),
                    );
                    ui.add_space(6.0);
                }
                ui.separator();
                let refresh = style::menu_row(ui, if status.checking { "Checking your plan…" } else { "Refresh plan" }, &t, None);
                app.auto.add("account.refresh", refresh.rect, "Refresh plan");
                if refresh.clicked()
                    && let Some(b) = &app.jbd.backend
                {
                    b.check();
                }
                let settings = style::menu_row(ui, "Account settings", &t, None);
                app.auto.add("account.settings", settings.rect, "Account settings");
                if settings.clicked() {
                    super::settings::open(app, "account");
                    close = true;
                }
                let manage = style::menu_row(ui, "JBD on jabrod.com", &t, None);
                app.auto.add("account.manage", manage.rect, "JBD on jabrod.com");
                if manage.clicked() {
                    open_page(ctx, Page::Product);
                    close = true;
                }
                let out = style::menu_row(ui, "Sign out", &t, Some(t.danger));
                app.auto.add("account.signOut", out.rect, "Sign out");
                if out.clicked()
                    && let Some(b) = &app.jbd.backend
                {
                    b.sign_out();
                    close = true;
                }
            });
    });
    // a click anywhere else closes the menu
    let clicked_outside =
        ctx.input(|i| i.pointer.any_click()) && ctx.input(|i| i.pointer.interact_pos()).is_some_and(|p| !area.response.rect.contains(p) && !chip.contains(p));
    if close || clicked_outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.jbd.menu_open = false;
    }
}

fn row(ui: &mut egui::Ui, label: &str, value: &str, t: &Tokens) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).font(Tokens::ui(12.0)).color(t.text_dim));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(value).font(Tokens::medium(12.0)).color(t.text));
        });
    });
}
