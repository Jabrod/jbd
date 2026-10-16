//! Settings ▸ Usage: the AI credits used this month and pay-as-you-go.
//!
//! The numbers come from jabrod.com (`/api/desktop/usage`, fetched on a worker thread when the page
//! opens and on Refresh): credits used against the Pro allowance, what they went on (thinking,
//! video, voiceover, research), the models, and a bar per day. Pay-as-you-go (Dodo Payments'
//! usage-based billing) is set up in the browser (jabrod.com/jbd/payg, where the person authorises
//! UPI AutoPay or a card); here it can be paused, its monthly spending limit changed, or turned off
//! (`/api/desktop/payg`).
//!
//! Automation ids: `usage.refresh`, `usage.upgrade`, `usage.payg.on`, `usage.payg.pause`,
//! `usage.payg.cap.<minor>`, `usage.payg.off`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use egui::{RichText, Sense, vec2};
use jbd_account::{UsageReport, format_money};

use super::style;
use crate::FilmcraftApp;
use crate::theme::Tokens;

/// The page's data and what it is doing.
#[derive(Default)]
pub struct UsageState {
    report: Arc<Mutex<Option<Result<UsageReport, String>>>>,
    loading: Arc<AtomicBool>,
    fetched_at: f64,
    /// A change being sent (limit, pause, off), and its answer.
    changing: Arc<AtomicBool>,
    message: Arc<Mutex<Option<String>>>,
    /// "Turn off" was clicked once: the next click turns it off.
    confirm_off: bool,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl UsageState {
    /// Show this report without asking jabrod.com (tests, screenshots).
    pub fn preload(&mut self, r: UsageReport) {
        *lock(&self.report) = Some(Ok(r));
        self.fetched_at = f64::MAX;
    }
}

/// Fetch the report in the background.
fn refresh(app: &mut FilmcraftApp, ctx: &egui::Context) {
    app.jbd.ai_usage.fetched_at = ctx.input(|i| i.time).max(0.001);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(backend) = app.jbd.backend.clone() else { return };
        let u = &app.jbd.ai_usage;
        if u.loading.swap(true, Ordering::Relaxed) {
            return;
        }
        let (report, loading, ctx) = (u.report.clone(), u.loading.clone(), ctx.clone());
        let _ = std::thread::Builder::new().name("jbd-usage".into()).spawn(move || {
            let got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| backend.access_token().and_then(|t| jbd_account::native::usage_report(&t))))
                .unwrap_or_else(|_| Err("reading your usage failed".into()));
            *lock(&report) = Some(got);
            loading.store(false, Ordering::Relaxed);
            ctx.request_repaint();
        });
    }
    #[cfg(target_arch = "wasm32")]
    let _ = ctx;
}

/// Send a pay-as-you-go change, then read the report again.
fn change(app: &mut FilmcraftApp, ctx: &egui::Context, body: serde_json::Value) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(backend) = app.jbd.backend.clone() else { return };
        let u = &app.jbd.ai_usage;
        if u.changing.swap(true, Ordering::Relaxed) {
            return;
        }
        let (changing, message, report, ctx) = (u.changing.clone(), u.message.clone(), u.report.clone(), ctx.clone());
        let _ = std::thread::Builder::new().name("jbd-payg".into()).spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let token = backend.access_token()?;
                jbd_account::native::payg_update(&token, &body)?;
                jbd_account::native::usage_report(&token)
            }))
            .unwrap_or_else(|_| Err("the change failed".into()));
            match r {
                Ok(fresh) => {
                    *lock(&report) = Some(Ok(fresh));
                    *lock(&message) = None;
                }
                Err(e) => *lock(&message) = Some(e),
            }
            changing.store(false, Ordering::Relaxed);
            ctx.request_repaint();
        });
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (app, ctx, body);
}

