//! Settings ▸ Usage: what the AI Director used, the month's allowance and pay-as-you-go.
//!
//! Laid out like an AI usage dashboard: a 7 / 30 / 90-day range, one card per kind of use
//! (Thinking with its tokens, Video, Voiceover, Research, each with its credits), and a stacked bar
//! per day (credits or tokens, as a chart or a table). Below: this month's credits against the Pro
//! allowance, the models, and pay-as-you-go. The numbers come from jabrod.com
//! (`/api/desktop/usage?range=…`, fetched on a worker thread when the page opens, on Refresh and
//! when the range changes). Pay-as-you-go (Dodo Payments'
//! usage-based billing) is set up in the browser (jabrod.com/jbd/payg, where the person authorises
//! UPI AutoPay or a card); here it can be paused, its monthly spending limit changed, or turned off
//! (`/api/desktop/payg`).
//!
//! Automation ids: `usage.refresh`, `usage.range.<7|30|90>`, `usage.view.<credits|tokens>`,
//! `usage.chart`, `usage.table`, `usage.upgrade`, `usage.payg.on`, `usage.payg.pause`,
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
    /// The days shown (7, 30 or 90; 0 = not chosen yet, 30).
    range: u32,
    /// The chart counts tokens instead of credits.
    show_tokens: bool,
    /// The day-by-day numbers as a table instead of a chart.
    as_table: bool,
}

impl UsageState {
    fn days(&self) -> u32 {
        if self.range == 0 { 30 } else { self.range }
    }
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
        let (report, loading, ctx, days) = (u.report.clone(), u.loading.clone(), ctx.clone(), u.days());
        let _ = std::thread::Builder::new().name("jbd-usage".into()).spawn(move || {
            let got =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| backend.access_token().and_then(|t| jbd_account::native::usage_report(&t, days))))
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
        let (changing, message, report, ctx, days) = (u.changing.clone(), u.message.clone(), u.report.clone(), ctx.clone(), u.days());
        let _ = std::thread::Builder::new().name("jbd-payg".into()).spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let token = backend.access_token()?;
                jbd_account::native::payg_update(&token, &body)?;
                jbd_account::native::usage_report(&token, days)
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

    let mut new_range: Option<u32> = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new("AI used on your Jabrod account, by the Director and its tools.").color(t.text_dim));
        if loading {
            super::cube::loader(ui, 13.0);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.small_button("Refresh");
            app.auto.add("usage.refresh", r.rect, "Refresh");
            if r.clicked() {
                app.jbd.ai_usage.fetched_at = 0.0;
            }
            ui.add_space(6.0);
            let current = app.jbd.ai_usage.days();
            for d in [90u32, 30, 7] {
                let b = ui.add(egui::Button::selectable(current == d, format!("{d}d")));
                app.auto.add(&format!("usage.range.{d}"), b.rect, &format!("Last {d} days"));
                if b.clicked() && current != d {
                    new_range = Some(d);
                }
            }
        });
    });
    if let Some(d) = new_range {
        app.jbd.ai_usage.range = d;
        app.jbd.ai_usage.fetched_at = 0.0;
    }
    ui.add_space(10.0);

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

    overview(app, ui, &r, t);
    ui.add_space(18.0);

    // this month: the allowance
    ui.label(RichText::new("This month").font(Tokens::semibold(14.0)).color(t.text));
    let (used, limit) = (r.credits.used, r.credits.limit.max(1));
    ui.label(
        RichText::new(format!("{} of {} credits used", style::thousands(used as u32), style::thousands(limit as u32))).font(Tokens::medium(14.0)).color(t.text),
    );
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width().min(460.0), 7.0), Sense::hover());
    style::meter(ui.painter(), bar, (used as f32 / limit as f32).min(1.0), t, None);
    ui.label(RichText::new("1 credit is about $0.01 of AI. Your included credits refill on the 1st of every month.").size(11.5).color(t.text_faint));
    ui.add_space(14.0);

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

