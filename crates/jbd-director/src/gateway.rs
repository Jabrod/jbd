//! jabrod.com's AI routes, called from the Director's thread (desktop only).
//!
//! - `GET  /api/ai/models`        the model pickers' lists and this month's credits
//! - `POST /api/ai/chat`          one model turn (with tools)
//! - `POST /api/ai/video`         start a video generation (charges its credits up front)
//! - `GET  /api/ai/video/status`  is it done? (a failed job is refunded by the server)
//!
//! The server code is in the `jbd_home` repo (`app/api/ai/*`, `lib/platform/models.ts`). The API
//! keys never leave the server; JBD sends the person's access token.
//!
//! Every call here blocks, so only call it from a worker thread, never from the UI.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::tools::clip as tools_clip;
use crate::{Catalogue, ChatMessage, Credits, Model, Reply, ToolCall, VideoJob, VideoStatus};

/// Gives a fresh access token (`AccountBackend::access_token`, which refreshes when needed).
pub type TokenSource = Arc<dyn Fn() -> Result<String, String> + Send + Sync>;

/// A model turn can take a while: the server tries OpenRouter, then the AI Gateway (55 s each).
const CHAT_TIMEOUT: Duration = Duration::from_secs(130);
const SHORT_TIMEOUT: Duration = Duration::from_secs(40);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// Largest clip JBD will download (a 15 s 1080p clip is ~30 MB).
const MAX_CLIP_BYTES: u64 = 1024 * 1024 * 1024;

pub struct Gateway {
    base: String,
    token: TokenSource,
}

impl Gateway {
    /// `base` is the platform URL (`jbd_account::platform_url()`).
    pub fn new(base: impl Into<String>, token: TokenSource) -> Self {
        Gateway { base: base.into().trim_end_matches('/').to_string(), token }
    }

    fn agent(timeout: Duration) -> ureq::Agent {
        let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
        ureq::Agent::config_builder()
            .tls_config(tls)
            .timeout_global(Some(timeout))
            // read 4xx bodies ourselves: they carry the server's plain-language error
            .http_status_as_error(false)
            .user_agent(concat!("JBD/", env!("CARGO_PKG_VERSION")))
            .build()
            .into()
    }

    fn url(&self, path: &str) -> String {
        let sep = if path.contains('?') { '&' } else { '?' };
        format!("{}{path}{sep}client_id={}", self.base, jbd_account::CLIENT_ID)
    }

    fn get(&self, path: &str, timeout: Duration) -> Result<Value, String> {
        let token = (self.token)()?;
        let mut resp = Self::agent(timeout).get(&self.url(path)).header("Authorization", &format!("Bearer {token}")).call().map_err(offline)?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().read_to_string().map_err(offline)?;
        answer(status, &text)
    }

    fn post(&self, path: &str, body: &Value, timeout: Duration) -> Result<Value, String> {
        let token = (self.token)()?;
        let mut resp = Self::agent(timeout)
            .post(&self.url(path))
            .header("Authorization", &format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .send(body.to_string())
            .map_err(offline)?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().read_to_string().map_err(offline)?;
        answer(status, &text)
    }

    /// The pickers' model lists and this month's credits.
    pub fn catalogue(&self) -> Result<Catalogue, String> {
        let v = self.get("/api/ai/models", SHORT_TIMEOUT)?;
        serde_json::from_value(v).map_err(|e| format!("jabrod.com sent a model list JBD can't read ({e}). Update JBD."))
    }

    /// Save a finished clip: from its URL, or from the bytes the server sent.
    fn save_clip(&self, v: &Value, dir: &Path) -> Result<PathBuf, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
        let path = unique_path(dir);
        if let Some(url) = v.get("url").and_then(Value::as_str) {
            let resp = Self::agent(DOWNLOAD_TIMEOUT).get(url).call().map_err(offline)?;
            if resp.status().as_u16() >= 400 {
                return Err(format!("downloading the clip failed (HTTP {}). Try again.", resp.status().as_u16()));
            }
            let mut file = std::fs::File::create(&path).map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
            let mut reader = resp.into_body().into_reader().take(MAX_CLIP_BYTES);
            std::io::copy(&mut reader, &mut file).map_err(|e| format!("downloading the clip failed: {e}"))?;
        } else if let Some(data) = v.get("data_base64").and_then(Value::as_str) {
            use base64::Engine as _;
            let bytes = base64::engine::general_purpose::STANDARD.decode(data.trim()).map_err(|e| format!("the clip arrived damaged ({e}). Try again."))?;
            std::fs::write(&path, bytes).map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
        } else {
            return Err("the server said the clip was ready but sent no clip".into());
        }
        Ok(path)
    }
}

