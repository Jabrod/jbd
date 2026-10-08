//! Signing in to JBD with a Jabrod account (desktop). Ported from the first JBD's
//! `src-tauri/src/auth.rs`; the server side is jbd_home `lib/platform/tokens.ts`.
//!
//! JBD never sees your password. It uses the method RFC 8252 recommends for desktop apps
//! (OAuth 2.0 authorization code + PKCE over a loopback redirect):
//!
//!   1. [`JabrodAccount::sign_in`] makes a one-time secret (the PKCE "verifier") and sends only its
//!      SHA-256 (the "challenge") to jabrod.com, by opening jabrod.com/desktop/authorize in your
//!      normal browser.
//!   2. You sign in there (email + password or Google).
//!   3. jabrod.com sends the browser back to http://127.0.0.1:<port>/callback, a tiny listener
//!      opened on a random port, with a one-time code.
//!   4. JBD swaps the code AND the verifier for tokens (POST /api/desktop/token). Anyone who
//!      intercepted the code can't use it without the verifier.
//!
//! The tokens:
//!   - access token: a 15-minute pass sent with API calls. Kept in memory only.
//!   - refresh token: gets new access tokens for 30 days. Kept in the operating system's credential
//!     store (Windows Credential Manager), never in a plain file on Windows. The server replaces it
//!     on every use and treats a replayed old one as theft (it signs that device out), so every use
//!     of it goes through one lock (`Inner::token_lock`).
//!
//! Offline: if jabrod.com can't be reached, JBD keeps working for up to [`OFFLINE_GRACE_DAYS`]
//! since the last successful check.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Account, AccountBackend, AccountStatus, CLIENT_ID, Phase, Tier, Usage, monthly_ai_limit, platform_url};

/// How long JBD keeps working without reaching jabrod.com.
pub const OFFLINE_GRACE_DAYS: u64 = 14;
/// How long to wait for you to finish signing in in the browser.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
/// Focus changes call `check` often; ask jabrod.com at most this often.
const CHECK_INTERVAL: Duration = Duration::from_secs(30);
/// Where the session is kept in the Credential Manager ("Windows Credentials").
const KEYRING_SERVICE: &str = "com.jabrod.jbd";
const KEYRING_USER: &str = "jabrod-session";

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ── The saved session ─────────────────────────────────────────────────────────────────────────

/// What is saved between runs, as JSON. Field names match the first JBD, so a session saved by
/// it (same Credential Manager entry) still loads.
#[derive(Clone, Serialize, Deserialize)]
struct Stored {
    refresh_token: String,
    account: Account,
    tier: String,
    #[serde(default)]
    pro_until: Option<String>,
    #[serde(default)]
    trial: bool,
    /// When jabrod.com last confirmed this session (seconds since 1970).
    verified_at: u64,
}

/// Where the saved session lives.
pub trait SessionStore: Send + Sync {
    fn load(&self) -> Option<String>;
    fn save(&self, json: &str) -> Result<(), String>;
    fn delete(&self);
}

/// The operating system's credential store (Windows Credential Manager, macOS Keychain).
#[cfg(any(windows, target_os = "macos"))]
pub struct KeyringStore;

#[cfg(any(windows, target_os = "macos"))]
impl KeyringStore {
    fn entry() -> Result<keyring::Entry, String> {
        keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).map_err(|e| format!("Can't use the credential store: {e}"))
    }
}

#[cfg(any(windows, target_os = "macos"))]
impl SessionStore for KeyringStore {
    fn load(&self) -> Option<String> {
        Self::entry().ok()?.get_password().ok()
    }
    fn save(&self, json: &str) -> Result<(), String> {
        Self::entry()?.set_password(json).map_err(|e| format!("Couldn't save your sign-in: {e}"))
    }
    fn delete(&self) {
        if let Ok(e) = Self::entry() {
            let _ = e.delete_credential();
        }
    }
}

/// A file (systems without a supported credential store).
pub struct FileStore(pub std::path::PathBuf);