/// What each kind of use is drawn in (the logo's colours, as in the rest of JBD's highlights).
fn kind_colour(kind: &str) -> egui::Color32 {
    match kind {
        "Thinking" => style::BLUE,
        "Video" => egui::Color32::from_rgb(0x7c, 0x4d, 0xff),
        "Voiceover" => egui::Color32::from_rgb(0x00, 0xb8, 0xd9),
        _ => egui::Color32::from_rgb(0xe0, 0xa1, 0x1b),
    }
}

const KINDS: [&str; 4] = ["Thinking", "Video", "Voiceover", "Research"];

/// 4,500,000 → "4.5M", 71,500 → "71.5K", 940 → "940".
pub fn compact(n: u64) -> String {
    let f = n as f64;
    if n >= 1_000_000 {
        format!("{:.1}M", f / 1e6).replace(".0M", "M")
    } else if n >= 10_000 {
        format!("{:.1}K", f / 1e3).replace(".0K", "K")
    } else {
        style::thousands(n as u32)
    }
}

/// Days since 1970-01-01 for a YYYY-MM-DD date (civil calendar), and back.
fn day_number(ymd: &str) -> Option<i64> {
    let mut it = ymd.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, d) = (it.next()??, it.next()??, it.next()??);
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

fn ymd(day: i64) -> (i64, u32, u32) {
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// "Oct 13".
fn short_date(day: i64) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (_, m, d) = ymd(day);
    format!("{} {d}", MONTHS.get(m as usize - 1).copied().unwrap_or(""))
}

/// Today (UTC), as a day number.
fn today() -> i64 {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    (secs / 86_400) as i64
}

