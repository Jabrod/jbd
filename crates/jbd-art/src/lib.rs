//! Art Studio's connection to Jabrod Art: the same `/api/art/*` routes the web studio at
//! art.jabrod.com/studio uses, called with the desktop app's own sign-in (the 15-minute access
//! token from `jbd-account`). jabrod.com checks that token in `artUser()` (jbd_home,
//! lib/platform/art.ts), so a person sees the same boards, Library, chats and credits on the web
//! and in the app.
//!
//! What is here:
//! - the types the API sends and takes (this file), mirroring `lib/store/types.ts` in the open
//!   source studio (github.com/Jabrod/art). Fields the app doesn't use yet are skipped by serde,
//!   so a newer server never breaks an older app;
//! - [`ArtError`]: every failure, always one short sentence that can be shown as it is;
//! - [`native::Client`] (desktop only): one blocking method per route. Call it from a worker
//!   thread, never from the UI thread.
//!
//! Not here yet: uploads (Vercel Blob's client-upload handshake; ticket A2 in
//! docs/jbd/ART-STUDIO-WINDOWS.md) and the board canvas document, kept as raw JSON until the
//! native canvas (A4) gives it types.
//!
//! To add a route: add its request/response types below, then a method on `Client` that calls
//! `get`/`send` with the path, and a parse test with a JSON sample in `tests` at the bottom.

use std::fmt;

use serde::{Deserialize, Serialize};

#[cfg(not(target_arch = "wasm32"))]
pub mod native;

/// Where the Art API lives: `<platform>/api/art`, so `JBD_PLATFORM_URL` (and debug builds'
/// localhost) point it at the same server as sign-in.
pub fn api_base() -> String {
    format!("{}/api/art", jbd_account::platform_url())
}

// ── Errors ─────────────────────────────────────────────────────────────────

/// Why a call failed. `Display` is the sentence to show the person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArtError {
    /// jabrod.com couldn't be reached (offline, DNS, timeout).
    Network,
    /// The server said no, with its own message (`{ error, error_description }`).
    Api { status: u16, code: String, message: String },
    /// The answer wasn't what this app understands (a proxy page, an older server).
    BadAnswer,
}

impl ArtError {
    /// 401: the access token is missing or expired. The caller asks `jbd-account` for a fresh
    /// one and tries once more, or shows the sign-in screen.
    pub fn is_signed_out(&self) -> bool {
        matches!(self, ArtError::Api { status: 401, .. })
    }

    /// 402: out of credits (the message says what to do).
    pub fn is_out_of_credits(&self) -> bool {
        matches!(self, ArtError::Api { status: 402, .. })
    }

    /// Build the error for a non-2xx answer. The body is shown only when it is the API's own
    /// JSON error: a proxy or a stale deployment can answer with a whole HTML page; never show that.
    pub fn from_answer(status: u16, body: &str) -> ArtError {
        #[derive(Deserialize)]
        struct Wire {
            error: Option<String>,
            error_description: Option<String>,
        }
        if let Ok(w) = serde_json::from_str::<Wire>(body)
            && let Some(message) = w.error_description.filter(|m| !m.trim().is_empty())
        {
            return ArtError::Api { status, code: w.error.unwrap_or_else(|| "error".into()), message: message.chars().take(300).collect() };
        }
        let message = match status {
            401 => "Sign in to your Jabrod account to use Art Studio.".to_string(),
            404 => "Art Studio's server doesn't know this yet. Update Art Studio, then try again.".to_string(),
            429 => "Too many requests at once. Wait a moment, then try again.".to_string(),
            500..=599 => "Jabrod is having trouble right now. Try again in a moment.".to_string(),
            _ => format!("Something went wrong ({status}). Try again."),
        };
        ArtError::Api { status, code: "error".into(), message }
    }
}

impl fmt::Display for ArtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArtError::Network => f.write_str("Couldn't reach Jabrod. Check your connection and try again."),
            ArtError::Api { message, .. } => f.write_str(message),
            ArtError::BadAnswer => f.write_str("Jabrod sent an answer Art Studio doesn't understand. Update Art Studio, then try again."),
        }
    }
}

impl std::error::Error for ArtError {}

// ── The signed-in person ───────────────────────────────────────────────────

/// `GET /me`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Me {
    pub signed_in: bool,
    pub user: Option<MeUser>,
    pub credits: Option<Credits>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct MeUser {
    pub name: String,
    pub email: String,
}

/// This month's credits: Art's free allowance, plus the Pro pool for Pro members.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Credits {
    pub used: u64,
    pub limit: u64,
    pub left: u64,
    /// Sent by `/me` only; generation answers leave it out.
    pub pro: bool,
}