/// The section's body.
pub fn page(app: &mut FilmcraftApp, ui: &mut egui::Ui, ctx: &egui::Context, t: &Tokens) {
    if !app.jbd.status.is_signed_in() {
        ui.label(RichText::new("Sign in to your Jabrod account to see your AI usage.").color(t.text_dim));
        return;
    }
    let now = ui.input(|i| i.time);
    if app.jbd.ai_usage.fetched_at == 0.0 || now - app.jbd.ai_usage.fetched_at > 60.0 {
        refresh(app, ctx);
    }
    let loading = app.jbd.ai_usage.loading.load(Ordering::Relaxed);
    let report = lock(&app.jbd.ai_usage.report).clone();
    let mut action: Option<serde_json::Value> = None;
    let mut open_url: Option<String> = None;

    ui.horizontal(|ui| {
        ui.label(RichText::new("This month").font(Tokens::semibold(14.0)).color(t.text));
        if loading {
            super::cube::loader(ui, 13.0);
        }
        let r = ui.small_button("Refresh");
        app.auto.add("usage.refresh", r.rect, "Refresh");
        if r.clicked() {
            app.jbd.ai_usage.fetched_at = 0.0;
        }
    });
    ui.add_space(6.0);

    let Some(report) = report else {
        if !loading {
            ui.label(RichText::new("Reading your usage…").color(t.text_dim));
        }
        return;
    };
    let r = match report {
        Ok(r) => r,
        Err(e) => {
            ui.label(RichText::new(format!("Couldn't read your usage: {e}. Check your connection, then Refresh.")).color(t.text_dim));
            return;
        }
    };

    // the free plan: the Director is a Pro feature
    if r.plan != "pro" && r.plan != "trial" {
        egui::Frame::new().fill(t.field_bg).corner_radius(10.0).inner_margin(14.0).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("The AI Director is part of JBD Pro").font(Tokens::semibold(14.0)).color(t.text));
            ui.label(
                RichText::new(
                    "Pro includes 1,000 AI credits every month for the Director's thinking, video and voiceover, and pay-as-you-go when you need more.",
                )
                .color(t.text_dim),
            );
            ui.add_space(8.0);
            let b = style::small_primary_button(ui, "Upgrade to Pro", t);
            app.auto.add("usage.upgrade", b.rect, "Upgrade to Pro");
            if b.clicked() {
                open_url = Some(r.urls.upgrade.clone());
            }
        });
        if let Some(u) = open_url {
            ctx.open_url(egui::OpenUrl::new_tab(u));
        }
        return;
    }

    // the allowance
    let (used, limit) = (r.credits.used, r.credits.limit.max(1));
    ui.label(
        RichText::new(format!("{} of {} credits used", style::thousands(used as u32), style::thousands(limit as u32))).font(Tokens::medium(14.0)).color(t.text),
    );
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width().min(460.0), 7.0), Sense::hover());
    style::meter(ui.painter(), bar, (used as f32 / limit as f32).min(1.0), t, None);
    ui.label(RichText::new("1 credit is about $0.01 of AI. Your included credits refill on the 1st of every month.").size(11.5).color(t.text_faint));
    ui.add_space(14.0);

    // what used them
    if !r.by_kind.is_empty() {
        ui.label(RichText::new("What used them").font(Tokens::semibold(13.0)).color(t.text));
        let top = r.by_kind.iter().map(|k| k.credits).max().unwrap_or(1).max(1);
        for k in &r.by_kind {
            ui.horizontal(|ui| {
                ui.add_sized(vec2(90.0, 18.0), egui::Label::new(RichText::new(&k.kind).color(t.text_dim)));
                let (b, _) = ui.allocate_exact_size(vec2(220.0, 8.0), Sense::hover());
                style::meter(ui.painter(), b, k.credits as f32 / top as f32, t, None);
                ui.label(
                    RichText::new(format!("{} credits · {} {}", style::thousands(k.credits as u32), k.calls, if k.calls == 1 { "use" } else { "uses" }))
                        .size(11.5)
                        .color(t.text_faint),
                );
            });
        }
        ui.add_space(10.0);
    }
    if !r.by_model.is_empty() {
        ui.label(RichText::new("Models").font(Tokens::semibold(13.0)).color(t.text));
        for m in r.by_model.iter().take(6) {
            ui.horizontal(|ui| {
                ui.add(egui::Label::new(RichText::new(&m.model).family(egui::FontFamily::Monospace).size(11.5).color(t.text_dim)).truncate());
                ui.label(RichText::new(format!("{} · {} credits", m.kind, style::thousands(m.credits as u32))).size(11.5).color(t.text_faint));
            });
        }
        ui.add_space(10.0);
    }
    // a bar per day
    if !r.days.is_empty() {
        ui.label(RichText::new("By day").font(Tokens::semibold(13.0)).color(t.text));
        let (area, _) = ui.allocate_exact_size(vec2(ui.available_width().min(460.0), 48.0), Sense::hover());
        let top = r.days.iter().map(|d| d.credits).max().unwrap_or(1).max(1) as f32;
        let w = (area.width() / 31.0).floor().max(3.0);
        for d in &r.days {
            let day: f32 = d.day.rsplit('-').next().and_then(|x| x.parse().ok()).unwrap_or(1.0);
            let h = (d.credits as f32 / top * area.height()).max(2.0);
            let x = area.min.x + (day - 1.0) * w;
            let rect = egui::Rect::from_min_max(egui::pos2(x + 1.0, area.max.y - h), egui::pos2(x + w - 1.0, area.max.y));
            ui.painter().rect_filled(rect, 1.5, style::BLUE);
        }
        ui.add_space(14.0);
    }

    // pay-as-you-go
    let changing = app.jbd.ai_usage.changing.load(Ordering::Relaxed);
    egui::Frame::new().fill(t.field_bg).corner_radius(10.0).inner_margin(14.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new("Pay-as-you-go").font(Tokens::semibold(14.0)).color(t.text));
            if changing {
                super::cube::loader(ui, 13.0);
            }
        });
        let p = r.payg.clone().filter(|p| p.status != "cancelled");
        match p {
            None => {
                ui.label(RichText::new("Keep using the Director after your 1,000 credits. Extra credits are ₹1.50 each ($0.02 outside India), charged once a month by UPI AutoPay or card, with a monthly limit you set.").color(t.text_dim));
                ui.add_space(8.0);
                let b = style::small_primary_button(ui, "Turn on pay-as-you-go", t);
                app.auto.add("usage.payg.on", b.rect, "Turn on pay-as-you-go");
                if b.clicked() {
                    open_url = Some(r.urls.payg.clone());
                }
                ui.label(RichText::new("Opens jabrod.com to set up the payment. Nothing is charged until you use credits past your allowance.").size(11.0).color(t.text_faint));
            }
            Some(p) if p.status == "pending" => {
                ui.label(RichText::new("Waiting for the payment setup to finish in your browser.").color(t.text_dim));
                if ui.button("Open the setup again").clicked() {
                    open_url = Some(r.urls.payg.clone());
                }
            }
            Some(p) => {
                let state = if p.status == "on_hold" {
                    "On hold: the last payment didn't go through"
                } else if p.paused {
                    "Paused"
                } else {
                    "On"
                };
                ui.label(RichText::new(state).font(Tokens::medium(13.0)).color(if p.status == "on_hold" { t.danger } else { t.text }));
                ui.label(
                    RichText::new(format!(
                        "{} extra credits this month: {} of your {} limit. {} per extra credit.",
                        style::thousands(p.used as u32),
                        format_money(p.spent, &p.currency),
                        format_money(p.cap, &p.currency),
                        format_money(p.price_per_credit, &p.currency)
                    ))
                    .color(t.text_dim),
                );
                let (b, _) = ui.allocate_exact_size(vec2(ui.available_width().min(460.0), 6.0), Sense::hover());
                style::meter(ui.painter(), b, if p.cap == 0 { 1.0 } else { (p.spent as f32 / p.cap as f32).min(1.0) }, t, None);
                ui.add_space(8.0);
                ui.label(RichText::new("Monthly limit").color(t.text_dim));
                ui.horizontal_wrapped(|ui| {
                    let presets: &[u64] = if p.currency.eq_ignore_ascii_case("INR") { &[50_000, 100_000, 250_000, 500_000, 1_000_000] } else { &[600, 1_200, 3_000, 6_000, 12_000] };
                    for &cap in presets {
                        let b = ui.add_enabled(!changing, egui::Button::selectable(p.cap == cap, format_money(cap, &p.currency)));
                        app.auto.add(&format!("usage.payg.cap.{cap}"), b.rect, "Monthly limit");
                        if b.clicked() && p.cap != cap {
                            action = Some(serde_json::json!({"cap": cap}));
                        }
                    }
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if p.status == "on_hold" {
                        if ui.button("Update payment method").clicked() {
                            open_url = Some(r.urls.payg.clone());
                        }
                    } else {
                        let label = if p.paused { "Resume" } else { "Pause" };
                        let b = ui.add_enabled(!changing, egui::Button::new(label));
                        app.auto.add("usage.payg.pause", b.rect, label);
                        if b.clicked() {
                            action = Some(serde_json::json!({"paused": !p.paused}));
                        }
                    }
                    let off = if app.jbd.ai_usage.confirm_off { "Click again to turn off" } else { "Turn off…" };
                    let b = ui.add_enabled(!changing, egui::Button::new(RichText::new(off).color(if app.jbd.ai_usage.confirm_off { t.danger } else { t.text_dim })).frame(false));
                    app.auto.add("usage.payg.off", b.rect, "Turn off pay-as-you-go");
                    if b.clicked() {
                        if app.jbd.ai_usage.confirm_off {
                            action = Some(serde_json::json!({"cancel": true}));
                            app.jbd.ai_usage.confirm_off = false;
                        } else {
                            app.jbd.ai_usage.confirm_off = true;
                        }
                    }
                });
                ui.label(RichText::new("Turning it off cancels the monthly payment; credits already used this month are still charged.").size(11.0).color(t.text_faint));
            }
        }
        if let Some(m) = lock(&app.jbd.ai_usage.message).clone() {
            ui.label(RichText::new(m).size(11.5).color(t.danger));
        }
    });

    if let Some(u) = open_url {
        ctx.open_url(egui::OpenUrl::new_tab(u));
        // read it again soon: the setup may have finished
        app.jbd.ai_usage.fetched_at = now - 50.0;
    }
    if let Some(body) = action {
        change(app, ctx, body);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_usage_state_starts_empty() {
        let s = super::UsageState::default();
        assert!(super::lock(&s.report).is_none());
    }
}