/// The cards (one per kind of use) and the chart or table of every day in the range.
fn overview(app: &mut FilmcraftApp, ui: &mut egui::Ui, r: &UsageReport, t: &Tokens) {
    // the cards
    // four across: each card's content width, after the gaps, its padding (12 each side) and border
    let gap = 10.0;
    let inner = (((ui.available_width() - 6.0 - gap * 3.0) / 4.0) - 26.0).max(90.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for kind in KINDS {
            let k = r.range_kinds.iter().find(|k| k.kind == kind).cloned().unwrap_or_default();
            egui::Frame::new().fill(t.field_bg).stroke(egui::Stroke::new(1.0, t.separator)).corner_radius(10.0).inner_margin(12.0).show(ui, |ui| {
                ui.set_width(inner);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                        ui.painter().circle_filled(dot.center(), 4.0, kind_colour(kind));
                        ui.label(RichText::new(kind).font(Tokens::medium(13.0)).color(t.text));
                    });
                    ui.add_space(4.0);
                    let (big, unit) = match kind {
                        "Thinking" => (compact(k.tokens), "tokens"),
                        "Video" => (compact(k.calls), if k.calls == 1 { "clip" } else { "clips" }),
                        "Voiceover" => (compact(k.calls), if k.calls == 1 { "voiceover" } else { "voiceovers" }),
                        _ => (compact(k.calls), if k.calls == 1 { "search" } else { "searches" }),
                    };
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(big).font(Tokens::semibold(18.0)).color(t.text));
                        ui.label(RichText::new(unit).size(11.5).color(t.text_dim));
                    });
                    ui.label(RichText::new(format!("{} credits", compact(k.credits))).size(12.0).color(t.text_dim));
                    if kind == "Thinking" {
                        ui.label(RichText::new(format!("{} model calls", compact(k.calls))).size(11.5).color(t.text_faint));
                    }
                });
            });
        }
    });
    ui.label(RichText::new("Credits are what the AI cost: 1 credit is about $0.01.").size(11.0).color(t.text_faint));
    ui.add_space(12.0);

    // the view switches
    let u = &mut app.jbd.ai_usage;
    ui.horizontal(|ui| {
        for (tokens, label, id) in [(false, "Credits", "credits"), (true, "Tokens", "tokens")] {
            let b = ui.add(egui::Button::selectable(u.show_tokens == tokens, label));
            app.auto.add(&format!("usage.view.{id}"), b.rect, label);
            if b.clicked() {
                u.show_tokens = tokens;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for (table, label, id) in [(true, "Table", "table"), (false, "Chart", "chart")] {
                let b = ui.add(egui::Button::selectable(u.as_table == table, label));
                app.auto.add(&format!("usage.{id}"), b.rect, label);
                if b.clicked() {
                    u.as_table = table;
                }
            }
        });
    });
    ui.add_space(6.0);

    // every day of the range, oldest first: (day number, value per kind)
    let days = r.range.max(1) as i64;
    let last = r.daily.iter().filter_map(|d| day_number(&d.day)).max().unwrap_or_else(today).max(today());
    let first = last - days + 1;
    let tokens = u.show_tokens;
    let value = |d: &jbd_account::DailyUse| if tokens { d.tokens } else { d.credits };
    let mut grid: Vec<[u64; 4]> = vec![[0; 4]; days as usize];
    for d in &r.daily {
        let (Some(n), Some(k)) = (day_number(&d.day), KINDS.iter().position(|k| *k == d.kind)) else { continue };
        if let Some(row) = grid.get_mut((n - first).max(0) as usize).filter(|_| n >= first)
            && let Some(cell) = row.get_mut(k)
        {
            *cell += value(d);
        }
    }

    egui::Frame::new().fill(t.field_bg).stroke(egui::Stroke::new(1.0, t.separator)).corner_radius(10.0).inner_margin(14.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(if tokens { "Tokens per day" } else { "Credits per day" }).font(Tokens::semibold(14.0)).color(t.text));
        ui.label(
            RichText::new(if tokens {
                format!("The Director's thinking, input and output tokens, last {days} days")
            } else {
                format!("By kind of use, last {days} days")
            })
            .size(11.5)
            .color(t.text_dim),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for (i, kind) in KINDS.iter().enumerate() {
                if tokens && i > 0 {
                    break;
                }
                let (dot, _) = ui.allocate_exact_size(vec2(9.0, 9.0), Sense::hover());
                ui.painter().rect_filled(dot, 2.0, kind_colour(kind));
                ui.label(RichText::new(*kind).size(11.5).color(t.text_dim));
                ui.add_space(8.0);
            }
        });
        ui.add_space(6.0);

        if u.as_table {
            egui::Grid::new("usage-table").striped(true).spacing(vec2(18.0, 4.0)).show(ui, |ui| {
                ui.label(RichText::new("Day").size(11.5).color(t.text_faint));
                for kind in KINDS {
                    if tokens && kind != "Thinking" {
                        continue;
                    }
                    ui.label(RichText::new(kind).size(11.5).color(t.text_faint));
                }
                ui.label(RichText::new("Total").size(11.5).color(t.text_faint));
                ui.end_row();
                for (i, row) in grid.iter().enumerate().rev() {
                    let total: u64 = row.iter().sum();
                    if total == 0 {
                        continue;
                    }
                    ui.label(RichText::new(short_date(first + i as i64)).size(12.0).color(t.text_dim));
                    for (k, v) in row.iter().enumerate() {
                        if tokens && k > 0 {
                            continue;
                        }
                        ui.label(RichText::new(compact(*v)).size(12.0).color(t.text));
                    }
                    ui.label(RichText::new(compact(total)).font(Tokens::medium(12.0)).color(t.text));
                    ui.end_row();
                }
            });
            if grid.iter().all(|r| r.iter().sum::<u64>() == 0) {
                ui.label(RichText::new("Nothing used in these days yet.").color(t.text_faint));
            }
            return;
        }

        // the chart: y-axis gridlines with values, a stacked bar per day, dates under it
        let h = 190.0;
        let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), h + 22.0), Sense::hover());
        let plot = egui::Rect::from_min_max(egui::pos2(area.min.x + 46.0, area.min.y + 6.0), egui::pos2(area.max.x - 4.0, area.min.y + h));
        let p = ui.painter();
        let top = grid.iter().map(|r| r.iter().sum::<u64>()).max().unwrap_or(0).max(1) as f64;
        // a round step: 1, 2 or 5 × 10^n, about four lines
        let raw = top / 4.0;
        let mag = 10f64.powf(raw.log10().floor());
        let step = [1.0, 2.0, 5.0, 10.0].iter().map(|m| m * mag).find(|s| *s >= raw).unwrap_or(raw).max(1.0);
        let ceiling = (top / step).ceil().max(1.0) * step;
        let mut v = 0.0;
        while v <= ceiling + 0.5 {
            let y = plot.max.y - (v / ceiling) as f32 * plot.height();
            p.line_segment([egui::pos2(plot.min.x, y), egui::pos2(plot.max.x, y)], egui::Stroke::new(1.0, t.separator));
            p.text(egui::pos2(plot.min.x - 8.0, y), egui::Align2::RIGHT_CENTER, compact(v as u64), Tokens::ui(10.5), t.text_faint);
            v += step;
        }
        let slot = plot.width() / days as f32;
        let bar = (slot * 0.62).clamp(2.0, 28.0);
        let hover = ui.input(|i| i.pointer.hover_pos());
        let mut tip: Option<(i64, [u64; 4])> = None;
        for (i, row) in grid.iter().enumerate() {
            let cx = plot.min.x + slot * (i as f32 + 0.5);
            let mut y = plot.max.y;
            for (k, val) in row.iter().enumerate() {
                if *val == 0 || (tokens && k > 0) {
                    continue;
                }
                let hgt = (*val as f64 / ceiling) as f32 * plot.height();
                let r = egui::Rect::from_min_max(egui::pos2(cx - bar / 2.0, y - hgt), egui::pos2(cx + bar / 2.0, y));
                p.rect_filled(r, 2.0, kind_colour(KINDS[k]));
                y -= hgt;
            }
            if hover.is_some_and(|h| (h.x - cx).abs() <= slot / 2.0 && plot.y_range().contains(h.y)) {
                tip = Some((first + i as i64, *row));
            }
        }
        // dates: about seven along the bottom
        let every = ((days as f32 / 7.0).ceil() as usize).max(1);
        for i in (0..days as usize).rev().step_by(every) {
            let cx = plot.min.x + slot * (i as f32 + 0.5);
            p.text(egui::pos2(cx, plot.max.y + 12.0), egui::Align2::CENTER_CENTER, short_date(first + i as i64), Tokens::ui(10.5), t.text_faint);
        }
        if let Some((day, row)) = tip {
            let mut lines = vec![short_date(day)];
            for (k, kind) in KINDS.iter().enumerate() {
                if row[k] > 0 && !(tokens && k > 0) {
                    lines.push(format!("{kind}: {} {}", compact(row[k]), if tokens { "tokens" } else { "credits" }));
                }
            }
            if lines.len() == 1 {
                lines.push("Nothing used".into());
            }
            egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), egui::Id::new("usage-bar-tip"), egui::PopupAnchor::Pointer).show(|ui| {
                for l in lines {
                    ui.label(l);
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn dates_go_there_and_back() {
        let n = super::day_number("2026-10-16").unwrap_or(0);
        assert_eq!(super::ymd(n), (2026, 10, 16));
        assert_eq!(super::short_date(n), "Oct 16");
        assert_eq!(super::ymd(super::day_number("2024-02-29").unwrap_or(0)), (2024, 2, 29));
        assert_eq!(super::compact(4_500_000), "4.5M");
        assert_eq!(super::compact(71_500), "71.5K");
        assert_eq!(super::compact(940), "940");
    }

    #[test]
    fn a_usage_state_starts_empty() {
        let s = super::UsageState::default();
        assert!(super::lock(&s.report).is_none());
    }
}
