//! The desktop client: one blocking method per `/api/art` route. Every call takes a few hundred
//! milliseconds to a minute (image generation), so run it on a worker thread and hand the result
//! back to the UI, the way `jbd-account`'s backend does.
//!
//! The access token lasts 15 minutes. Make a `Client` with the current token for each piece of
//! work (`Client::new(token)`); on `ArtError::is_signed_out()` get a fresh token from the account
//! backend and try once more.
//!
//! Change the common things here: timeouts in `agent()`, the user agent string, and the routes
//! (each method is a few lines around `get`/`send`).

use std::io::Read;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{
    ArtError, Billing, Board, BoardChange, BoardSummary, Chat, ChatThread, ImageRequest, ImageResult, LibraryAsset, Me, MediaKind, Models, Saved,
    SpeechRequest, SpeechResult, Utf8Stream, VideoJob, VideoRequest, VideoStatus, path_id, query,
};

/// Long enough for image generation and a long chat reply; connecting fails much sooner.
const CALL_TIMEOUT: Duration = Duration::from_secs(300);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Answers bigger than this are refused (a board document is well under it).
const MAX_ANSWER_BYTES: u64 = 32 * 1024 * 1024;

fn agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_global(Some(CALL_TIMEOUT))
        .timeout_connect(Some(CONNECT_TIMEOUT))
        // read 4xx bodies ourselves: they carry the error sentence
        .http_status_as_error(false)
        .user_agent(concat!("ArtStudio/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// A connection to the Art API as one signed-in person.
pub struct Client {
    base: String,
    token: String,
    agent: ureq::Agent,
}

enum Method {
    Post,
    Put,
    Patch,
}

impl Client {
    /// `token`: the desktop access token from `jbd-account`.
    pub fn new(token: impl Into<String>) -> Self {
        Self::with_base(crate::api_base(), token)
    }

    /// Against another server (tests, a staging deployment).
    pub fn with_base(base: impl Into<String>, token: impl Into<String>) -> Self {
        Client { base: base.into().trim_end_matches('/').to_string(), token: token.into(), agent: agent() }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn auth(&self) -> String {
        format!("Bearer {}", self.token)
    }

    /// Status and body of an answer, or Network when there was none.
    fn finish(resp: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<(u16, String), ArtError> {
        let mut resp = resp.map_err(|_| ArtError::Network)?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().with_config().limit(MAX_ANSWER_BYTES).read_to_string().map_err(|_| ArtError::Network)?;
        Ok((status, text))
    }

    fn parse<T: DeserializeOwned>((status, text): (u16, String)) -> Result<T, ArtError> {
        if !(200..300).contains(&status) {
            return Err(ArtError::from_answer(status, &text));
        }
        serde_json::from_str(&text).map_err(|_| ArtError::BadAnswer)
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, ArtError> {
        Self::parse(Self::finish(self.agent.get(&self.url(path)).header("Authorization", &self.auth()).call())?)
    }

    fn delete(&self, path: &str) -> Result<(), ArtError> {
        let (status, text) = Self::finish(self.agent.delete(&self.url(path)).header("Authorization", &self.auth()).call())?;
        if (200..300).contains(&status) { Ok(()) } else { Err(ArtError::from_answer(status, &text)) }
    }

    fn send_raw(&self, method: Method, path: &str, body: &impl Serialize) -> Result<(u16, String), ArtError> {
        let json = serde_json::to_string(body).map_err(|_| ArtError::BadAnswer)?;
        let url = self.url(path);
        let auth = self.auth();
        let resp = match method {
            Method::Post => self.agent.post(&url).header("Authorization", &auth).header("Content-Type", "application/json").send(json),
            Method::Put => self.agent.put(&url).header("Authorization", &auth).header("Content-Type", "application/json").send(json),
            Method::Patch => self.agent.patch(&url).header("Authorization", &auth).header("Content-Type", "application/json").send(json),
        };
        Self::finish(resp)
    }

    fn send<T: DeserializeOwned>(&self, method: Method, path: &str, body: &impl Serialize) -> Result<T, ArtError> {
        Self::parse(self.send_raw(method, path, body)?)
    }

    // ── who and what ────────────────────────────────────────────────────────

    /// The signed-in person and this month's credits. A 401 means the token has run out.
    pub fn me(&self) -> Result<crate::Me, ArtError> {
        self.get::<Me>("/me")
    }

    /// The models each studio can use, with prices. Public: works signed out too.
    pub fn models(&self) -> Result<Models, ArtError> {
        self.get("/models")
    }

    // ── boards ──────────────────────────────────────────────────────────────

    pub fn boards(&self) -> Result<Vec<BoardSummary>, ArtError> {
        #[derive(serde::Deserialize)]
        struct Wire {
            boards: Vec<BoardSummary>,
        }
        Ok(self.get::<Wire>("/boards")?.boards)
    }

    pub fn create_board(&self, title: &str) -> Result<Board, ArtError> {
        #[derive(serde::Deserialize)]
        struct Wire {
            board: Board,
        }
        Ok(self.send::<Wire>(Method::Post, "/boards", &serde_json::json!({ "title": title }))?.board)
    }

    pub fn board(&self, id: &str) -> Result<Board, ArtError> {
        #[derive(serde::Deserialize)]
        struct Wire {
            board: Board,
        }
        Ok(self.get::<Wire>(&format!("/boards/{}", path_id(id)?))?.board)
    }

    /// Save a change. `updated_at`: the board's `updatedAt` from when it was opened (or the last
    /// save), so a newer save from elsewhere is never overwritten.
    pub fn save_board(&self, id: &str, change: &BoardChange, updated_at: &str) -> Result<Saved, ArtError> {
        let mut body = serde_json::to_value(change).map_err(|_| ArtError::BadAnswer)?;
        if let Some(obj) = body.as_object_mut() {
            obj.insert("updatedAt".into(), serde_json::Value::String(updated_at.to_string()));
        }
        let (status, text) = self.send_raw(Method::Put, &format!("/boards/{}", path_id(id)?), &body)?;
        if status == 409 {
            return Ok(Saved::Conflict);
        }
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire {
            updated_at: String,
        }
        Ok(Saved::At(Self::parse::<Wire>((status, text))?.updated_at))
    }

    pub fn delete_board(&self, id: &str) -> Result<(), ArtError> {
        self.delete(&format!("/boards/{}", path_id(id)?))
    }

    // ── library ─────────────────────────────────────────────────────────────

    /// Newest first, a page at a time: pass the last asset's `created_at` as `before` for the next page.
    pub fn library(&self, kind: Option<MediaKind>, before: Option<&str>) -> Result<Vec<LibraryAsset>, ArtError> {
        #[derive(serde::Deserialize)]
        struct Wire {
            assets: Vec<LibraryAsset>,
        }
        let q = query(&[("kind", kind.map(MediaKind::as_str)), ("before", before)]);
        Ok(self.get::<Wire>(&format!("/assets{q}"))?.assets.into_iter().filter(|a| a.asset.kind != MediaKind::Other).collect())
    }

    // ── generation ──────────────────────────────────────────────────────────

    /// Images (stored in the Library, and on the board when `board_id` is set). Can take a minute.
    pub fn generate_image(&self, req: &ImageRequest) -> Result<ImageResult, ArtError> {
        self.send(Method::Post, "/generate/image", req)
    }

    pub fn generate_speech(&self, req: &SpeechRequest) -> Result<SpeechResult, ArtError> {
        self.send(Method::Post, "/generate/speech", req)
    }

    /// Starts a video; poll [`Client::video_status`] every 5 seconds or so.
    pub fn start_video(&self, req: &VideoRequest) -> Result<VideoJob, ArtError> {
        self.send(Method::Post, "/generate/video", req)
    }

    /// `prompt` is stored with the finished video in the Library (first 300 characters).
    pub fn video_status(&self, job: &str, board_id: Option<&str>, prompt: &str) -> Result<VideoStatus, ArtError> {
        let short: String = prompt.chars().take(300).collect();
        let q = query(&[("job", Some(job)), ("board", board_id), ("prompt", Some(short.as_str()))]);
        self.get(&format!("/generate/video/status{q}"))
    }

    // ── chat ────────────────────────────────────────────────────────────────

    /// Chats, newest first; `search` matches titles and messages, `board_id` keeps one board's.
    pub fn chats(&self, search: Option<&str>, board_id: Option<&str>) -> Result<Vec<Chat>, ArtError> {
        #[derive(serde::Deserialize)]
        struct Wire {
            chats: Vec<Chat>,
        }
        Ok(self.get::<Wire>(&format!("/chats{}", query(&[("q", search), ("board", board_id)])))?.chats)
    }

    pub fn create_chat(&self, model: Option<&str>, board_id: Option<&str>) -> Result<Chat, ArtError> {
        #[derive(serde::Deserialize)]
        struct Wire {
            chat: Chat,
        }
        Ok(self.send::<Wire>(Method::Post, "/chats", &serde_json::json!({ "model": model, "boardId": board_id }))?.chat)
    }

    pub fn chat(&self, id: &str) -> Result<ChatThread, ArtError> {
        self.get(&format!("/chats/{}", path_id(id)?))
    }

    pub fn rename_chat(&self, id: &str, title: &str) -> Result<Chat, ArtError> {
        #[derive(serde::Deserialize)]
        struct Wire {
            chat: Chat,
        }
        Ok(self.send::<Wire>(Method::Patch, &format!("/chats/{}", path_id(id)?), &serde_json::json!({ "title": title }))?.chat)
    }

    pub fn delete_chat(&self, id: &str) -> Result<(), ArtError> {
        self.delete(&format!("/chats/{}", path_id(id)?))
    }

    /// Send a message; the reply streams into `on_delta` piece by piece as it is written. Return
    /// `false` from `on_delta` to stop early (Stop button). Resolves with the chat's title (set
    /// by the server after the first message).
    pub fn send_message(&self, chat_id: &str, content: &str, model: &str, mut on_delta: impl FnMut(&str) -> bool) -> Result<String, ArtError> {
        let json = serde_json::json!({ "content": content, "model": model }).to_string();
        let mut resp = self
            .agent
            .post(&self.url(&format!("/chats/{}/messages", path_id(chat_id)?)))
            .header("Authorization", &self.auth())
            .header("Content-Type", "application/json")
            .send(json)
            .map_err(|_| ArtError::Network)?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let text = resp.body_mut().read_to_string().unwrap_or_default();
            return Err(ArtError::from_answer(status, &text));
        }
        let title = resp.headers().get("x-chat-title").and_then(|v| v.to_str().ok()).map(percent_decode).unwrap_or_default();
        let mut reader = resp.body_mut().as_reader();
        let mut buf = [0u8; 4096];
        let mut text = Utf8Stream::default();
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => return Err(ArtError::Api { status: 0, code: "cut_off".into(), message: "The reply was cut off. Try again.".into() }),
            };
            let piece = text.push(buf.get(..n).unwrap_or_default());
            if !piece.is_empty() && !on_delta(&piece) {
                return Ok(title);
            }
        }
        let rest = text.finish();
        if !rest.is_empty() {
            on_delta(&rest);
        }
        Ok(title)
    }

    // ── Art Pro ─────────────────────────────────────────────────────────────

    pub fn billing(&self) -> Result<Billing, ArtError> {
        self.get("/billing")
    }

    /// A checkout page to open in the browser. `currency`: "INR"/"USD"; `interval`: "month"/"year".
    pub fn checkout_url(&self, interval: &str, currency: &str) -> Result<String, ArtError> {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire {
            checkout_url: String,
        }
        let body = serde_json::json!({ "interval": interval, "currency": currency, "returnPath": "/studio" });
        Ok(self.send::<Wire>(Method::Post, "/billing/checkout", &body)?.checkout_url)
    }

    /// The customer portal (payment method, invoices, cancel), to open in the browser.
    pub fn billing_portal(&self) -> Result<String, ArtError> {
        #[derive(serde::Deserialize)]
        struct Wire {
            url: String,
        }
        Ok(self.send::<Wire>(Method::Post, "/billing/portal", &serde_json::json!({}))?.url)
    }
}

/// `%XX` → bytes → text (the chat title header is percent-encoded UTF-8).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%'
            && let (Some(h), Some(l)) = (bytes.get(i + 1).and_then(|c| (*c as char).to_digit(16)), bytes.get(i + 2).and_then(|c| (*c as char).to_digit(16)))
        {
            out.push(u8::try_from(h * 16 + l).unwrap_or(b'?'));
            i += 3;
            continue;
        }
        out.push(b);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_chat_title_header() {
        assert_eq!(percent_decode("Poster%20ideas%20%E2%80%94%20Diwali"), "Poster ideas — Diwali");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn base_url_has_no_trailing_slash() {
        let c = Client::with_base("http://localhost:3000/api/art/", "t");
        assert_eq!(c.url("/me"), "http://localhost:3000/api/art/me");
    }

    #[test]
    fn bad_ids_never_reach_the_network() {
        // an unreachable base: if the id check let it through, this would be Network instead
        let c = Client::with_base("http://127.0.0.1:9", "t");
        assert!(matches!(c.board("../me"), Err(ArtError::Api { status: 400, .. })));
        assert!(matches!(c.delete_chat("a/b"), Err(ArtError::Api { status: 400, .. })));
    }

    #[test]
    fn unreachable_server_is_a_network_error() {
        let c = Client::with_base("http://127.0.0.1:9", "t");
        assert_eq!(c.me(), Err(ArtError::Network));
    }
}
