//! The AI Director: JBD's own agent.
//!
//! You type what you want; the Director plans, then works in steps you can watch, each one a
//! normal JBD command that lands in History and can be undone. It can also generate video clips
//! and drop them on the timeline.
//!
//! How the pieces fit:
//!
//! ```text
//!  Director tab (ui-egui/src/jbd/director.rs)
//!        │ reads every frame            ▲ sets answers to permission asks
//!        ▼                              │
//!   Shared (transcript, to-dos, pending ask, credits)   ← Arc<Mutex<…>>, this crate
//!        ▲
//!        │ the agent thread writes
//!   agent::run  ──Model──► jabrod.com  (/api/ai/chat, /api/ai/video)
//!        │
//!        └──Editor──► the UI thread  ──► Session::execute (undoable, panic-guarded)
//! ```
//!
//! The agent never touches the editor's `Session` itself: it asks through the [`Editor`] trait,
//! which the app answers on the UI thread between frames. That is what keeps JBD responsive and
//! keeps every AI edit on the normal undo path.
//!
//! To change what the Director can do, change [`tools::definitions`] (what the model is offered)
//! and [`tools::run`] (what each one does). To change how it behaves, edit `agent::SYSTEM`.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod agent;
pub mod digest;
pub mod look;
pub mod overlay;
pub mod production;
pub mod tools;

// the production tools against the real editing engine (headless), not fakes
#[cfg(test)]
mod engine_tests;

#[cfg(not(target_arch = "wasm32"))]
pub mod gateway;
#[cfg(not(target_arch = "wasm32"))]
pub mod mcp;

/// How much the Director may do without asking.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// Every tool asks first.
    Ask,
    /// Edits run; anything that spends credits asks first.
    #[default]
    AutoEdits,
    /// Read-only: it looks and proposes, but changes nothing.
    Plan,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Ask => "Ask",
            Mode::AutoEdits => "Auto edits",
            Mode::Plan => "Plan",
        }
    }

    pub const ALL: [Mode; 3] = [Mode::Ask, Mode::AutoEdits, Mode::Plan];
}

/// One line of the to-do list the Director keeps for itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Todo {
    pub text: String,
    pub state: TodoState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TodoState {
    Todo,
    Doing,
    Done,
}

/// How a step ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepState {
    Running,
    Ok,
    Failed,
    /// The person said no (or Plan mode refused it).
    Skipped,
    /// Some of it was done, some wasn't (the result starts "Partly done:" and says what).
    Partial,
}

/// One tool call, as the transcript shows it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Step {
    pub id: u64,
    /// What the user sees on the collapsed line, e.g. `timeline.razor at 4.0 s`.
    pub title: String,
    /// The tool's name and arguments (shown when expanded).
    pub detail: String,
    /// The result or the error (shown when expanded).
    pub result: String,
    pub state: StepState,
    /// Seconds the call took.
    pub took: f32,
    /// Credits this step spent (video and image generation).
    pub credits: u32,
    /// Set while a generated clip is being made, 0.0–1.0 when known.
    pub progress: Option<f32>,
    #[serde(default, skip_serializing)]
    pub expanded: bool,
    /// When it started (seconds since 1970; 0 in chats saved before this existed), so a group
    /// of steps can show the real time it took, thinking between steps included.
    #[serde(default)]
    pub started: f64,
}

/// Now, in seconds since 1970 (for [`Step::started`]).
pub fn now_seconds() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// What the transcript shows, oldest first.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Entry {
    User(String),
    /// Something the Director said.
    Said(String),
    Step(Step),
    /// A request failed before any step ran (no credits, offline, signed out).
    Error(String),
    /// What a request spent, shown small after it: "Used 23 credits: thinking 5, video 18".
    Spent(String),
    /// What the person attached to their message (files and timeline clips), shown as pictures
    /// under it: (project item, label).
    Attached(Vec<(u64, String)>),
}

/// A permission ask waiting for the person to answer.
#[derive(Clone, Debug)]
pub struct Ask {
    pub id: u64,
    /// "Generate video", "Run command".
    pub title: String,
    /// The detail under it: the model, the length, the command's parameters.
    pub detail: String,
    /// What it will cost, 0 when it spends nothing.
    pub credits: u32,
    /// Set by the UI when the person answers.
    pub answer: Option<bool>,
}

/// What a request ended up doing (for the UI's "Undo all").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Run {
    /// Undo steps this request pushed (0 = nothing to undo).
    pub edits: u32,
    pub credits: u32,
}

