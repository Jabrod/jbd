//! JBD's Jabrod account: who is signed in, their plan, and this month's AI credits.
//!
//! JBD needs a Jabrod account (free). The plan decides what exports can do:
//!
//! | Plan | Exports | AI credits a month (1 credit = $0.01 of AI cost) |
//! |---|---|---|
//! | Free | up to 720p, with the "Made with JBD" mark | 30 |
//! | Pro trial (7 days, first sign-in) | everything | 100 |
//! | Pro (₹4,999 a year, bought on jabrod.com) | everything | 250 |
//!
//! jabrod.com is the source of truth for all of it (repo `jbd_home`: `lib/platform/tokens.ts`,
//! `lib/platform/billing.ts`, `lib/platform/ai.ts`). The numbers here are only for display and
//! must match `MONTHLY_CREDITS` in `lib/platform/credits.ts`.
//!
//! - [`AccountBackend`]: what the UI talks to. The real one is [`native::JabrodAccount`] (desktop
//!   only); [`DemoAccount`] is a signed-in Pro account with no network, for screenshots and UI
//!   tests.
//! - [`AccountStatus`]: a snapshot the UI reads every frame. It never holds a token.
//!
//! How signing in works (OAuth 2.0 code + PKCE over a loopback redirect, RFC 8252) is explained
//! at the top of `native.rs`.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

use std::sync::Arc;

use serde::{Deserialize, Serialize};

#[cfg(not(target_arch = "wasm32"))]
pub mod native;

/// This app's id on the Jabrod platform (`platform.products`, the OAuth `client_id`).
pub const CLIENT_ID: &str = "jbd";

/// Where JBD's pages live. Release builds use jabrod.com; debug builds a local `npm run dev` of
/// jbd_home; `JBD_PLATFORM_URL` overrides both (no trailing slash needed).
pub fn platform_url() -> String {
    let url = std::env::var("JBD_PLATFORM_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| if cfg!(debug_assertions) { "http://localhost:3000" } else { "https://jabrod.com" }.to_string());
    url.trim_end_matches('/').to_string()
}

/// The signed-in person, as jabrod.com describes them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub name: String,
    pub email: String,
    #[serde(default)]
    pub image: Option<String>,
}

impl Account {
    /// One or two capital letters for the avatar: "Anshuman Tiwari" → "AT", "jbd" → "J".
    pub fn initials(&self) -> String {
        let source = if self.name.trim().is_empty() { &self.email } else { &self.name };
        let mut out: String = source.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect();
        if out.is_empty() {
            out.push('?');
        }
        out.to_uppercase()
    }

    /// The name to show, falling back to the email.
    pub fn display_name(&self) -> &str {
        if self.name.trim().is_empty() { &self.email } else { &self.name }
    }
}

/// The JBD plan tier.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    #[default]
    Free,
    Pro,
}

impl Tier {
    /// The tier string jabrod.com sends ("free" / "pro"); anything else is Free.
    pub fn from_server(s: &str) -> Tier {
        if s.eq_ignore_ascii_case("pro") { Tier::Pro } else { Tier::Free }
    }
}

/// Where the account flow is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// Reading the saved session at launch (a moment; the splash covers it).
    #[default]
    Starting,
    /// Nobody is signed in: the sign-in screen shows.
    SignedOut,
    /// The browser is open on jabrod.com; waiting for it to come back.
    WaitingForBrowser,
    SignedIn,
}

/// This month's AI use (Director requests).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub calls: u32,
    pub limit: u32,
}

impl Usage {
    /// Requests left this month.
    pub fn left(&self) -> u32 {
        self.limit.saturating_sub(self.calls)
    }
    /// Fraction used, 0–1, for the meter.
    pub fn fraction(&self) -> f32 {
        if self.limit == 0 { 1.0 } else { (self.calls as f32 / self.limit as f32).clamp(0.0, 1.0) }
    }
}