impl SessionStore for FileStore {
    fn load(&self) -> Option<String> {
        std::fs::read_to_string(&self.0).ok()
    }
    fn save(&self, json: &str) -> Result<(), String> {
        if let Some(dir) = self.0.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&self.0, json).map_err(|e| format!("Couldn't save your sign-in: {e}"))
    }
    fn delete(&self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// In memory only (tests).
#[derive(Default)]
pub struct MemoryStore(Mutex<Option<String>>);

impl SessionStore for MemoryStore {
    fn load(&self) -> Option<String> {
        lock(&self.0).clone()
    }
    fn save(&self, json: &str) -> Result<(), String> {
        *lock(&self.0) = Some(json.to_string());
        Ok(())
    }
    fn delete(&self) {
        *lock(&self.0) = None;
    }
}

/// The store for this computer: the credential store where there is one, else `account.json` in
/// `data_dir`.
pub fn os_store(data_dir: Option<std::path::PathBuf>) -> Box<dyn SessionStore> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        let _ = data_dir;
        Box::new(KeyringStore)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Box::new(FileStore(data_dir.unwrap_or_else(std::env::temp_dir).join("account.json")))
    }
}

// ── Talking to jabrod.com ─────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
    refresh_token: String,
    user: Account,
    tier: String,
    #[serde(default)]
    pro_until: Option<String>,
    #[serde(default)]
    trial: bool,
}

#[derive(Deserialize)]
struct OAuthError {
    error: String,
    error_description: Option<String>,
}

/// A token rejection is shown on the sign-in screen, so it must always be one short sentence. A
/// proxy or a stale deployment can answer with a whole HTML page; never show that.
fn token_error(status: u16, text: &str) -> String {
    if let Ok(e) = serde_json::from_str::<OAuthError>(text) {
        return e.error_description.unwrap_or(e.error);
    }
    match status {
        404 => "JBD sign-in is temporarily unavailable. Try again in a moment.".into(),
        400..=499 => "Sign-in couldn't be completed. Try again.".into(),
        _ => "Jabrod sign-in is temporarily unavailable. Try again in a moment.".into(),
    }
}

/// How a call to the token endpoint went.
enum TokenOutcome {
    Ok(TokenResponse),
    /// jabrod.com said no (expired, revoked, replayed): the session is over.
    Rejected(String),
    /// Couldn't reach jabrod.com (offline, DNS, server down).
    Unreachable(String),
}

fn agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_global(Some(HTTP_TIMEOUT))
        // read 4xx bodies ourselves: they carry the OAuth error description
        .http_status_as_error(false)
        .user_agent(concat!("JBD/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn post_json(path: &str, body: &serde_json::Value) -> Result<(u16, String), String> {
    let mut resp =
        agent().post(&format!("{}{path}", platform_url())).header("Content-Type", "application/json").send(body.to_string()).map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    Ok((status, text))
}

fn token_request(body: serde_json::Value) -> TokenOutcome {
    match post_json("/api/desktop/token", &body) {
        Err(e) => TokenOutcome::Unreachable(e),
        Ok((200, text)) => match serde_json::from_str::<TokenResponse>(&text) {
            Ok(t) => TokenOutcome::Ok(t),
            Err(e) => TokenOutcome::Unreachable(format!("unexpected answer: {e}")),
        },
        // 5xx: the server is having trouble; that is not the same as "signed out"
        Ok((status, _)) if status >= 500 => TokenOutcome::Unreachable(format!("server error {status}")),
        Ok((status, text)) => TokenOutcome::Rejected(token_error(status, &text)),
    }
}

#[derive(Deserialize)]
struct MeResponse {
    #[serde(default)]
    tier: Option<String>,
    #[serde(default)]
    usage: Option<MeUsage>,
}
#[derive(Deserialize)]
struct MeUsage {
    month: MeMonth,
}
#[derive(Deserialize)]
struct MeMonth {
    #[serde(default)]
    calls: u32,
    /// Sent by newer jabrod.com versions; the app's table is the fallback.
    #[serde(default)]
    limit: Option<u32>,
}

/// GET /api/desktop/me: the current tier and this month's AI use.
fn fetch_me(access: &str) -> Result<MeResponse, String> {
    let mut resp = agent()
        .get(&format!("{}/api/desktop/me?client_id={CLIENT_ID}", platform_url()))
        .header("Authorization", &format!("Bearer {access}"))
        .call()
        .map_err(|e| e.to_string())?;
    if resp.status().as_u16() != 200 {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

// ── PKCE and the loopback redirect ────────────────────────────────────────────────────────────

fn random_b64(bytes: usize) -> Result<String, String> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|e| format!("No secure randomness available: {e}"))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

/// The PKCE challenge for a verifier: base64url(SHA-256(verifier)) (RFC 7636, "S256").
fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Percent-encodes a query value (everything but letters, digits and -._~).
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Undoes `%XX` and `+` in a query value. A `%` not followed by two hex digits is kept.
fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b, hex) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What came back on the loopback redirect.
#[derive(Debug, PartialEq)]
struct Callback {
    code: Option<String>,
    error: Option<String>,
    state: Option<String>,
}

/// Reads "GET /callback?code=…&state=… HTTP/1.1". None for any other path (browsers also ask
/// for /favicon.ico).
fn parse_callback(request_line: &str) -> Option<Callback> {
    let target = request_line.strip_prefix("GET ")?.split(' ').next()?;
    let query = target.strip_prefix("/callback?")?;
    let mut cb = Callback { code: None, error: None, state: None };
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = Some(decode(value));
        match key {
            "code" => cb.code = value,
            "error" => cb.error = value,
            "state" => cb.state = value,
            _ => {}
        }
    }
    Some(cb)
}