/// Everything the Director tab draws, written by the agent thread.
///
/// Locked for a moment at a time; never hold it across a network call.
#[derive(Debug, Default)]
pub struct Shared {
    pub entries: Vec<Entry>,
    pub todos: Vec<Todo>,
    /// Set while a request is being worked on.
    pub busy: bool,
    /// The permission ask on screen, if any.
    pub ask: Option<Ask>,
    /// The last finished request, for "Undo all".
    pub last_run: Option<Run>,
    /// Credits as the server last reported them.
    pub credits: Option<Credits>,
    /// What the Director is doing right now, for the status line ("Thinking…").
    pub activity: Option<String>,
    next_id: u64,
}

impl Shared {
    pub fn next_id(&mut self) -> u64 {
        self.next_id = self.next_id.saturating_add(1);
        self.next_id
    }

    pub fn push(&mut self, entry: Entry) {
        // The transcript is not a log: keep it long enough to scroll back through a session.
        const MAX: usize = 400;
        if self.entries.len() >= MAX {
            self.entries.remove(0);
        }
        self.entries.push(entry);
    }

    /// Update a step in place (the agent fills in the result when the call returns).
    pub fn update_step(&mut self, id: u64, f: impl FnOnce(&mut Step)) {
        if let Some(Entry::Step(s)) = self.entries.iter_mut().rev().find(|e| matches!(e, Entry::Step(s) if s.id == id)) {
            f(s);
        }
    }

    pub fn last_step_id(&self) -> Option<u64> {
        self.entries.iter().rev().find_map(|e| match e {
            Entry::Step(s) => Some(s.id),
            _ => None,
        })
    }
}

/// Credits left this month, as `/api/ai/*` reports them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credits {
    pub used: u32,
    pub limit: u32,
}

impl Credits {
    pub fn left(&self) -> u32 {
        self.limit.saturating_sub(self.used)
    }

    pub fn fraction(&self) -> f32 {
        if self.limit == 0 { 0.0 } else { (self.used as f32 / self.limit as f32).clamp(0.0, 1.0) }
    }
}

/// Lock that survives a panic in another thread holding it (house rule: never `unwrap`).
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Stop button / closing the app.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn stop(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn reset(&self) {
        self.0.store(false, Ordering::Relaxed);
    }

    pub fn stopped(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// What the agent is allowed to ask the editor to do.
///
/// The app's implementation sends the request to the UI thread and waits for the answer; the
/// tests use a fake. `method` is a control method (`engine.execute`, `ui.screenshot`); the
/// error is the editor's message, which goes back to the model so it can fix its call.
pub trait Editor: Send + Sync {
    fn call(&self, method: &str, params: Value) -> Result<Value, String>;

    /// Where generated clips are written (the project's folder, or the temp folder).
    fn media_dir(&self) -> std::path::PathBuf;
}

/// Tools from outside JBD the Director may use: the person's MCP connectors (`mcp.rs`). Each
/// call asks first unless the connector is trusted.
pub trait ExternalTools: Send + Sync {
    /// OpenAI-style function definitions (names start with `mcp__`).
    fn definitions(&self) -> Vec<Value>;
    /// Run one; the text goes back to the model.
    fn call(&self, name: &str, args: Value) -> Result<String, String>;
    fn is_trusted(&self, name: &str) -> bool;
}

/// One message in the conversation sent to the model (OpenAI shape).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Pictures the model sees with this message (`data:image/png;base64,…`): contact sheets from
    /// `look`, attached images. The gateway sends them as OpenAI image parts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
}

impl ChatMessage {
    pub fn system(text: impl Into<String>) -> Self {
        Self { role: "system".into(), content: Some(text.into()), tool_calls: None, tool_call_id: None, images: Vec::new() }
    }
    pub fn user(text: impl Into<String>) -> Self {
        Self { role: "user".into(), content: Some(text.into()), tool_calls: None, tool_call_id: None, images: Vec::new() }
    }
    pub fn tool(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self { role: "tool".into(), content: Some(text.into()), tool_calls: None, tool_call_id: Some(id.into()), images: Vec::new() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(default = "function_type")]
    pub r#type: String,
    pub function: FunctionCall,
}

fn function_type() -> String {
    "function".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    /// JSON, as text, exactly as the model wrote it (it is not always valid).
    pub arguments: String,
}

impl FunctionCall {
    /// The arguments, or an empty object when the model wrote something unparseable.
    pub fn args(&self) -> Value {
        serde_json::from_str(&self.arguments).unwrap_or(Value::Object(Default::default()))
    }
}

/// What the model answered.
#[derive(Clone, Debug, Default)]
pub struct Reply {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub credits: Option<Credits>,
}

/// A model JBD can think with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatModel {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub default: bool,
}

/// A model JBD can generate video with, and what it costs (prices in USD).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoModel {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub durations: Vec<u32>,
    #[serde(default)]
    pub resolutions: Vec<String>,
    #[serde(default)]
    pub aspects: Vec<String>,
    #[serde(default)]
    pub audio: bool,
    #[serde(default)]
    pub price_per_second: std::collections::BTreeMap<String, f64>,
    #[serde(default)]
    pub audio_price_per_second: std::collections::BTreeMap<String, f64>,
    #[serde(default)]
    pub default: bool,
}