/// The monthly AI allowance of a plan. MUST match `MONTHLY_CREDITS` in jbd_home `lib/platform/credits.ts` (the server sends the real numbers; this is the fallback).
pub fn monthly_ai_limit(tier: Tier, trial: bool) -> u32 {
    match (tier, trial) {
        (Tier::Pro, true) => 100,
        (Tier::Pro, false) => 250,
        (Tier::Free, _) => 30,
    }
}

/// What the UI shows about the account. Never holds a token.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    pub phase: Phase,
    pub account: Option<Account>,
    pub tier: Tier,
    /// When Pro ends, ISO 8601 (from jabrod.com), if on Pro.
    pub pro_until: Option<String>,
    /// Pro is the 7-day free trial.
    pub trial: bool,
    /// Signed in from the saved session because jabrod.com couldn't be reached.
    pub offline: bool,
    /// One sentence to show (why you were signed out, why sign-in failed). Never raw HTML.
    pub message: Option<String>,
    /// This month's AI credits, once jabrod.com has been asked.
    pub usage: Option<Usage>,
    /// A check with jabrod.com is running in the background.
    pub checking: bool,
}

impl AccountStatus {
    pub fn signed_out(message: Option<String>) -> Self {
        AccountStatus { phase: Phase::SignedOut, message, ..Default::default() }
    }

    pub fn is_signed_in(&self) -> bool {
        self.phase == Phase::SignedIn
    }

    /// Pro limits apply (Pro or the trial, and signed in).
    pub fn is_pro(&self) -> bool {
        self.is_signed_in() && self.tier == Tier::Pro
    }

    /// "Pro", "Pro trial" or "Free".
    pub fn plan_label(&self) -> &'static str {
        match (self.tier, self.trial) {
            (Tier::Pro, true) => "Pro trial",
            (Tier::Pro, false) => "Pro",
            (Tier::Free, _) => "Free",
        }
    }

    /// Whole days of Pro left (rounded up), when `pro_until` is known; `now` in Unix seconds.
    pub fn pro_days_left(&self, now: u64) -> Option<u64> {
        let end = parse_iso8601(self.pro_until.as_deref()?)?;
        Some(end.saturating_sub(now).div_ceil(86_400))
    }

    /// "Pro until 12 Oct 2027", "Pro trial: 5 days left", "Free plan".
    pub fn plan_line(&self, now: u64) -> String {
        match (self.tier, self.trial, self.pro_days_left(now)) {
            (Tier::Pro, true, Some(1)) => "Pro trial: last day".to_string(),
            (Tier::Pro, true, Some(d)) => format!("Pro trial: {d} days left"),
            (Tier::Pro, false, _) => match self.pro_until.as_deref().and_then(format_date) {
                Some(date) => format!("Pro until {date}"),
                None => "Pro".to_string(),
            },
            (Tier::Pro, true, None) => "Pro trial".to_string(),
            (Tier::Free, _, _) => "Free plan".to_string(),
        }
    }
}

/// The jabrod.com pages JBD opens. Only these, so nothing can make JBD open an arbitrary link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// Buy JBD Pro (Dodo Payments checkout behind it).
    Upgrade,
    /// JBD's product page.
    Product,
    /// Help: email support.
    Support,
}

impl Page {
    pub fn url(self) -> String {
        match self {
            Page::Upgrade => format!("{}/jbd/upgrade", platform_url()),
            Page::Product => format!("{}/jbd", platform_url()),
            Page::Support => "mailto:contact@jabrod.com?subject=JBD%20support".to_string(),
        }
    }
}

/// What the UI talks to. Every method returns at once; slow work runs on a background thread and
/// shows up in [`AccountBackend::status`], with a repaint requested through the callback given
/// to [`AccountBackend::set_repaint`].
pub trait AccountBackend: Send + Sync {
    /// The current snapshot.
    fn status(&self) -> AccountStatus;
    /// Called (from any thread) whenever the status changes, so the UI redraws.
    fn set_repaint(&self, repaint: Arc<dyn Fn() + Send + Sync>);
    /// Ask jabrod.com about the saved session again (plan, credits) in the background. JBD calls
    /// it at launch and when the window regains focus, so a purchase shows without a restart.
    fn check(&self);
    /// Start signing in: returns the jabrod.com URL to open in the browser, then waits for the
    /// browser to come back in the background (up to 5 minutes, or [`AccountBackend::cancel_sign_in`]).
    fn sign_in(&self) -> Result<String, String>;
    fn cancel_sign_in(&self);
    /// Sign out on this computer (at once) and end the session on jabrod.com (best effort).
    fn sign_out(&self);
    /// A current access token for jabrod.com APIs (the AI Director). Blocking; call it from a
    /// worker thread.
    fn access_token(&self) -> Result<String, String>;
}