/// Shown in the browser after signing in. Windows won't let JBD pull itself in front of the
/// browser, so the page says to go back to JBD.
const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>JBD</title>\
<body style=\"margin:0;height:100vh;display:grid;place-items:center;background:#0b0b0c;color:#f4f4f5;font:15px system-ui,sans-serif\">\
<div style=\"text-align:center\"><h1 style=\"font-size:20px;margin:0 0 8px\">Signed in to JBD</h1>\
<p style=\"color:#9b9ba3;margin:0\">You can close this tab and go back to JBD.</p></div>";

const CANCELLED_PAGE: &str = "<!doctype html><meta charset=utf-8><title>JBD</title>\
<body style=\"margin:0;height:100vh;display:grid;place-items:center;background:#0b0b0c;color:#f4f4f5;font:15px system-ui,sans-serif\">\
<div style=\"text-align:center\"><h1 style=\"font-size:20px;margin:0 0 8px\">Sign-in cancelled</h1>\
<p style=\"color:#9b9ba3;margin:0\">You can close this tab.</p></div>";

/// Answers one browser request on the listener; Some(callback) when it was the redirect. The
/// page says "Signed in" only for a code that answers this request (`expected_state`).
fn serve_one(mut socket: TcpStream, expected_state: &str) -> Option<Callback> {
    let _ = socket.set_nonblocking(false);
    let _ = socket.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = vec![0u8; 8192];
    let n = socket.read(&mut buf).unwrap_or(0);
    let request = String::from_utf8_lossy(buf.get(..n).unwrap_or_default());
    let first_line = request.lines().next().unwrap_or("");
    let Some(cb) = parse_callback(first_line) else {
        let _ = socket.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return None;
    };
    let page = if cb.code.is_some() && cb.state.as_deref() == Some(expected_state) { DONE_PAGE } else { CANCELLED_PAGE };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{page}",
        page.len()
    );
    let _ = socket.write_all(response.as_bytes());
    let _ = socket.flush();
    Some(cb)
}

// ── The account ───────────────────────────────────────────────────────────────────────────────

struct Inner {
    status: Mutex<AccountStatus>,
    /// The current access token and when it expires (Unix seconds).
    access: Mutex<Option<(String, u64)>>,
    /// Every use of the (single-use) refresh token goes through this lock.
    token_lock: Mutex<()>,
    /// Bumped by every sign-in and cancel: a waiting sign-in whose number is stale stops.
    sign_in_gen: AtomicU64,
    checking: AtomicBool,
    last_check: Mutex<Option<Instant>>,
    repaint: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    store: Box<dyn SessionStore>,
}