impl Model for Gateway {
    fn chat(&self, model: &str, messages: &[ChatMessage], tools: &[Value]) -> Result<Reply, String> {
        let body = json!({
            "model": model,
            "messages": messages.iter().map(wire_message).collect::<Vec<_>>(),
            "tools": tools,
            "tool_choice": "auto",
            "max_tokens": 4096,
            "temperature": 0.2,
        });
        let v = self.post("/api/ai/chat", &body, CHAT_TIMEOUT)?;
        let tool_calls: Vec<ToolCall> = v.get("tool_calls").cloned().and_then(|t| serde_json::from_value(t).ok()).unwrap_or_default();
        Ok(Reply { content: v.get("content").and_then(Value::as_str).unwrap_or("").to_string(), tool_calls, credits: credits(&v) })
    }

    fn start_video(&self, model: &str, prompt: &str, seconds: u32, aspect: &str, resolution: &str, audio: bool) -> Result<VideoJob, String> {
        let body = json!({"model": model, "prompt": prompt, "seconds": seconds, "aspect": aspect, "resolution": resolution, "audio": audio});
        let v = self.post("/api/ai/video", &body, SHORT_TIMEOUT)?;
        let job = v.get("job").and_then(Value::as_str).ok_or("the server didn't start the clip. Try again.")?;
        let charged = v.get("credits").and_then(Value::as_u64).unwrap_or(0);
        Ok(VideoJob { job: job.to_string(), credits: u32::try_from(charged).unwrap_or(u32::MAX) })
    }

    fn video_status(&self, job: &str, into: &Path) -> Result<VideoStatus, String> {
        let v = self.get(&format!("/api/ai/video/status?job={}", encode(job)), SHORT_TIMEOUT)?;
        match v.get("status").and_then(Value::as_str) {
            Some("pending") => Ok(VideoStatus::Pending),
            Some("completed") => self.save_clip(&v, into).map(VideoStatus::Done),
            Some("error") => Ok(VideoStatus::Failed(v.get("error").and_then(Value::as_str).unwrap_or("the video model failed").to_string())),
            _ => Err("jabrod.com sent an answer JBD can't read".into()),
        }
    }

    fn web(&self, tool: &str, args: Value) -> Result<Value, String> {
        self.post("/api/ai/tools", &json!({"tool": tool, "args": args}), Duration::from_secs(70))
    }

    fn speech(&self, model: &str, text: &str, voice: Option<&str>, instructions: Option<&str>, dir: &Path) -> Result<(PathBuf, u32), String> {
        let mut body = json!({"model": model, "text": text});
        if let Some(v) = voice.filter(|v| !v.is_empty()) {
            body["voice"] = json!(v);
        }
        if let Some(i) = instructions.filter(|i| !i.is_empty()) {
            body["instructions"] = json!(tools_clip(i, 300));
        }
        let v = self.post("/api/ai/speech", &body, Duration::from_secs(70))?;
        let data = v.get("data_base64").and_then(Value::as_str).ok_or("the voice service sent no audio. Try again.")?;
        let ext = match v.get("format").and_then(Value::as_str).unwrap_or("mp3") {
            "wav" => "wav",
            "ogg" | "opus" => "ogg",
            "flac" => "flac",
            _ => "mp3",
        };
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD.decode(data.trim()).map_err(|e| format!("the voiceover arrived damaged ({e}). Try again."))?;
        std::fs::create_dir_all(dir).map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let mut path = dir.join(format!("JBD voiceover {}.{ext}", stamp(secs)));
        let mut n = 2;
        while path.exists() && n < 1000 {
            path = dir.join(format!("JBD voiceover {} ({n}).{ext}", stamp(secs)));
            n += 1;
        }
        std::fs::write(&path, bytes).map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
        let charged = v.get("credits").and_then(Value::as_u64).unwrap_or(0);
        Ok((path, u32::try_from(charged).unwrap_or(u32::MAX)))
    }