/// A signed-in Pro account that never touches the network: screenshots, demos, UI tests.
/// The JBD app only uses it in debug builds when `JBD_DEMO_ACCOUNT=1` is set.
pub struct DemoAccount {
    status: std::sync::Mutex<AccountStatus>,
}

impl DemoAccount {
    pub fn new(name: &str, email: &str) -> Self {
        let status = AccountStatus {
            phase: Phase::SignedIn,
            account: Some(Account { id: "demo".into(), name: name.into(), email: email.into(), image: None }),
            tier: Tier::Pro,
            pro_until: Some("2027-10-07T00:00:00.000Z".into()),
            trial: false,
            usage: Some(Usage { calls: 148, limit: monthly_ai_limit(Tier::Pro, false) }),
            ..Default::default()
        };
        DemoAccount { status: std::sync::Mutex::new(status) }
    }

    /// The same, on the free plan (tests of what Pro unlocks).
    pub fn free(name: &str, email: &str) -> Self {
        let d = DemoAccount::new(name, email);
        {
            let mut s = d.lock();
            s.tier = Tier::Free;
            s.pro_until = None;
            s.usage = None;
        }
        d
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, AccountStatus> {
        self.status.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl AccountBackend for DemoAccount {
    fn status(&self) -> AccountStatus {
        self.lock().clone()
    }
    fn set_repaint(&self, _repaint: Arc<dyn Fn() + Send + Sync>) {}
    fn check(&self) {}
    fn sign_in(&self) -> Result<String, String> {
        self.lock().phase = Phase::SignedIn;
        Err("Demo account: already signed in.".into())
    }
    fn cancel_sign_in(&self) {}
    fn sign_out(&self) {
        *self.lock() = AccountStatus::signed_out(None);
    }
    fn access_token(&self) -> Result<String, String> {
        Err("The demo account can't use jabrod.com.".into())
    }
}

/// Seconds since 1970 for an ISO 8601 UTC time like `2027-10-07T05:30:00.000Z` (the format
/// jabrod.com sends). Only the date and time matter; fractions and the zone suffix are ignored.
pub fn parse_iso8601(s: &str) -> Option<u64> {
    let s = s.trim();
    let num = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hh, mm, ss) = if s.len() >= 19 { (num(11, 13)?, num(14, 16)?, num(17, 19)?) } else { (0, 0, 0) };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let days = days_from_civil(y, m, d);
    let secs = days.checked_mul(86_400)?.checked_add(hh * 3600 + mm * 60 + ss)?;
    u64::try_from(secs).ok()
}

/// "12 Oct 2027" for an ISO 8601 date.
pub fn format_date(iso: &str) -> Option<String> {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let y: i64 = iso.get(0..4)?.parse().ok()?;
    let m: usize = iso.get(5..7)?.parse().ok()?;
    let d: u32 = iso.get(8..10)?.parse().ok()?;
    Some(format!("{d} {} {y}", MONTHS.get(m.checked_sub(1)?)?))
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_parse_and_format() {
        assert_eq!(parse_iso8601("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso8601("2026-10-07T00:00:00.000Z"), Some(1_791_331_200));
        assert_eq!(parse_iso8601("2024-02-29"), Some(1_709_164_800));
        assert_eq!(parse_iso8601("not a date"), None);
        assert_eq!(parse_iso8601("2026-13-01T00:00:00Z"), None);
        assert_eq!(format_date("2027-10-12T00:00:00.000Z").as_deref(), Some("12 Oct 2027"));
        assert_eq!(format_date("2027-00-12"), None);
    }

    #[test]
    fn plan_lines() {
        let now = parse_iso8601("2026-10-07T12:00:00Z").unwrap_or_default();
        let mut s =
            AccountStatus { phase: Phase::SignedIn, tier: Tier::Pro, trial: true, pro_until: Some("2026-10-12T12:00:00Z".into()), ..Default::default() };
        assert_eq!(s.plan_line(now), "Pro trial: 5 days left");
        assert_eq!(s.plan_label(), "Pro trial");
        assert!(s.is_pro());
        s.trial = false;
        s.pro_until = Some("2027-10-07T00:00:00Z".into());
        assert_eq!(s.plan_line(now), "Pro until 7 Oct 2027");
        s.tier = Tier::Free;
        assert_eq!(s.plan_line(now), "Free plan");
        assert!(!s.is_pro());
        // signed out is never Pro, whatever the tier field says
        let out = AccountStatus { tier: Tier::Pro, ..AccountStatus::signed_out(None) };
        assert!(!out.is_pro());
    }

    #[test]
    fn limits_match_jabrod_com() {
        assert_eq!(monthly_ai_limit(Tier::Free, false), 30);
        assert_eq!(monthly_ai_limit(Tier::Pro, true), 100);
        assert_eq!(monthly_ai_limit(Tier::Pro, false), 250, "Pro: MONTHLY_CREDITS.pro in jbd_home lib/platform/credits.ts");
        let u = Usage { calls: 45, limit: 30 };
        assert_eq!(u.left(), 0);
        assert_eq!(u.fraction(), 1.0);
    }

    #[test]
    fn initials() {
        let a = |name: &str, email: &str| Account { id: "x".into(), name: name.into(), email: email.into(), image: None };
        assert_eq!(a("Anshuman Tiwari", "a@b.c").initials(), "AT");
        assert_eq!(a("", "jbd@jabrod.com").initials(), "J");
        assert_eq!(a("  ", "").initials(), "?");
    }

    #[test]
    fn tier_from_server() {
        assert_eq!(Tier::from_server("pro"), Tier::Pro);
        assert_eq!(Tier::from_server("PRO"), Tier::Pro);
        assert_eq!(Tier::from_server("enterprise?"), Tier::Free);
    }
}

// ------------------------------------------------------------------------------------ releases

/// The latest JBD release, as jabrod.com/api/releases/jbd describes it (the in-app update check).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub version: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub notes: Vec<String>,
    /// The installer (for copies installed from jabrod.com).
    #[serde(default)]
    pub download_url: String,
    /// The product page (fallback when there's no direct download).
    #[serde(default)]
    pub page_url: String,
}

/// Is `latest` a newer version than `current`? Compares `1.10.0` > `1.9.2` as numbers; anything
/// after a `-` (a pre-release tag) is ignored, and an unreadable part counts as 0.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> { v.trim().trim_start_matches('v').split('-').next().unwrap_or("").split('.').map(|p| p.parse().unwrap_or(0)).collect() };
    let (a, b) = (parts(latest), parts(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// What a problem report carries besides the message.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// "bug", "idea", "question" or "billing" (jabrod.com's support topics).
    pub topic: String,
    pub subject: String,
    pub message: String,
    /// App version, Windows version, … (short values).
    pub context: Vec<(String, String)>,
    /// The end of JBD's log, when the person chose to include it.
    pub log: Option<String>,
}

#[cfg(test)]
mod release_tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers() {
        assert!(is_newer("1.1.0", "1.0.0"));
        assert!(is_newer("1.10.0", "1.9.9"));
        assert!(is_newer("2.0", "1.99.99"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("1.0.0", "1.1.0"));
        assert!(!is_newer("1.1.0-beta", "1.1.0"));
        assert!(is_newer("v1.0.1", "1.0.0"));
        assert!(!is_newer("garbage", "1.0.0"));
    }
}

// --------------------------------------------------------------------------------------- usage

/// This month's AI usage, as jabrod.com/api/desktop/usage reports it (Settings ▸ Usage).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UsageReport {
    pub plan: String,
    pub credits: CreditCount,
    pub by_kind: Vec<KindUse>,
    pub by_model: Vec<ModelUse>,
    pub days: Vec<DayUse>,
    pub payg: Option<PaygInfo>,
    pub urls: UsageUrls,
    /// The days the cards and the chart cover (7, 30 or 90).
    pub range: u32,
    /// Per kind over `range` (always Thinking, Video, Voiceover, Research).
    pub range_kinds: Vec<RangeKind>,
    /// Per day and kind over `range` (days without use are left out).
    pub daily: Vec<DailyUse>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RangeKind {
    pub kind: String,
    pub credits: u64,
    pub calls: u64,
    /// Input + output tokens (the Director's thinking only).
    pub tokens: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DailyUse {
    /// YYYY-MM-DD (UTC)
    pub day: String,
    pub kind: String,
    pub credits: u64,
    pub tokens: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CreditCount {
    pub used: u64,
    pub limit: u64,
}

/// Credits by what they were spent on ("Thinking", "Video", "Voiceover", "Research").
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KindUse {
    pub kind: String,
    pub credits: u64,
    pub calls: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelUse {
    pub model: String,
    pub kind: String,
    pub credits: u64,
    pub calls: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DayUse {
    /// YYYY-MM-DD
    pub day: String,
    pub credits: u64,
}

/// Pay-as-you-go: money in minor units of `currency` (paise, cents).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PaygInfo {
    /// "pending", "active", "on_hold" or "cancelled".
    pub status: String,
    pub paused: bool,
    pub currency: String,
    pub price_per_credit: u64,
    /// The monthly spending limit.
    pub cap: u64,
    /// Credits used past the allowance this month, and what they cost.
    pub used: u64,
    pub spent: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UsageUrls {
    pub upgrade: String,
    pub payg: String,
}

/// "₹1,000" / "$12.00" from minor units.
pub fn format_money(minor: u64, currency: &str) -> String {
    if currency.eq_ignore_ascii_case("INR") {
        let rupees = minor / 100;
        let paise = minor % 100;
        let whole = group_indian(rupees);
        if paise == 0 { format!("₹{whole}") } else { format!("₹{whole}.{paise:02}") }
    } else {
        format!("${}.{:02}", minor / 100, minor % 100)
    }
}

/// 100000 → "1,00,000" (Indian digit grouping).
fn group_indian(n: u64) -> String {
    let s = n.to_string();
    if s.len() <= 3 {
        return s;
    }
    let (head, last3) = s.split_at(s.len() - 3);
    let mut parts: Vec<String> = Vec::new();
    let mut h = head.to_string();
    while h.len() > 2 {
        let cut = h.len() - 2;
        parts.insert(0, h[cut..].to_string());
        h.truncate(cut);
    }
    if !h.is_empty() {
        parts.insert(0, h);
    }
    format!("{},{last3}", parts.join(","))
}

#[cfg(test)]
mod usage_tests {
    use super::*;

    #[test]
    fn money_reads_like_money() {
        assert_eq!(format_money(100_000, "INR"), "₹1,000");
        assert_eq!(format_money(150, "INR"), "₹1.50");
        assert_eq!(format_money(10_000_000, "INR"), "₹1,00,000");
        assert_eq!(format_money(1_200, "USD"), "$12.00");
        assert_eq!(format_money(2, "USD"), "$0.02");
    }

    #[test]
    fn a_usage_report_reads_from_the_server_answer() {
        let r: UsageReport = serde_json::from_str(
            r#"{"plan":"pro","credits":{"used":120,"limit":1000},"byKind":[{"kind":"Video","credits":90,"calls":3}],"byModel":[],"days":[{"day":"2026-10-16","credits":120}],"payg":{"status":"active","paused":false,"currency":"INR","pricePerCredit":150,"cap":100000,"used":0,"spent":0},"urls":{"upgrade":"u","payg":"p"}}"#,
        )
        .unwrap_or_default();
        assert_eq!(r.credits.limit, 1000);
        assert_eq!(r.by_kind[0].kind, "Video");
        assert_eq!(r.payg.map(|p| p.cap), Some(100_000));
    }
}