impl Inner {
    fn load(&self) -> Option<Stored> {
        self.store.load().and_then(|json| serde_json::from_str(&json).ok())
    }

    fn set(&self, f: impl FnOnce(&mut AccountStatus)) {
        f(&mut lock(&self.status));
        let repaint = lock(&self.repaint).clone();
        if let Some(r) = repaint {
            r();
        }
    }

    /// Saves a fresh token response: refresh token to the store, access token to memory.
    fn remember(&self, t: TokenResponse) -> Result<Stored, String> {
        let stored = Stored { refresh_token: t.refresh_token, account: t.user, tier: t.tier, pro_until: t.pro_until, trial: t.trial, verified_at: now_secs() };
        self.store.save(&serde_json::to_string(&stored).map_err(|e| e.to_string())?)?;
        *lock(&self.access) = Some((t.access_token, now_secs() + t.expires_in));
        Ok(stored)
    }

    fn forget(&self) {
        self.store.delete();
        *lock(&self.access) = None;
    }

    /// Status from a saved session.
    fn signed_in(stored: &Stored, offline: bool, usage: Option<Usage>) -> AccountStatus {
        AccountStatus {
            phase: Phase::SignedIn,
            account: Some(stored.account.clone()),
            tier: Tier::from_server(&stored.tier),
            pro_until: stored.pro_until.clone(),
            trial: stored.trial,
            offline,
            message: None,
            usage,
            checking: false,
        }
    }

    /// A refresh with jabrod.com, then the month's AI use. Runs on a worker thread.
    fn check_now(&self) {
        let _one_at_a_time = lock(&self.token_lock);
        let Some(stored) = self.load() else {
            self.set(|s| s.checking = false);
            return;
        };
        let outcome = token_request(serde_json::json!({
            "grant_type": "refresh_token",
            "client_id": CLIENT_ID,
            "refresh_token": stored.refresh_token,
        }));
        match outcome {
            TokenOutcome::Ok(tokens) => {
                let access = tokens.access_token.clone();
                match self.remember(tokens) {
                    Ok(stored) => {
                        let usage = lock(&self.status).usage;
                        self.set(|s| *s = Self::signed_in(&stored, false, usage));
                        self.update_usage(&access, &stored);
                    }
                    Err(e) => self.set(|s| {
                        s.checking = false;
                        s.message = Some(e);
                    }),
                }
            }
            TokenOutcome::Rejected(reason) => {
                self.forget();
                self.set(|s| *s = AccountStatus::signed_out(Some(reason)));
            }
            TokenOutcome::Unreachable(_) => {
                if now_secs().saturating_sub(stored.verified_at) < OFFLINE_GRACE_DAYS * 86_400 {
                    let usage = lock(&self.status).usage;
                    self.set(|s| *s = Self::signed_in(&stored, true, usage));
                } else {
                    // keep the saved session: it works again once you're back online
                    self.set(|s| {
                        *s = AccountStatus::signed_out(Some(format!(
                            "JBD needs to check your account once every {OFFLINE_GRACE_DAYS} days. Connect to the internet and try again."
                        )))
                    });
                }
            }
        }
        *lock(&self.last_check) = Some(Instant::now());
    }

    /// Asks /api/desktop/me for the current tier and this month's AI use.
    fn update_usage(&self, access: &str, stored: &Stored) {
        if let Ok(me) = fetch_me(access) {
            let tier = me.tier.as_deref().map(Tier::from_server).unwrap_or_else(|| Tier::from_server(&stored.tier));
            let usage = me.usage.map(|u| Usage { calls: u.month.calls, limit: u.month.limit.unwrap_or_else(|| monthly_ai_limit(tier, stored.trial)) });
            self.set(|s| {
                s.tier = tier;
                if usage.is_some() {
                    s.usage = usage;
                }
            });
        }
    }