    fn download_media(&self, url: &str, dir: &Path) -> Result<PathBuf, String> {
        if !url.starts_with("https://") {
            return Err("only https links can be imported".into());
        }
        let resp = Self::agent(DOWNLOAD_TIMEOUT).get(url).call().map_err(offline)?;
        if resp.status().as_u16() >= 400 {
            return Err(format!("the link answered HTTP {}", resp.status().as_u16()));
        }
        let kind = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_ascii_lowercase();
        let (ext, what, cap) = media_kind(kind.split(';').next().unwrap_or("").trim(), url)?;
        std::fs::create_dir_all(dir).map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let mut path = dir.join(format!("JBD {what} {}.{ext}", stamp(secs)));
        let mut n = 2;
        while path.exists() && n < 1000 {
            path = dir.join(format!("JBD {what} {} ({n}).{ext}", stamp(secs)));
            n += 1;
        }
        let mut file = std::fs::File::create(&path).map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
        let mut reader = resp.into_body().into_reader().take(cap);
        std::io::copy(&mut reader, &mut file).map_err(|e| format!("downloading failed: {e}"))?;
        Ok(path)
    }
}

/// What a download is, from its content type (or, for servers that say only
/// "application/octet-stream", the link's extension): file extension, a word for the file name,
/// and the largest size JBD accepts for it.
fn media_kind(kind: &str, url: &str) -> Result<(&'static str, &'static str, u64), String> {
    const MB: u64 = 1024 * 1024;
    let by_type = match kind {
        "image/jpeg" | "image/jpg" => Some(("jpg", "image", 25 * MB)),
        "image/png" => Some(("png", "image", 25 * MB)),
        "image/webp" => Some(("webp", "image", 25 * MB)),
        "image/gif" => Some(("gif", "image", 25 * MB)),
        "audio/mpeg" | "audio/mp3" => Some(("mp3", "audio", 100 * MB)),
        "audio/wav" | "audio/x-wav" | "audio/wave" => Some(("wav", "audio", 200 * MB)),
        "audio/ogg" | "application/ogg" => Some(("ogg", "audio", 100 * MB)),
        "audio/flac" | "audio/x-flac" => Some(("flac", "audio", 200 * MB)),
        "audio/mp4" | "audio/aac" | "audio/x-m4a" => Some(("m4a", "audio", 100 * MB)),
        "video/mp4" => Some(("mp4", "video", 500 * MB)),
        "video/webm" => Some(("webm", "video", 500 * MB)),
        "video/quicktime" => Some(("mov", "video", 500 * MB)),
        _ => None,
    };
    if let Some(k) = by_type {
        return Ok(k);
    }
    let path = url.split(['?', '#']).next().unwrap_or(url).to_ascii_lowercase();
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "jpg" | "jpeg" => Ok(("jpg", "image", 25 * MB)),
        "png" => Ok(("png", "image", 25 * MB)),
        "mp3" => Ok(("mp3", "audio", 100 * MB)),
        "wav" => Ok(("wav", "audio", 200 * MB)),
        "ogg" | "oga" => Ok(("ogg", "audio", 100 * MB)),
        "flac" => Ok(("flac", "audio", 200 * MB)),
        "mp4" => Ok(("mp4", "video", 500 * MB)),
        "webm" => Ok(("webm", "video", 500 * MB)),
        _ => Err(format!("that link isn't a picture, sound or video JBD can use ({kind})")),
    }
}

#[cfg(test)]
mod media_kind_tests {
    use super::media_kind;

    #[test]
    fn content_types_and_extensions_decide_the_file_kind() {
        assert_eq!(media_kind("video/webm", "https://x/y").map(|k| k.0), Ok("webm"));
        assert_eq!(media_kind("audio/mpeg; charset=x", "https://x/y").map(|k| k.1).ok(), None, "callers pass the bare type");
        assert_eq!(media_kind("audio/mpeg", "https://x/y").map(|k| k.1), Ok("audio"));
        // a server that only says octet-stream: the link's extension decides
        assert_eq!(media_kind("application/octet-stream", "https://cdn/sound.ogg?dl=1").map(|k| k.0), Ok("ogg"));
        assert!(media_kind("text/html", "https://example.com/page").is_err());
    }
}

/// A message as the chat API takes it: a message with pictures becomes OpenAI content parts
/// (its text, then one `image_url` part per picture); every other message is sent as it is.
fn wire_message(m: &ChatMessage) -> Value {
    let mut v = serde_json::to_value(m).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.remove("images");
        if !m.images.is_empty() {
            let mut parts = vec![json!({"type": "text", "text": m.content.clone().unwrap_or_default()})];
            parts.extend(m.images.iter().map(|url| json!({"type": "image_url", "image_url": {"url": url}})));
            o.insert("content".into(), Value::Array(parts));
        }
    }
    v
}