impl VideoModel {
    /// What JBD shows before spending anything. Must match the server's own sum.
    pub fn credits_for(&self, seconds: u32, resolution: &str, audio: bool) -> u32 {
        let table = if audio && !self.audio_price_per_second.is_empty() { &self.audio_price_per_second } else { &self.price_per_second };
        let per_second = table.get(resolution).or_else(|| table.values().next()).copied().unwrap_or(0.0);
        // Same sum as the server's `creditsFor` (lib/platform/credits.ts): round to a
        // millionth of a dollar first, so 6 × $0.03 is 18 credits and not 19 from float error.
        let usd = per_second * f64::from(seconds);
        ((usd * 1e6).round() / 1e4).ceil().max(0.0) as u32
    }

    /// The length allowed by this model that is closest to what was asked for. `durations` is
    /// the full list the server accepts (Veo: 4, 6, 8; Kling: 5, 10), never a range.
    pub fn nearest_duration(&self, seconds: u32) -> u32 {
        self.durations.iter().copied().min_by_key(|d| d.abs_diff(seconds)).unwrap_or_else(|| seconds.clamp(1, 60))
    }

    pub fn best_resolution(&self, want: Option<&str>) -> String {
        if let Some(w) = want
            && self.resolutions.iter().any(|r| r == w)
        {
            return w.to_string();
        }
        // 720p is the sensible default: cheap, and plenty for a reel.
        self.resolutions.iter().find(|r| r.as_str() == "720p").or_else(|| self.resolutions.first()).cloned().unwrap_or_else(|| "720p".to_string())
    }
}

/// A voice JBD can narrate with (`/api/ai/speech`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceModel {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub vendor: String,
    /// USD per character.
    #[serde(default)]
    pub price_per_char: f64,
    #[serde(default)]
    pub voices: Vec<String>,
    #[serde(default)]
    pub default: bool,
}

impl VoiceModel {
    /// Credits to speak `text` (same rounding as the server).
    pub fn credits_for(&self, text: &str) -> u32 {
        let usd = self.price_per_char * text.chars().count() as f64;
        ((usd * 1e6).round() / 1e4).ceil().max(0.0) as u32
    }
}

/// Sound on generated clips: the Director decides per shot, or the person forces it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sound {
    #[default]
    Auto,
    On,
    Off,
}

impl Sound {
    pub const ALL: [Sound; 3] = [Sound::Auto, Sound::On, Sound::Off];

    pub fn label(self) -> &'static str {
        match self {
            Sound::Auto => "Sound: Director decides",
            Sound::On => "Sound: always on",
            Sound::Off => "Sound: off",
        }
    }

    /// Whether a clip gets sound, given what the Director asked for and what the model can do.
    pub fn resolve(self, asked: bool, model_can: bool) -> bool {
        model_can
            && match self {
                Sound::Auto => asked,
                Sound::On => true,
                Sound::Off => false,
            }
    }
}

/// The model lists and the person's credits, as `/api/ai/models` gives them.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Catalogue {
    #[serde(default)]
    pub chat: Vec<ChatModel>,
    #[serde(default)]
    pub video: Vec<VideoModel>,
    #[serde(default)]
    pub voice: Vec<VoiceModel>,
    #[serde(default)]
    pub credits: Option<Credits>,
}

impl Catalogue {
    pub fn default_chat(&self) -> Option<&ChatModel> {
        self.chat.iter().find(|m| m.default).or_else(|| self.chat.first())
    }
    pub fn default_video(&self) -> Option<&VideoModel> {
        self.video.iter().find(|m| m.default).or_else(|| self.video.first())
    }
    pub fn video(&self, id: &str) -> Option<&VideoModel> {
        self.video.iter().find(|m| m.id == id)
    }
    pub fn default_voice(&self) -> Option<&VoiceModel> {
        self.voice.iter().find(|m| m.default).or_else(|| self.voice.first())
    }
}

/// A video generation that has been started on the server.
#[derive(Clone, Debug)]
pub struct VideoJob {
    /// The server's signed job token.
    pub job: String,
    pub credits: u32,
}