    /// The browser part of signing in, on a worker thread.
    fn finish_sign_in(&self, listener: TcpListener, verifier: String, expected_state: String, redirect_uri: String, generation: u64) {
        let deadline = Instant::now() + SIGN_IN_TIMEOUT;
        let stale = || self.sign_in_gen.load(Ordering::SeqCst) != generation;
        let callback = loop {
            if stale() {
                return; // cancelled, or a newer sign-in took over
            }
            if Instant::now() > deadline {
                self.set(|s| *s = AccountStatus::signed_out(Some("Sign-in timed out. Try again.".into())));
                return;
            }
            match listener.accept() {
                Ok((socket, _)) => {
                    if let Some(cb) = serve_one(socket, &expected_state) {
                        break cb;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(100)),
                Err(e) => {
                    self.set(|s| *s = AccountStatus::signed_out(Some(format!("Sign-in stopped: {e}"))));
                    return;
                }
            }
        };
        if stale() {
            return;
        }
        if callback.state.as_deref() != Some(expected_state.as_str()) {
            self.set(|s| *s = AccountStatus::signed_out(Some("That sign-in answer wasn't for this request. Try again.".into())));
            return;
        }
        if callback.error.is_some() {
            self.set(|s| *s = AccountStatus::signed_out(Some("Sign-in was cancelled in the browser.".into())));
            return;
        }
        let Some(code) = callback.code else {
            self.set(|s| *s = AccountStatus::signed_out(Some("Sign-in didn't finish. Try again.".into())));
            return;
        };
        let device = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).ok();
        let _one_at_a_time = lock(&self.token_lock);
        match token_request(serde_json::json!({
            "grant_type": "authorization_code",
            "client_id": CLIENT_ID,
            "code": code,
            "code_verifier": verifier,
            "redirect_uri": redirect_uri,
            "device_name": device,
        })) {
            TokenOutcome::Ok(tokens) => {
                let access = tokens.access_token.clone();
                match self.remember(tokens) {
                    Ok(stored) => {
                        self.set(|s| *s = Self::signed_in(&stored, false, None));
                        self.update_usage(&access, &stored);
                        *lock(&self.last_check) = Some(Instant::now());
                    }
                    Err(e) => self.set(|s| *s = AccountStatus::signed_out(Some(e))),
                }
            }
            TokenOutcome::Rejected(reason) => self.set(|s| *s = AccountStatus::signed_out(Some(reason))),
            TokenOutcome::Unreachable(e) => self.set(|s| *s = AccountStatus::signed_out(Some(format!("Couldn't reach jabrod.com: {e}")))),
        }
    }
}

/// The real account backend: jabrod.com, the credential store, background threads.
#[derive(Clone)]
pub struct JabrodAccount {
    inner: Arc<Inner>,
}

impl JabrodAccount {
    /// Reads the saved session at once (so JBD can open straight away) and confirms it with
    /// jabrod.com in the background.
    pub fn start(store: Box<dyn SessionStore>) -> Self {
        let inner = Arc::new(Inner {
            status: Mutex::new(AccountStatus::default()),
            access: Mutex::new(None),
            token_lock: Mutex::new(()),
            sign_in_gen: AtomicU64::new(0),
            checking: AtomicBool::new(false),
            last_check: Mutex::new(None),
            repaint: Mutex::new(None),
            store,
        });
        let initial = match inner.load() {
            Some(stored) => Inner::signed_in(&stored, false, None),
            None => AccountStatus::signed_out(None),
        };
        *lock(&inner.status) = initial;
        let account = JabrodAccount { inner };
        account.check();
        account
    }
}

impl AccountBackend for JabrodAccount {
    fn status(&self) -> AccountStatus {
        let mut s = lock(&self.inner.status).clone();
        s.checking = self.inner.checking.load(Ordering::SeqCst);
        s
    }

    fn set_repaint(&self, repaint: Arc<dyn Fn() + Send + Sync>) {
        *lock(&self.inner.repaint) = Some(repaint);
    }