// ── Models ─────────────────────────────────────────────────────────────────

/// `GET /models`: what each studio can use, with prices in credits.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Models {
    pub chat: Vec<ChatModel>,
    pub image: Vec<ImageModel>,
    pub video: Vec<VideoModel>,
    pub voice: Vec<VoiceModel>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ChatModel {
    pub id: String,
    pub label: String,
    pub vendor: String,
    /// "fast" or "smart"
    pub tier: String,
    pub default: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ImageModel {
    pub id: String,
    pub label: String,
    pub vendor: String,
    pub credits: u64,
    pub aspects: Vec<String>,
    pub default: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VideoModel {
    pub id: String,
    pub label: String,
    pub vendor: String,
    pub durations: Vec<u32>,
    pub resolutions: Vec<String>,
    pub aspects: Vec<String>,
    pub audio: bool,
    /// credits for one second, by resolution (silent)
    pub credits_per_second: std::collections::BTreeMap<String, f64>,
    /// credits for one second, by resolution, with audio
    pub audio_credits_per_second: std::collections::BTreeMap<String, f64>,
    pub default: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VoiceModel {
    pub id: String,
    pub label: String,
    pub voices: Vec<String>,
    /// credits for 1,000 characters
    pub credits_per1k: f64,
    pub default: bool,
}

/// The model marked default, else the first one.
pub fn default_of<T>(models: &[T], is_default: impl Fn(&T) -> bool) -> Option<&T> {
    models.iter().find(|m| is_default(m)).or_else(|| models.first())
}

// ── Media ──────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    #[default]
    Image,
    Video,
    Audio,
    /// A kind this app doesn't know yet (skipped in lists).
    #[serde(other)]
    Other,
}

impl MediaKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MediaKind::Image => "image",
            MediaKind::Video => "video",
            MediaKind::Audio => "audio",
            MediaKind::Other => "other",
        }
    }
}

/// A stored file: uploaded or generated, always at a public Blob URL.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AssetRef {
    pub id: String,
    pub kind: MediaKind,
    pub url: String,
    pub name: Option<String>,
    pub mime: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
}

/// A file in the Library (`GET /assets`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LibraryAsset {
    #[serde(flatten)]
    pub asset: AssetRef,
    /// "upload" or "generated"
    pub source: String,
    pub prompt: Option<String>,
    pub model: Option<String>,
    pub board_id: Option<String>,
    pub created_at: String,
}

// ── Boards ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BoardSummary {
    pub id: String,
    pub title: String,
    pub updated_at: String,
    pub thumbnail_url: Option<String>,
}

/// A whole board. `doc` is the canvas (nodes, edges, viewport) as the web studio stores it
/// (`lib/board.ts`); kept as JSON until the native canvas gives it types.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Board {
    pub id: String,
    pub title: String,
    pub doc: serde_json::Value,
    pub updated_at: String,
}

/// What to change in `PUT /boards/:id` (unset fields stay as they are).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardChange {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<serde_json::Value>,
}

/// How a save went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Saved {
    /// Saved; the board's new `updatedAt` (send it with the next save).
    At(String),
    /// Someone saved this board elsewhere (the web, another PC) since it was opened: reload it
    /// before saving, or the other change would be lost.
    Conflict,
}