/// `usage.credits` from any `/api/ai/*` answer.
fn credits(v: &Value) -> Option<Credits> {
    v.get("usage").and_then(|u| u.get("credits")).cloned().and_then(|c| serde_json::from_value(c).ok())
}

/// Turn a response into its JSON, or the server's own explanation of what went wrong.
fn answer(status: u16, text: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    if status < 400 {
        return if v.is_null() { Err("jabrod.com sent an answer JBD can't read. Try again shortly.".into()) } else { Ok(v) };
    }
    if let Some(d) = v.get("error_description").and_then(Value::as_str) {
        return Err(d.to_string());
    }
    Err(match (status, v.get("error").and_then(Value::as_str)) {
        (401, _) => "You're signed out of your Jabrod account. Sign in again from the account menu.".into(),
        (429, _) => "You've used this month's AI credits.".into(),
        (404, _) => "This version of jabrod.com doesn't have the Director yet.".into(),
        (_, Some(e)) => format!("jabrod.com said: {e} (HTTP {status})."),
        _ => format!("jabrod.com answered HTTP {status}. Try again shortly."),
    })
}

fn offline(e: impl std::fmt::Display) -> String {
    format!("Couldn't reach jabrod.com. Check your internet connection. ({e})")
}

/// Percent-encode a query value (the job token is a JWT: letters, digits, `-`, `_`, `.`).
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// `JBD generated 2026-10-15 14-03-22.mp4` (UTC), or with ` (2)` if that exists.
fn unique_path(dir: &Path) -> PathBuf {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let stem = format!("JBD generated {}", stamp(secs));
    let mut path = dir.join(format!("{stem}.mp4"));
    let mut n = 2;
    while path.exists() && n < 1000 {
        path = dir.join(format!("{stem} ({n}).mp4"));
        n += 1;
    }
    path
}

/// Unix seconds → `2026-10-15 14-03-22` (UTC; no `:` because Windows file names can't have it).
/// Howard Hinnant's days-to-civil algorithm, so no date crate is needed for one file name.
fn stamp(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}-{:02}-{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_carry_a_readable_date() {
        assert_eq!(stamp(0), "1970-01-01 00-00-00");
        assert_eq!(stamp(1_792_009_846), "2026-10-14 20-30-46");
        assert_eq!(stamp(951_782_400), "2000-02-29 00-00-00");
    }

    #[test]
    fn server_errors_become_plain_messages() {
        assert_eq!(
            answer(429, r#"{"error":"limit_reached","error_description":"You've used this month's 250 credits."}"#).unwrap_err(),
            "You've used this month's 250 credits."
        );
        assert!(answer(401, "").unwrap_err().contains("Sign in again"));
        assert!(answer(502, "<html>").unwrap_err().contains("502"));
        assert!(answer(200, "not json").is_err());
        assert_eq!(answer(200, r#"{"a":1}"#).unwrap()["a"], 1);
    }

    #[test]
    fn credits_are_read_from_usage() {
        let v = json!({"usage": {"calls": 3, "limit": 2000, "credits": {"used": 12, "limit": 250}}});
        assert_eq!(credits(&v), Some(Credits { used: 12, limit: 250 }));
        assert_eq!(credits(&json!({})), None);
    }

    #[test]
    fn job_tokens_survive_the_query_string() {
        assert_eq!(encode("eyJ.abc-_~"), "eyJ.abc-_~");
        assert_eq!(encode("a+b/c="), "a%2Bb%2Fc%3D");
    }

    #[test]
    fn urls_carry_the_client_id() {
        let g = Gateway::new("http://localhost:3010/", Arc::new(|| Ok("t".into())));
        assert_eq!(g.url("/api/ai/models"), "http://localhost:3010/api/ai/models?client_id=jbd");
        assert_eq!(g.url("/api/ai/video/status?job=x"), "http://localhost:3010/api/ai/video/status?job=x&client_id=jbd");
    }

    #[test]
    fn a_clip_sent_as_bytes_is_written_to_disk() {
        use base64::Engine as _;
        let dir = std::env::temp_dir().join(format!("jbd-director-test-{}", std::process::id()));
        let g = Gateway::new("http://unused", Arc::new(|| Ok("t".into())));
        let data = base64::engine::general_purpose::STANDARD.encode(b"fake mp4");
        let path = g.save_clip(&json!({"status": "completed", "data_base64": data}), &dir).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"fake mp4");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