/// How far along a generation is.
#[derive(Clone, Debug)]
pub enum VideoStatus {
    Pending,
    /// The finished clip, written to this file.
    Done(std::path::PathBuf),
    Failed(String),
}

/// jabrod.com, as the agent sees it. The desktop implementation is [`gateway::Gateway`];
/// the tests use a fake.
pub trait Model: Send + Sync {
    fn chat(&self, model: &str, messages: &[ChatMessage], tools: &[Value]) -> Result<Reply, String>;

    fn start_video(&self, model: &str, prompt: &str, seconds: u32, aspect: &str, resolution: &str, audio: bool) -> Result<VideoJob, String>;

    /// Ask once whether the clip is ready; downloads it into `into` when it is.
    fn video_status(&self, job: &str, into: &std::path::Path) -> Result<VideoStatus, String>;

    /// The internet tools on jabrod.com (`/api/ai/tools`): `web_search {query}`,
    /// `read_webpage {url}`, `find_images {query, orientation?, count?}`.
    fn web(&self, tool: &str, args: Value) -> Result<Value, String>;

    /// Download a picture, sound or video (https only, size-capped) into `dir`; returns the file.
    fn download_media(&self, url: &str, dir: &std::path::Path) -> Result<std::path::PathBuf, String>;

    /// Narrate `text` (`/api/ai/speech`) into an audio file in `dir`; returns it and the credits.
    fn speech(
        &self,
        model: &str,
        text: &str,
        voice: Option<&str>,
        instructions: Option<&str>,
        dir: &std::path::Path,
    ) -> Result<(std::path::PathBuf, u32), String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn veo() -> VideoModel {
        VideoModel {
            id: "google/veo-3.1-lite-generate-001".into(),
            label: "Veo 3.1 Lite".into(),
            vendor: "Google".into(),
            durations: vec![4, 6, 8],
            resolutions: vec!["720p".into(), "1080p".into()],
            aspects: vec!["16:9".into(), "9:16".into()],
            audio: true,
            price_per_second: [("720p".to_string(), 0.03), ("1080p".to_string(), 0.08)].into_iter().collect(),
            audio_price_per_second: [("720p".to_string(), 0.05), ("1080p".to_string(), 0.1)].into_iter().collect(),
            default: true,
        }
    }

    #[test]
    fn credits_round_up_and_follow_audio() {
        let m = veo();
        assert_eq!(m.credits_for(6, "720p", false), 18); // 6 × $0.03 = $0.18
        assert_eq!(m.credits_for(6, "720p", true), 30); // 6 × $0.05
        assert_eq!(m.credits_for(8, "1080p", false), 64);
        // An unknown resolution falls back to a known price instead of charging nothing.
        assert!(m.credits_for(4, "4k", false) > 0);
    }

    #[test]
    fn durations_snap_to_what_the_model_allows() {
        let m = veo();
        assert_eq!(m.nearest_duration(5), 4);
        assert_eq!(m.nearest_duration(7), 6);
        assert_eq!(m.nearest_duration(30), 8);
        // Two entries are two choices, not a range: Kling makes 5 or 10 s clips.
        let kling = VideoModel { durations: vec![5, 10], ..m.clone() };
        assert_eq!(kling.nearest_duration(7), 5);
        assert_eq!(kling.nearest_duration(9), 10);
        assert_eq!(kling.nearest_duration(30), 10);
        // No list at all: keep what was asked for, within reason.
        let any = VideoModel { durations: vec![], ..m };
        assert_eq!(any.nearest_duration(12), 12);
    }

    #[test]
    fn resolution_defaults_to_720p() {
        let m = veo();
        assert_eq!(m.best_resolution(None), "720p");
        assert_eq!(m.best_resolution(Some("1080p")), "1080p");
        assert_eq!(m.best_resolution(Some("4k")), "720p");
    }

    #[test]
    fn credits_left_never_goes_negative() {
        let c = Credits { used: 300, limit: 250 };
        assert_eq!(c.left(), 0);
        assert_eq!(c.fraction(), 1.0);
        assert_eq!(Credits { used: 0, limit: 0 }.fraction(), 0.0);
    }

    #[test]
    fn bad_tool_arguments_become_an_empty_object() {
        let f = FunctionCall { name: "run_command".into(), arguments: "{not json".into() };
        assert_eq!(f.args(), serde_json::json!({}));
    }

    #[test]
    fn transcript_keeps_its_length_in_check() {
        let mut s = Shared::default();
        for i in 0..500 {
            s.push(Entry::User(format!("{i}")));
        }
        assert_eq!(s.entries.len(), 400);
        // The oldest went, the newest stayed.
        assert!(matches!(s.entries.last(), Some(Entry::User(t)) if t == "499"));
    }
}