// ── Generation ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageRequest {
    pub board_id: Option<String>,
    pub prompt: String,
    pub model: String,
    pub aspect: String,
    pub count: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechRequest {
    pub board_id: Option<String>,
    pub text: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoRequest {
    pub board_id: Option<String>,
    pub prompt: String,
    pub model: String,
    pub seconds: u32,
    pub aspect: String,
    pub resolution: String,
    pub audio: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ImageResult {
    pub assets: Vec<AssetRef>,
    pub credits: Credits,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct SpeechResult {
    pub asset: AssetRef,
    pub credits: Credits,
}

/// A started video: poll [`native::Client::video_status`] with `job` every few seconds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct VideoJob {
    pub job: String,
    pub credits: Credits,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum VideoStatus {
    Pending,
    Done { asset: AssetRef },
    Error { error: String },
}

// ── Chat ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Chat {
    pub id: String,
    pub title: String,
    pub model: String,
    pub board_id: Option<String>,
    pub updated_at: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatMessage {
    pub id: String,
    /// "user" or "assistant"
    pub role: String,
    pub content: String,
    pub model: Option<String>,
    pub created_at: String,
}

/// `GET /chats/:id`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ChatThread {
    pub chat: Chat,
    pub messages: Vec<ChatMessage>,
}

/// Turns streamed bytes into text without breaking a character split across two reads (a
/// Hindi letter or an emoji is 3-4 bytes; the network can cut anywhere).
#[derive(Default)]
pub struct Utf8Stream {
    pending: Vec<u8>,
}

impl Utf8Stream {
    /// Add bytes; returns the text that is complete so far.
    pub fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let valid = match std::str::from_utf8(&self.pending) {
            Ok(_) => self.pending.len(),
            // an incomplete character at the end waits for the next read; a truly invalid byte
            // (error_len is Some) is replaced so the stream can't stall on it
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            Err(_) => {
                let text = String::from_utf8_lossy(&self.pending).into_owned();
                self.pending.clear();
                return text;
            }
        };
        let rest = self.pending.split_off(valid);
        let text = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending = rest;
        text
    }

    /// What's left at the end of the stream (an incomplete character becomes U+FFFD).
    pub fn finish(&mut self) -> String {
        let text = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        text
    }
}

// ── Art Pro ────────────────────────────────────────────────────────────────

/// `GET /billing`: the plan and prices (minor units: paise / cents).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Billing {
    /// "INR" or "USD": the one to show first
    pub currency: String,
    /// currency → interval ("month"/"year") → price in minor units
    pub prices: std::collections::BTreeMap<String, std::collections::BTreeMap<String, u64>>,
    pub available: std::collections::BTreeMap<String, std::collections::BTreeMap<String, bool>>,
    /// "test" or "live"
    pub mode: String,
    pub plan: Plan,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Plan {
    pub pro: bool,
    pub until: Option<String>,
    pub subscribed: bool,
    pub status: Option<String>,
    pub interval: Option<String>,
    pub next_billing_at: Option<String>,
}

// ── URLs ───────────────────────────────────────────────────────────────────

/// Percent-encode one query value (RFC 3986 unreserved characters stay as they are).
pub fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(char::from(b)),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `?a=1&b=2` from the pairs whose value is set; "" when none are.
pub fn query(pairs: &[(&str, Option<&str>)]) -> String {
    let parts: Vec<String> = pairs.iter().filter_map(|(k, v)| v.filter(|v| !v.is_empty()).map(|v| format!("{k}={}", encode(v)))).collect();
    if parts.is_empty() { String::new() } else { format!("?{}", parts.join("&")) }
}

/// An id from the server, checked before it goes into a path (ids are uuids; anything with a
/// slash or a dot could reach another route).
pub fn path_id(id: &str) -> Result<&str, ArtError> {
    if !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        Ok(id)
    } else {
        Err(ArtError::Api { status: 400, code: "bad_id".into(), message: "That item can't be opened. Refresh the list and try again.".into() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_errors_show_the_servers_sentence() {
        let e = ArtError::from_answer(402, r#"{"error":"out_of_credits","error_description":"You've used this month's credits."}"#);
        assert!(e.is_out_of_credits());
        assert_eq!(e.to_string(), "You've used this month's credits.");
    }

    #[test]
    fn errors_never_show_an_html_page() {
        let e = ArtError::from_answer(502, "<!doctype html><html>huge proxy page</html>");
        assert_eq!(e.to_string(), "Jabrod is having trouble right now. Try again in a moment.");
        assert!(ArtError::from_answer(401, "").is_signed_out());
        assert!(!ArtError::from_answer(500, "").is_signed_out());
    }

    #[test]
    fn reads_me() {
        let me: Me = serde_json::from_str(
            r#"{"signedIn":true,"user":{"name":"Ansh","email":"a@b.c"},"credits":{"used":3,"limit":50,"left":47,"pro":false},"jbd":{"hasDevice":true}}"#,
        )
        .unwrap();
        assert!(me.signed_in);
        assert_eq!(me.user.unwrap().name, "Ansh");
        assert_eq!(me.credits.unwrap().left, 47);
        let out: Me = serde_json::from_str(r#"{"signedIn":false}"#).unwrap();
        assert!(!out.signed_in && out.user.is_none());
    }

    #[test]
    fn reads_models_and_picks_the_default() {
        let m: Models = serde_json::from_str(
            r#"{"chat":[{"id":"a","label":"A","vendor":"x","tier":"fast"},{"id":"b","label":"B","vendor":"y","tier":"smart","default":true}],
               "image":[{"id":"i","label":"I","vendor":"v","credits":4,"aspects":["1:1","16:9"]}],
               "video":[{"id":"v","label":"V","vendor":"g","durations":[4,8],"resolutions":["720p"],"aspects":["16:9"],"audio":true,
                         "creditsPerSecond":{"720p":10},"audioCreditsPerSecond":{"720p":15.5}}],
               "voice":[{"id":"t","label":"T","voices":["alloy"],"creditsPer1k":2}],
               "somethingNew":[1]}"#,
        )
        .unwrap();
        assert_eq!(default_of(&m.chat, |c| c.default).unwrap().id, "b");
        assert_eq!(default_of(&m.image, |c| c.default).unwrap().id, "i");
        assert_eq!(m.video[0].audio_credits_per_second.get("720p"), Some(&15.5));
        assert_eq!(m.voice[0].credits_per1k, 2.0);
    }

    #[test]
    fn reads_library_assets_and_unknown_kinds() {
        let list: Vec<LibraryAsset> = serde_json::from_str(
            r#"[{"id":"1","kind":"image","url":"https://x/a.png","width":1024,"height":1024,"source":"generated","prompt":"a cat","createdAt":"2026-11-19T00:00:00Z"},
                {"id":"2","kind":"model3d","url":"https://x/b.glb","source":"upload","createdAt":"2026-11-19T00:00:00Z"}]"#,
        )
        .unwrap();
        assert_eq!(list[0].asset.kind, MediaKind::Image);
        assert_eq!(list[0].asset.width, Some(1024));
        assert_eq!(list[0].prompt.as_deref(), Some("a cat"));
        assert_eq!(list[1].asset.kind, MediaKind::Other);
    }

    #[test]
    fn reads_video_status() {
        assert_eq!(serde_json::from_str::<VideoStatus>(r#"{"status":"pending"}"#).unwrap(), VideoStatus::Pending);
        let done: VideoStatus =
            serde_json::from_str(r#"{"status":"done","asset":{"id":"v","kind":"video","url":"https://x/v.mp4","durationMs":8000}}"#).unwrap();
        assert!(matches!(done, VideoStatus::Done { asset } if asset.duration_ms == Some(8000)));
        let err: VideoStatus = serde_json::from_str(r#"{"status":"error","error":"Blocked by the safety filter."}"#).unwrap();
        assert_eq!(err, VideoStatus::Error { error: "Blocked by the safety filter.".into() });
    }

    #[test]
    fn writes_requests_the_way_the_server_reads_them() {
        let req = ImageRequest { board_id: None, prompt: "p".into(), model: "m".into(), aspect: "1:1".into(), count: 2 };
        assert_eq!(serde_json::to_string(&req).unwrap(), r#"{"boardId":null,"prompt":"p","model":"m","aspect":"1:1","count":2}"#);
        let change = BoardChange { title: Some("New".into()), doc: None };
        assert_eq!(serde_json::to_string(&change).unwrap(), r#"{"title":"New"}"#);
    }

    #[test]
    fn reads_billing() {
        let b: Billing = serde_json::from_str(
            r#"{"currency":"INR","prices":{"INR":{"month":99900,"year":799900}},"available":{"INR":{"month":true,"year":false}},"mode":"live",
               "plan":{"pro":true,"until":null,"subscribed":true,"status":"active","interval":"month","nextBillingAt":"2026-12-19T00:00:00Z"}}"#,
        )
        .unwrap();
        assert_eq!(b.prices["INR"]["month"], 99900);
        assert!(b.plan.pro && b.plan.subscribed);
        assert_eq!(b.plan.next_billing_at.as_deref(), Some("2026-12-19T00:00:00Z"));
    }

    #[test]
    fn utf8_stream_keeps_split_characters_whole() {
        let text = "नमस्ते, hi 🎬";
        let bytes = text.as_bytes();
        let mut s = Utf8Stream::default();
        let mut out = String::new();
        // feed one byte at a time: the worst case
        for b in bytes {
            out.push_str(&s.push(std::slice::from_ref(b)));
        }
        out.push_str(&s.finish());
        assert_eq!(out, text);
        // an invalid byte is replaced instead of stalling the stream
        let mut s = Utf8Stream::default();
        assert_eq!(s.push(&[b'a', 0xFF, b'b']), "a\u{FFFD}b");
    }

    #[test]
    fn query_and_ids() {
        assert_eq!(query(&[("kind", Some("image")), ("before", None), ("q", Some("a b&c"))]), "?kind=image&q=a%20b%26c");
        assert_eq!(query(&[("q", Some(""))]), "");
        assert_eq!(path_id("0f5c-AB_1").unwrap(), "0f5c-AB_1");
        assert!(path_id("../me").is_err());
        assert!(path_id("").is_err());
    }
}