    fn check(&self) {
        if lock(&self.inner.status).phase != Phase::SignedIn {
            return;
        }
        if lock(&self.inner.last_check).is_some_and(|t| t.elapsed() < CHECK_INTERVAL) {
            return;
        }
        if self.inner.checking.swap(true, Ordering::SeqCst) {
            return; // one is already running
        }
        let inner = self.inner.clone();
        let spawned = std::thread::Builder::new().name("jbd-account-check".into()).spawn(move || {
            inner.check_now();
            inner.checking.store(false, Ordering::SeqCst);
            inner.set(|_| {});
        });
        if spawned.is_err() {
            self.inner.checking.store(false, Ordering::SeqCst);
        }
    }

    fn sign_in(&self) -> Result<String, String> {
        let verifier = random_b64(32)?;
        let state = random_b64(16)?;
        // Port 0 = any free port; 127.0.0.1 only, so nothing else on the network can reach it.
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("Couldn't start sign-in: {e}"))?;
        listener.set_nonblocking(true).map_err(|e| format!("Couldn't start sign-in: {e}"))?;
        let port = listener.local_addr().map_err(|e| format!("Couldn't start sign-in: {e}"))?.port();
        let redirect_uri = format!("http://127.0.0.1:{port}/callback");
        let url = format!(
            "{}/desktop/authorize?client_id={CLIENT_ID}&redirect_uri={}&code_challenge={}&code_challenge_method=S256&state={}",
            platform_url(),
            encode(&redirect_uri),
            challenge_for(&verifier),
            state
        );
        let generation = self.inner.sign_in_gen.fetch_add(1, Ordering::SeqCst) + 1;
        self.inner.set(|s| *s = AccountStatus { phase: Phase::WaitingForBrowser, ..Default::default() });
        let inner = self.inner.clone();
        std::thread::Builder::new()
            .name("jbd-sign-in".into())
            .spawn(move || inner.finish_sign_in(listener, verifier, state, redirect_uri, generation))
            .map_err(|e| format!("Couldn't start sign-in: {e}"))?;
        Ok(url)
    }

    fn cancel_sign_in(&self) {
        self.inner.sign_in_gen.fetch_add(1, Ordering::SeqCst);
        self.inner.set(|s| {
            if s.phase == Phase::WaitingForBrowser {
                *s = AccountStatus::signed_out(None);
            }
        });
    }

    fn sign_out(&self) {
        let stored = self.inner.load();
        self.inner.forget();
        self.inner.set(|s| *s = AccountStatus::signed_out(None));
        if let Some(stored) = stored {
            // best effort: signing out locally must work offline too
            let _ = std::thread::Builder::new().name("jbd-sign-out".into()).spawn(move || {
                let _ = post_json("/api/desktop/revoke", &serde_json::json!({ "refresh_token": stored.refresh_token }));
            });
        }
    }

    fn access_token(&self) -> Result<String, String> {
        let current = || lock(&self.inner.access).clone().filter(|(_, exp)| *exp > now_secs() + 60).map(|(t, _)| t);
        if let Some(t) = current() {
            return Ok(t);
        }
        let _one_at_a_time = lock(&self.inner.token_lock);
        // another call may have refreshed while this one waited
        if let Some(t) = current() {
            return Ok(t);
        }
        let stored = self.inner.load().ok_or("Sign in to use AI features.")?;
        match token_request(serde_json::json!({ "grant_type": "refresh_token", "client_id": CLIENT_ID, "refresh_token": stored.refresh_token })) {
            TokenOutcome::Ok(tokens) => {
                let token = tokens.access_token.clone();
                let stored = self.inner.remember(tokens)?;
                let usage = lock(&self.inner.status).usage;
                self.inner.set(|s| *s = Inner::signed_in(&stored, false, usage));
                Ok(token)
            }
            TokenOutcome::Rejected(_) => {
                self.inner.forget();
                self.inner.set(|s| *s = AccountStatus::signed_out(Some("Your session has ended. Sign in again.".into())));
                Err("Your session has ended. Sign in again.".into())
            }
            TokenOutcome::Unreachable(e) => Err(format!("AI needs an internet connection. Couldn't reach jabrod.com: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_rfc_7636() {
        // the worked example in RFC 7636, appendix B
        assert_eq!(challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn token_errors_never_show_an_html_page() {
        assert_eq!(token_error(404, "<!doctype html><html>huge page</html>"), "JBD sign-in is temporarily unavailable. Try again in a moment.");
        assert_eq!(
            token_error(400, r#"{"error":"invalid_grant","error_description":"This session has ended. Sign in again."}"#),
            "This session has ended. Sign in again."
        );
    }

    #[test]
    fn redirect_uri_is_encoded() {
        assert_eq!(encode("http://127.0.0.1:5123/callback"), "http%3A%2F%2F127.0.0.1%3A5123%2Fcallback");
        assert_eq!(decode("a%3Db+c%zz"), "a=b c%zz");
    }

    #[test]
    fn reads_the_callback() {
        let cb = parse_callback("GET /callback?code=abc_-1&state=xyz%3D HTTP/1.1").unwrap();
        assert_eq!(cb, Callback { code: Some("abc_-1".into()), error: None, state: Some("xyz=".into()) });
        let denied = parse_callback("GET /callback?error=access_denied&state=s HTTP/1.1").unwrap();
        assert_eq!(denied.error.as_deref(), Some("access_denied"));
        assert!(parse_callback("GET /favicon.ico HTTP/1.1").is_none());
        assert!(parse_callback("POST /callback?code=a HTTP/1.1").is_none());
    }

    #[test]
    fn random_values_differ() {
        assert_ne!(random_b64(32).unwrap(), random_b64(32).unwrap());
        assert_eq!(random_b64(32).unwrap().len(), 43);
    }

    #[test]
    fn starts_signed_out_without_a_saved_session() {
        let a = JabrodAccount::start(Box::new(MemoryStore::default()));
        assert_eq!(a.status().phase, Phase::SignedOut);
        assert!(!a.status().is_pro());
    }

    #[test]
    fn a_saved_session_opens_at_once_and_a_first_jbd_session_still_loads() {
        // JSON exactly as the first (Tauri) JBD saved it. Verified just now, so even if the
        // background check can't reach jabrod.com, the offline grace keeps it signed in.
        let json = format!(
            r#"{{"refresh_token":"r","account":{{"id":"u1","name":"Ansh","email":"a@jabrod.com","image":null}},"tier":"pro","pro_until":"2027-01-01T00:00:00Z","trial":false,"verified_at":{}}}"#,
            now_secs()
        );
        let store = MemoryStore::default();
        store.save(&json).unwrap();
        let a = JabrodAccount::start(Box::new(store));
        let s = a.status();
        assert_eq!(s.phase, Phase::SignedIn);
        assert_eq!(s.account.as_ref().map(|a| a.name.as_str()), Some("Ansh"));
        assert_eq!(s.tier, Tier::Pro);
    }

    #[test]
    fn sign_in_waits_and_cancel_signs_out() {
        let a = JabrodAccount::start(Box::new(MemoryStore::default()));
        let url = a.sign_in().unwrap();
        assert!(url.contains("/desktop/authorize?client_id=jbd&redirect_uri=http%3A%2F%2F127.0.0.1%3A"), "{url}");
        assert!(url.contains("code_challenge_method=S256"));
        assert_eq!(a.status().phase, Phase::WaitingForBrowser);
        a.cancel_sign_in();
        assert_eq!(a.status().phase, Phase::SignedOut);
    }

    #[test]
    fn the_loopback_listener_answers_the_browser() {
        let a = JabrodAccount::start(Box::new(MemoryStore::default()));
        let url = a.sign_in().unwrap();
        // pull the port out of the redirect_uri
        let port: u16 = url.split("127.0.0.1%3A").nth(1).and_then(|r| r.split('%').next()).and_then(|p| p.parse().ok()).unwrap();
        // a wrong `state` is refused without trading the code
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(b"GET /callback?code=c&state=wrong HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut page = String::new();
        let _ = s.read_to_string(&mut page);
        assert!(page.contains("Sign-in cancelled") && !page.contains("Signed in"), "{page}");
        for _ in 0..50 {
            if a.status().phase == Phase::SignedOut {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let st = a.status();
        assert_eq!(st.phase, Phase::SignedOut);
        assert_eq!(st.message.as_deref(), Some("That sign-in answer wasn't for this request. Try again."));
    }
}
