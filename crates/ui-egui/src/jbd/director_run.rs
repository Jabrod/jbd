//! The Director's state in the app, and the bridge between its thread and the editor.
//!
//! The agent (`crates/jbd-director`) runs on its own thread so a slow model never freezes JBD.
//! It can't touch `app.session` from there, so it sends [`ControlRequest`]s down a channel; the
//! UI thread answers them in [`drain`] each frame through `control::handle`, the same path the
//! MCP server uses. Every edit therefore goes through `Session::execute` (undoable, panic-
//! guarded, autosaved).
//!
//! When a request finishes, [`drain`] folds all of its undo steps into one ("Director: …"), so
//! "Undo all" (or Ctrl+Z) takes the whole request back.
//!
//! The view is `director.rs`; this file has no drawing in it.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use jbd_director::{Cancel, Catalogue, ChatMessage, Entry, Mode, Shared, lock};
use serde_json::{Value, json};

use super::director_history as history;
use crate::FilmcraftApp;
use crate::control::{self, ControlRequest};

/// How long a failed model list waits before JBD tries again by itself.
const RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(30);

/// The model lists, loaded once from jabrod.com on a worker thread.
#[derive(Default)]
pub enum Models {
    #[default]
    NotLoaded,
    Loading,
    Ready(Catalogue),
    /// Why it failed, and when: it is tried again after [`RETRY_AFTER`] or when the person
    /// presses Retry, never every frame.
    Failed(String, std::time::Instant),
}

/// Everything the Director tab remembers (lives in `app.jbd.director`).
#[derive(Default)]
pub struct Director {
    /// What the agent thread writes and the tab draws.
    pub shared: Arc<Mutex<Shared>>,
    pub cancel: Cancel,
    /// The prompt being typed.
    pub input: String,
    pub mode: Mode,
    /// Sound on generated clips (the video chip's menu).
    pub sound: jbd_director::Sound,
    /// The picked models (ids from the catalogue; empty = the server's default).
    pub chat_model: String,
    pub video_model: String,
    pub models: Arc<Mutex<Models>>,
    /// The conversation, kept between requests so "now make it shorter" works.
    memory: Arc<Mutex<Vec<ChatMessage>>>,
    worker: Option<std::thread::JoinHandle<()>>,
    /// Editor requests from the agent thread.
    rx: Option<Receiver<ControlRequest>>,
    tx: Option<Sender<ControlRequest>>,
    /// The project before the running request, and its undo label.
    run_start: Option<(Arc<filmcraft_project::Project>, String)>,
    /// The undo steps the running request made itself (each step's "project before" snapshot),
    /// and the mask-tracking jobs it started (their steps are written a frame later). Only these
    /// fold into its one undo step: an edit you make by hand meanwhile is never swept into it.
    owned: Vec<Arc<filmcraft_project::Project>>,
    owned_jobs: Vec<String>,
    /// The finished request "Undo all" can take back: its undo label and the project before it.
    pub undo_all: Option<(Arc<filmcraft_project::Project>, String)>,
    /// The person's MCP connectors, started on first use (`jbd/connections.rs`).
    #[cfg(not(target_arch = "wasm32"))]
    connectors: Option<Arc<jbd_director::mcp::Connectors>>,
    /// When the sidebar last read the chat list (UI time; 0 = read it now).
    pub chats_read_at: f64,
    /// The History menu's list (read when it opens).
    pub chats: Vec<super::director_history::Summary>,
    /// The conversation on screen (its file name in the history folder).
    pub chat_id: String,
    /// Step groups opened in the transcript (by their first step id).
    pub open_groups: std::collections::HashSet<u64>,
    /// The plan strip is expanded to show every step.
    pub plan_open: bool,
    /// Timeline clips attached to the next message as context.
    pub context: Vec<ClipRef>,
    /// Files attached to the next message (already imported into the project).
    pub attachments: Vec<Attachment>,
    /// Long texts pasted into the prompt box, kept out of it as chips ("Pasted text · 23k
    /// chars") so the box stays small; they are sent with the message.
    pub pasted: Vec<String>,
    /// Clips paid for but not fetched yet (JBD closed, or Stop, while they were being made):
    /// how many, and when the media folder was last checked (UI time).
    pub pending: (usize, f64),
    /// A "Fetch them" run: true while it works; its files and problems land in `fetched`.
    pub fetching: Arc<std::sync::atomic::AtomicBool>,
    pub fetched: Arc<Mutex<Option<(Vec<std::path::PathBuf>, Vec<String>)>>>,
    /// When the chat on screen last really changed, and how many entries it had then: opening
    /// or leaving a chat must not move it to the top as "just now".
    chat_saved: (u64, usize),
    /// The chats sidebar is hidden in Director view (the header's sidebar button).
    pub sidebar_hidden: bool,
    /// Long messages of yours expanded in the transcript (by entry index).
    pub open_messages: std::collections::HashSet<usize>,
}

/// A paste longer than this becomes a chip instead of filling the prompt box.
pub const PASTE_CHIP_CHARS: usize = 2_000;
/// Most text one message may carry (typed and pasted together); the server takes 200k.
pub const MAX_MESSAGE_CHARS: usize = 150_000;

/// A file attached in the prompt box.
#[derive(Clone, Debug)]
pub struct Attachment {
    pub name: String,
    /// The project item it became.
    pub item: Option<u64>,
}

/// The "+" button: pick files and import them now, so the Director can refer to them by id.
/// The person chose these files themselves, which is why this may read from anywhere on disk
/// (the Director alone may only import what it generated; see jbd-director/src/tools.rs).
pub fn attach(app: &mut FilmcraftApp) {
    let exts: Vec<&str> =
        filmcraft_media::VIDEO_EXTENSIONS.iter().chain(filmcraft_media::AUDIO_EXTENSIONS).chain(filmcraft_media::STILL_EXTENSIONS).copied().collect();
    let paths = app.hooks.pick_files.as_mut().map(|f| f(&exts)).unwrap_or_default();
    if paths.is_empty() {
        return;
    }
    match app.session.execute("file.import", json!({"paths": paths})) {
        Ok(v) => {
            let items: Vec<u64> = v.get("items").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64).collect();
            for (i, p) in paths.iter().enumerate() {
                let name = std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.clone());
                app.jbd.director.attachments.push(Attachment { name, item: items.get(i).copied() });
            }
        }
        Err(e) => lock(&app.jbd.director.shared).push(Entry::Error(format!("Couldn’t attach those files: {e}"))),
    }
}

impl Director {
    pub fn busy(&self) -> bool {
        self.worker.as_ref().is_some_and(|w| !w.is_finished())
    }

    /// Forget the conversation (the "New chat" button).
    pub fn clear(&mut self) {
        if self.busy() {
            return;
        }
        self.save_chat();
        lock(&self.memory).clear();
        let mut s = lock(&self.shared);
        s.entries.clear();
        s.todos.clear();
        s.ask = None;
        drop(s);
        self.chat_id = history::new_id();
        self.chat_saved = (0, 0);
        self.undo_all = None;
        self.open_groups.clear();
        self.open_messages.clear();
    }

    /// Save this conversation (nothing to save before the first message). A failed save is
    /// ignored: it must never interrupt editing.
    pub fn save_chat(&mut self) {
        let s = lock(&self.shared);
        if s.entries.is_empty() {
            return;
        }
        // a chat's time is when something was last said in it, not when it was last opened
        let (when, len) = self.chat_saved;
        let updated = if when == 0 || s.entries.len() != len { history::now() } else { when };
        let c = history::Conversation {
            id: self.chat_id.clone(),
            title: history::title_of(&s.entries),
            updated,
            entries: s.entries.clone(),
            todos: s.todos.clone(),
            memory: lock(&self.memory).clone(),
        };
        drop(s);
        if updated == when {
            // nothing new: the file already holds it
            return;
        }
        self.chat_saved = (updated, c.entries.len());
        let _ = history::save(&c);
    }

    /// Open a saved conversation (after saving the one on screen).
    pub fn open_chat(&mut self, id: &str) {
        if self.busy() {
            return;
        }
        let Some(c) = history::load(id) else { return };
        self.save_chat();
        *lock(&self.memory) = c.memory;
        self.chat_saved = (c.updated, c.entries.len());
        let mut s = lock(&self.shared);
        s.entries = c.entries;
        s.todos = c.todos;
        s.ask = None;
        drop(s);
        self.chat_id = c.id;
        // an older conversation's edits are not the ones on the timeline now
        self.undo_all = None;
        self.open_groups.clear();
        self.open_messages.clear();
    }

    /// How full the conversation is, 0–1, against a 200k-token context window (about 4
    /// characters a token). Older turns are dropped automatically as it fills; "New chat" empties it.
    pub fn context_fraction(&self) -> f32 {
        const WINDOW_TOKENS: f32 = 200_000.0;
        let chars: usize = lock(&self.memory)
            .iter()
            .map(|m| m.content.as_deref().map_or(0, str::len) + m.tool_calls.as_ref().map_or(0, |c| c.iter().map(|x| x.function.arguments.len()).sum()))
            .sum();
        (chars as f32 / 4.0 / WINDOW_TOKENS).clamp(0.0, 1.0)
    }

    /// Start the person's connectors again on the next request (after Settings ▸ Connectors
    /// changed).
    pub fn restart_connectors(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.connectors = None;
        }
    }

    /// The connectors, started on first use. None when none are configured or enabled.
    #[cfg(not(target_arch = "wasm32"))]
    fn external(&mut self) -> Option<Arc<dyn jbd_director::ExternalTools>> {
        if self.connectors.is_none() {
            let path = super::connections::connectors_path()?;
            let configs = jbd_director::mcp::load_config(&path);
            if configs.iter().any(|c| c.enabled) {
                // starting a server runs someone else's program: only ever the person's own
                // connectors from Settings, never anything a model asked for
                self.connectors = Some(Arc::new(jbd_director::mcp::Connectors::start(&configs)));
            }
        }
        self.connectors.clone().map(|c| c as Arc<dyn jbd_director::ExternalTools>)
    }

    /// Answer the permission ask on screen.
    pub fn answer(&self, yes: bool) {
        if let Some(a) = lock(&self.shared).ask.as_mut() {
            a.answer = Some(yes);
        }
    }

    pub fn stop(&self) {
        self.cancel.stop();
        self.answer(false);
    }
}

/// How many clips are still waiting to be fetched for this project (read from disk every few
/// seconds, not every frame).
pub fn pending_clips(app: &mut FilmcraftApp, now: f64) -> usize {
    if now - app.jbd.director.pending.1 > 5.0 || app.jbd.director.pending.1 == 0.0 {
        app.jbd.director.pending = (jbd_director::production::pending_jobs(&media_dir(app)).len(), now);
    }
    app.jbd.director.pending.0
}

/// "Fetch them": download the clips that were still being made, on a worker thread (it waits
/// for the ones not ready yet, up to ten minutes). [`drain`] imports what arrives.
pub fn fetch_pending(app: &mut FilmcraftApp, ctx: &egui::Context) {
    use std::sync::atomic::Ordering;
    #[cfg(not(target_arch = "wasm32"))]
    {
        if app.jbd.director.fetching.swap(true, Ordering::Relaxed) {
            return;
        }
        let Some(backend) = app.jbd.backend.clone().filter(|_| app.jbd.status.is_signed_in()) else {
            app.jbd.director.fetching.store(false, Ordering::Relaxed);
            lock(&app.jbd.director.shared).push(Entry::Error("Sign in to your Jabrod account to fetch the clips.".into()));
            return;
        };
        let dir = media_dir(app);
        let (busy, out, ctx) = (app.jbd.director.fetching.clone(), app.jbd.director.fetched.clone(), ctx.clone());
        let spawned = std::thread::Builder::new().name("jbd-director-fetch".into()).spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let token: jbd_director::gateway::TokenSource = Arc::new(move || backend.access_token());
                let gw = jbd_director::gateway::Gateway::new(jbd_account::platform_url(), token);
                let (mut files, mut errors) = (Vec::new(), Vec::new());
                let began = std::time::Instant::now();
                loop {
                    let (f, still, e) = jbd_director::production::fetch_pending(&gw, &dir);
                    files.extend(f);
                    errors.extend(e);
                    if still == 0 || began.elapsed() > std::time::Duration::from_secs(600) {
                        if still > 0 {
                            errors.push(format!("{still} clip(s) still aren't ready; try Fetch again later"));
                        }
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_secs(5));
                }
                (files, errors)
            }));
            *lock(&out) = Some(r.unwrap_or_else(|_| (Vec::new(), vec!["fetching the clips failed".into()])));
            busy.store(false, Ordering::Relaxed);
            ctx.request_repaint();
        });
        if spawned.is_err() {
            app.jbd.director.fetching.store(false, Ordering::Relaxed);
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (app, ctx);
}

/// Where generated clips go: next to the saved project, else Videos\JBD Generated.
fn media_dir(app: &FilmcraftApp) -> std::path::PathBuf {
    if let Some(dir) = app.session.path.as_deref().and_then(|p| std::path::Path::new(p).parent()) {
        return dir.join("JBD Generated");
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    home.join("Videos").join("JBD Generated")
}

/// The agent's view of the editor: requests go to the UI thread and wait for its answer.
struct ChannelEditor {
    tx: Sender<ControlRequest>,
    ctx: egui::Context,
    dir: std::path::PathBuf,
}

impl jbd_director::Editor for ChannelEditor {
    fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let (req, reply) = ControlRequest::new(method, params);
        self.tx.send(req).map_err(|_| "JBD is closing".to_string())?;
        self.ctx.request_repaint();
        let v = reply.recv_timeout(std::time::Duration::from_secs(120)).map_err(|_| "the editor didn't answer in time".to_string())?;
        if v.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(v.get("result").cloned().unwrap_or(Value::Null))
        } else {
            Err(v.get("error").and_then(Value::as_str).unwrap_or("the command failed").to_string())
        }
    }

    fn media_dir(&self) -> std::path::PathBuf {
        self.dir.clone()
    }
}

/// Load the model lists in the background (once, or again after a failure).
pub fn load_models(app: &mut FilmcraftApp, ctx: &egui::Context) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let d = &app.jbd.director;
        let wait = match &*lock(&d.models) {
            Models::Loading | Models::Ready(_) => true,
            Models::Failed(_, at) => at.elapsed() < RETRY_AFTER,
            Models::NotLoaded => false,
        };
        if wait {
            return;
        }
        let Some(backend) = app.jbd.backend.clone() else { return };
        if !app.jbd.status.is_signed_in() {
            return;
        }
        *lock(&d.models) = Models::Loading;
        let models = d.models.clone();
        let shared = d.shared.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let token: jbd_director::gateway::TokenSource = Arc::new(move || backend.access_token());
            let gw = jbd_director::gateway::Gateway::new(jbd_account::platform_url(), token);
            let got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gw.catalogue())).unwrap_or_else(|_| Err("loading the models failed".into()));
            *lock(&models) = match got {
                Ok(c) => {
                    lock(&shared).credits = c.credits;
                    Models::Ready(c)
                }
                Err(e) => {
                    // come back to try again once the pause is over
                    ctx.request_repaint_after(RETRY_AFTER);
                    Models::Failed(e, std::time::Instant::now())
                }
            };
            ctx.request_repaint();
        });
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (app, ctx);
}

/// Start working on what was typed.
pub fn send(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let text = message_text(&app.jbd.director.input, &app.jbd.director.pasted);
    if text.is_empty() || app.jbd.director.busy() {
        return;
    }
    if text.chars().count() > MAX_MESSAGE_CHARS {
        lock(&app.jbd.director.shared).push(Entry::Error(format!(
            "That message is too long ({}k characters; at most {}k). Remove a pasted text, or send the brief in parts.",
            text.chars().count() / 1000,
            MAX_MESSAGE_CHARS / 1000
        )));
        return;
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = ctx;
        lock(&app.jbd.director.shared).push(Entry::Error("The Director works in the desktop app.".into()));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(backend) = app.jbd.backend.clone().filter(|_| app.jbd.status.is_signed_in()) else {
            lock(&app.jbd.director.shared).push(Entry::Error("Sign in to your Jabrod account to use the Director.".into()));
            return;
        };
        if !app.jbd.status.is_pro() {
            lock(&app.jbd.director.shared).push(Entry::Error("The AI Director is part of JBD Pro. Upgrade at jabrod.com/jbd/upgrade.".into()));
            return;
        }
        app.jbd.director.input.clear();
        app.jbd.director.pasted.clear();
        let context: Vec<ClipRef> = std::mem::take(&mut app.jbd.director.context);
        let context_clips: Vec<String> = context.iter().map(ClipRef::line).collect();
        let files = std::mem::take(&mut app.jbd.director.attachments);
        // pictures go to the model as pictures (it reads PNG, JPEG and WebP), so it can see the
        // logo it is asked to place; anything else it knows by name and item id
        let images: Vec<String> = files.iter().filter_map(|a| a.item.and_then(|i| media_path(app, i))).filter_map(|p| image_data_url(&p)).collect();
        // shown under the message in the chat as thumbnails
        let mut shown_items: Vec<(u64, String)> = files.iter().filter_map(|a| a.item.map(|i| (i, a.name.clone()))).collect();
        shown_items.extend(context.iter().map(|c| (c.item, format!("{}\n{}", c.label(), c.name))));
        let attachments: Vec<String> = files
            .into_iter()
            .map(|a| match a.item {
                Some(id) => format!("\"{}\" (item {id})", a.name),
                None => format!("\"{}\"", a.name),
            })
            .collect();
        let label = format!("Director: {}", jbd_director::tools::clip(&text, 40));
        app.jbd.director.run_start = Some((app.session.project.clone(), label));
        app.jbd.director.owned.clear();
        app.jbd.director.owned_jobs.clear();
        app.jbd.director.undo_all = None;
        if app.jbd.director.chat_id.is_empty() {
            app.jbd.director.chat_id = history::new_id();
        }
        if app.jbd.director.tx.is_none() {
            let (tx, rx) = std::sync::mpsc::channel();
            app.jbd.director.tx = Some(tx);
            app.jbd.director.rx = Some(rx);
        }
        let d = &app.jbd.director;
        let Some(tx) = d.tx.clone() else { return };
        {
            let mut s = lock(&d.shared);
            // the message as typed; what was attached shows as pictures under it
            s.push(Entry::User(text.clone()));
            if !shown_items.is_empty() {
                s.push(Entry::Attached(shown_items));
            }
            s.todos.clear();
            s.busy = true;
        }
        d.cancel.reset();
        let editor = ChannelEditor { tx, ctx: ctx.clone(), dir: media_dir(app) };
        let (shared, cancel, memory, models) = (d.shared.clone(), d.cancel.clone(), d.memory.clone(), d.models.clone());
        let external = app.jbd.director.external();
        let d = &app.jbd.director;
        let (mode, sound, chat_pick, video_pick) = (d.mode, d.sound, d.chat_model.clone(), d.video_model.clone());
        let ctx = ctx.clone();
        let worker = std::thread::Builder::new().name("jbd-director".into()).spawn(move || {
            let token: jbd_director::gateway::TokenSource = Arc::new(move || backend.access_token());
            let gw = jbd_director::gateway::Gateway::new(jbd_account::platform_url(), token);
            let repaint = {
                let ctx = ctx.clone();
                move || ctx.request_repaint()
            };
            // Anything that goes wrong in here, a panic included, ends this request only.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // The model lists, if they aren't loaded yet.
                let catalogue = {
                    let have = match &*lock(&models) {
                        Models::Ready(c) => Some(c.clone()),
                        _ => None,
                    };
                    match have {
                        Some(c) => c,
                        None => match gw.catalogue() {
                            Ok(c) => {
                                *lock(&models) = Models::Ready(c.clone());
                                c
                            }
                            // An older server without the list still has the "best" role.
                            Err(_) => Catalogue::default(),
                        },
                    }
                };
                let chat_model = if chat_pick.is_empty() { catalogue.default_chat().map(|m| m.id.clone()).unwrap_or_else(|| "best".into()) } else { chat_pick };
                let video_model = if video_pick.is_empty() { catalogue.default_video().map(|m| m.id.clone()).unwrap_or_default() } else { video_pick };
                let req = jbd_director::agent::Request {
                    text,
                    chat_model,
                    video_model,
                    mode,
                    sound,
                    catalogue: &catalogue,
                    attachments,
                    context_clips,
                    images,
                    external,
                };
                let mut mem = lock(&memory).clone();
                let run = jbd_director::agent::run(&req, &mut mem, &editor, &gw, &shared, &cancel, &repaint);
                *lock(&memory) = mem;
                run
            }));
            let mut s = lock(&shared);
            match result {
                Ok(run) => s.last_run = Some(run),
                Err(_) => s.push(Entry::Error("The Director hit a problem and stopped. Your project is fine; everything it did can be undone.".into())),
            }
            s.busy = false;
            s.ask = None;
            s.activity = None;
            drop(s);
            ctx.request_repaint();
        });
        match worker {
            Ok(w) => app.jbd.director.worker = Some(w),
            Err(e) => {
                let mut s = lock(&app.jbd.director.shared);
                s.busy = false;
                s.push(Entry::Error(format!("Couldn't start the Director ({e}).")));
            }
        }
    }
}

/// The message as sent: what was typed, then each pasted text.
pub fn message_text(input: &str, pasted: &[String]) -> String {
    let mut parts: Vec<&str> = vec![input.trim()];
    parts.extend(pasted.iter().map(|p| p.trim()));
    parts.retain(|p| !p.is_empty());
    parts.join("\n\n")
}

/// Each frame, on the UI thread: answer the agent's editor requests, and tidy up when a request
/// has finished.
pub fn drain(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if let Some(rx) = app.jbd.director.rx.take() {
        while let Ok(req) = rx.try_recv() {
            let reply = req.reply.clone();
            // Only editor commands: the Director doesn't click around the UI or take screenshots.
            let answer = if req.method == "jbd.frames" {
                // read-only: the edit rendered at these times, as one contact sheet (look tool).
                // Rendered and encoded on a thread of its own, which answers the request itself:
                // the UI never waits for it.
                render_frames(app, req);
                continue;
            } else if req.method == "jbd.mediaPath" {
                // read-only: where a media item's file is (regenerate_clip reads its recipe)
                let item = req.params.get("item").and_then(Value::as_u64).unwrap_or(0);
                match media_path(app, item) {
                    Some(p) => json!({"ok": true, "result": {"path": p}}),
                    None => json!({"ok": false, "error": "that item isn't a file on disk"}),
                }
            } else if req.method == "engine.execute" {
                let top = app.session.history.undo.last().map(|(_, p)| p.clone());
                let answer = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| control::handle(app, ctx, &req))) {
                    Ok(control::Outcome::Done(v)) => v,
                    Ok(_) => json!({"ok": false, "error": "that command needs the window and can't be run by the Director"}),
                    Err(_) => json!({"ok": false, "error": "the command failed inside JBD; try another way"}),
                };
                own_new_steps(app, top.as_ref());
                // a tracking job writes its result later, as a step of its own: remember it
                if req.params.get("command").and_then(Value::as_str) == Some("masks.track")
                    && let Some(job) = answer.get("result").and_then(|r| r.get("job")).and_then(Value::as_u64)
                {
                    app.jbd.director.owned_jobs.push(format!("mask-track-{job}"));
                }
                answer
            } else {
                json!({"ok": false, "error": format!("`{}` isn't available to the Director", req.method)})
            };
            let _ = reply.send(answer);
        }
        app.jbd.director.rx = Some(rx);
    }

    // the Director's own tracking result, written by the editor this frame
    if app.jbd.director.run_start.is_some()
        && let Some(key) = app.session.history.merge_key.clone()
        && app.jbd.director.owned_jobs.contains(&key)
        && let Some((_, before)) = app.session.history.undo.last()
        && !app.jbd.director.owned.iter().any(|o| Arc::ptr_eq(o, before))
    {
        let before = before.clone();
        app.jbd.director.owned.push(before);
    }

    // Clips fetched after the fact: into the project (the bin), not onto the timeline.
    let fetched = lock(&app.jbd.director.fetched).take();
    if let Some((files, errors)) = fetched {
        let paths: Vec<String> = files.iter().map(|p| p.to_string_lossy().to_string()).collect();
        let mut s = Vec::new();
        if !paths.is_empty() {
            match app.session.execute("file.import", json!({"paths": paths})) {
                Ok(_) => s.push(format!(
                    "Fetched {} clip{} into the project. They're in the media list, ready to place.",
                    paths.len(),
                    if paths.len() == 1 { "" } else { "s" }
                )),
                Err(e) => s.push(format!("Downloaded {} clip(s) but couldn’t import them: {e}", paths.len())),
            }
        }
        s.extend(errors);
        app.jbd.director.pending.1 = 0.0;
        if !s.is_empty() {
            lock(&app.jbd.director.shared).push(Entry::Said(s.join("\n\n")));
        }
    }

    // A finished request: fold its edits into one undo step.
    if app.jbd.director.worker.as_ref().is_some_and(|w| w.is_finished()) {
        if let Some(w) = app.jbd.director.worker.take() {
            let _ = w.join();
        }
        if let Some((start, label)) = app.jbd.director.run_start.take() {
            let owned = std::mem::take(&mut app.jbd.director.owned);
            match fold_history(&mut app.session.history, &start, &label, &owned) {
                Fold::Folded => app.jbd.director.undo_all = Some((start, label)),
                Fold::Nothing => {}
                Fold::Mixed => lock(&app.jbd.director.shared).push(Entry::Said(
                    "You edited the timeline while I was working, so my changes stay as separate undo steps (each named \"Director: …\") instead of one: undoing them all at once would also undo yours. Ctrl+Z takes them back one at a time.".into(),
                )),
            }
        }
        // keep the conversation in the history (jbd/director_history.rs)
        if app.jbd.director.chat_id.is_empty() {
            app.jbd.director.chat_id = history::new_id();
        }
        app.jbd.director.save_chat();
    }
}

/// A timeline clip attached to the next message as context ("these are the clips I mean"), like
/// a file attached in Claude Code.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipRef {
    pub clip: u64,
    pub item: u64,
    pub name: String,
    /// "V1", "A2".
    pub track: String,
    pub start: f64,
    pub end: f64,
    /// The clip's own sound (its linked audio clip), selected along with it.
    pub sound: Option<u64>,
}

impl ClipRef {
    /// How the Director reads it.
    pub fn line(&self) -> String {
        let sound = self.sound.map(|s| format!(", with its own sound as linked audio clip {s}")).unwrap_or_default();
        format!("clip {} \"{}\" on {}, {:.1}–{:.1} s (media item {}){sound}", self.clip, self.name, self.track, self.start, self.end, self.item)
    }
    /// The chip's label.
    pub fn label(&self) -> String {
        format!("{} · {:.1}–{:.1} s{}", self.track, self.start, self.end, if self.sound.is_some() { " + sound" } else { "" })
    }
}

/// The clips selected on the open timeline, in timeline order. A video clip and its own sound
/// (selected together while Linked Selection is on) are one clip here, as the person sees them.
pub fn selected_clips(app: &FilmcraftApp) -> Vec<ClipRef> {
    clip_refs(app, &app.session.state.selection)
}

/// `clips` as the Director reads them (a video clip and its own sound folded into one).
fn clip_refs(app: &FilmcraftApp, sel: &[filmcraft_project::ClipId]) -> Vec<ClipRef> {
    const TICKS: f64 = 254_016_000_000.0;
    let Some(seq) = app.session.active_sequence() else { return Vec::new() };
    let mut out: Vec<ClipRef> = Vec::new();
    // links of the selected video clips, to fold their audio partners into them
    let mut links: Vec<(u64, usize)> = Vec::new();
    for (i, t) in seq.video_tracks.iter().enumerate() {
        for it in t.items.iter().filter(|it| sel.contains(&it.id)) {
            if let Some(l) = it.link {
                links.push((l, out.len()));
            }
            out.push(ClipRef {
                clip: it.id.0,
                item: it.item.0,
                name: it.name.clone(),
                track: format!("V{}", i + 1),
                start: it.start.0 as f64 / TICKS,
                end: it.end().0 as f64 / TICKS,
                sound: None,
            });
        }
    }
    for (i, t) in seq.audio_tracks.iter().enumerate() {
        for it in t.items.iter().filter(|it| sel.contains(&it.id)) {
            if let Some(&(_, at)) = it.link.and_then(|l| links.iter().find(|(x, _)| *x == l))
                && let Some(v) = out.get_mut(at)
                && v.sound.is_none()
            {
                v.sound = Some(it.id.0);
                continue;
            }
            out.push(ClipRef {
                clip: it.id.0,
                item: it.item.0,
                name: it.name.clone(),
                track: format!("A{}", i + 1),
                start: it.start.0 as f64 / TICKS,
                end: it.end().0 as f64 / TICKS,
                sound: None,
            });
        }
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.track.cmp(&b.track)));
    out
}

/// The timeline's right-click "Add to Director": the selected clips (a clip and its own sound as
/// one) become context for the next message; the Director opens so the chips are seen.
pub fn add_selection_to_director(app: &mut FilmcraftApp) {
    let refs = selected_clips(app);
    let mut added = 0;
    for r in refs {
        if !app.jbd.director.context.iter().any(|c| c.clip == r.clip) {
            app.jbd.director.context.push(r);
            added += 1;
        }
    }
    app.show_panel(crate::dock::PanelKind::Director);
    app.ui.status = if added == 0 {
        "Already given to the Director".into()
    } else {
        format!("Gave {added} clip{} to the Director as context", if added == 1 { "" } else { "s" })
    };
}

/// Ctrl+Alt+click on a timeline clip: give it to the Director as context (with its own sound),
/// or take it back if it is already attached. Opens the Director so the chip is seen.
pub fn toggle_context_clip(app: &mut FilmcraftApp, clip: filmcraft_project::ClipId) {
    let ids = filmcraft_engine::commands::with_links(&app.session, &[clip]);
    let ctx = &mut app.jbd.director.context;
    let had = ctx.len();
    ctx.retain(|c| c.clip != clip.0 && c.sound != Some(clip.0));
    if ctx.len() != had {
        app.ui.status = "Taken back from the Director".into();
        return;
    }
    let refs = clip_refs(app, &ids);
    let n = refs.len();
    for r in refs {
        if !app.jbd.director.context.iter().any(|c| c.clip == r.clip) {
            app.jbd.director.context.push(r);
        }
    }
    if n > 0 {
        app.show_panel(crate::dock::PanelKind::Director);
        app.ui.status = "Given to the Director as context (Ctrl+Alt+click again to take it back)".into();
    }
}

/// Render the open timeline at `seconds` (each frame at most `maxSide` px) into one contact
/// sheet (PNG as base64, a 10 % grid on each frame with `grid`), and answer `req` with it.
///
/// Only a snapshot is taken here (the project is an `Arc`, the media pool is shared): the
/// rendering and the PNG encoding run on their own thread, so a look at nine frames of a heavy
/// edit never stalls the editor. The playhead isn't touched.
pub fn render_frames(app: &FilmcraftApp, req: ControlRequest) {
    let fail = |req: &ControlRequest, e: &str| {
        let _ = req.reply.send(json!({"ok": false, "error": e}));
    };
    let seconds: Vec<f64> = req
        .params
        .get("seconds")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_f64).take(jbd_director::look::MAX_FRAMES).collect())
        .unwrap_or_default();
    if seconds.is_empty() {
        return fail(&req, "no times given");
    }
    let max_side = req.params.get("maxSide").and_then(Value::as_u64).unwrap_or(384).clamp(64, 1024) as f32;
    let grid = req.params.get("grid").and_then(Value::as_bool).unwrap_or(false);
    let s = &app.session;
    let Some(seq) = s.state.active_sequence else { return fail(&req, "no timeline is open") };
    let Some((w, h)) = s.active_sequence().map(|q| (q.settings.width, q.settings.height)) else { return fail(&req, "no timeline is open") };
    let scale = (max_side / w.max(h).max(1) as f32).min(1.0);
    let project = s.project.clone();
    let provider = s.media.provider(project.clone(), s.services.clone());
    let worker = std::thread::Builder::new().name("jbd-director-look".into()).spawn(move || {
        let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let opts = filmcraft_render::RenderOptions { scale, captions: true, ..Default::default() };
            let frames: Vec<(u32, u32, Vec<u8>)> = seconds
                .iter()
                .map(|t| filmcraft_render::render_sequence(&project, seq, filmcraft_engine::time::Tick::from_seconds_f64(*t), opts, &provider))
                .map(|img| (img.w as u32, img.h as u32, img.over_black_rgba8()))
                .collect();
            if frames.is_empty() {
                return Err("nothing rendered at those times".to_string());
            }
            jbd_director::look::contact_sheet_with(&frames, grid)
        }));
        let answer = match made {
            Ok(Ok(png)) => {
                use base64::Engine as _;
                json!({"ok": true, "result": {"pngBase64": base64::engine::general_purpose::STANDARD.encode(png)}})
            }
            Ok(Err(e)) => json!({"ok": false, "error": e}),
            Err(_) => json!({"ok": false, "error": "rendering the frames failed"}),
        };
        let _ = req.reply.send(answer);
    });
    if let Err(e) = worker {
        // the request moved into the closure that never ran: the agent's wait times out
        log::warn!("couldn't start the look thread: {e}");
    }
}

/// A picture file as a `data:` URL for the model: PNG, JPEG or WebP up to 2.5 MB (bigger files
/// would make every request slow; the model knows those by name).
fn image_data_url(path: &str) -> Option<String> {
    use base64::Engine as _;
    let ext = std::path::Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => return None,
    };
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > 2_500_000 {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

/// The file behind a media item, if it is one.
fn media_path(app: &FilmcraftApp, item: u64) -> Option<String> {
    match &app.session.project.item(filmcraft_project::ItemId(item))?.kind {
        filmcraft_project::ItemKind::Media(m) => match &m.media {
            filmcraft_project::MediaRef::File { path } => Some(path.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// When exactly one clip is selected and the Director generated it: the clip and how it was made
/// (for the Regenerate strip above the prompt box).
pub fn selected_generation(app: &FilmcraftApp) -> Option<(u64, jbd_director::production::Generation)> {
    let [clip] = app.session.state.selection.as_slice() else { return None };
    let seq = app.session.active_sequence()?;
    let (_, it) = seq.find_item(*clip)?;
    let path = media_path(app, it.item.0)?;
    jbd_director::production::read_recipe(std::path::Path::new(&path)).map(|g| (clip.0, g))
}

/// Mark the undo steps added since `top` (the newest step before a Director command) as the
/// Director's own, and stop the next edit from merging into its step.
fn own_new_steps(app: &mut FilmcraftApp, top: Option<&Arc<filmcraft_project::Project>>) {
    let undo = &app.session.history.undo;
    let from = match top {
        // (if the history was full and that step fell off its end, only the newest step can be ours)
        Some(t) => undo.iter().rposition(|(_, p)| Arc::ptr_eq(p, t)).map_or(undo.len().saturating_sub(1), |i| i + 1),
        None => 0,
    };
    let new: Vec<Arc<filmcraft_project::Project>> = undo.iter().skip(from).map(|(_, p)| p.clone()).collect();
    if !new.is_empty() {
        app.jbd.director.owned.extend(new);
        app.session.history.merge_key = None;
    }
}

/// What folding a request's undo steps did.
#[derive(Debug, PartialEq, Eq)]
enum Fold {
    /// Its steps are now one ("Director: …"): Undo all takes the request back.
    Folded,
    /// It changed nothing (or its steps are gone from the history).
    Nothing,
    /// Edits by hand are mixed in between: nothing folded; its own steps are named "Director: …".
    Mixed,
}

/// Fold the undo steps the request made (`owned`, from `start`) into one step called `label` —
/// only when every step since `start` is its own. An edit by hand in between can't be part of
/// it (undoing the group would undo that too), so then the steps stay apart.
fn fold_history(
    history: &mut filmcraft_engine::History,
    start: &Arc<filmcraft_project::Project>,
    label: &str,
    owned: &[Arc<filmcraft_project::Project>],
) -> Fold {
    let mine = |p: &Arc<filmcraft_project::Project>| owned.iter().any(|o| Arc::ptr_eq(o, p));
    let Some(at) = history.undo.iter().rposition(|(_, before)| Arc::ptr_eq(before, start)) else { return Fold::Nothing };
    if history.undo.iter().skip(at).all(|(_, p)| mine(p)) {
        history.undo.truncate(at + 1);
        if let Some(step) = history.undo.get_mut(at) {
            step.0 = label.to_string();
        }
        history.merge_key = None;
        return Fold::Folded;
    }
    for step in history.undo.iter_mut().skip(at) {
        if mine(&step.1) && !step.0.starts_with("Director") {
            step.0 = format!("Director: {}", step.0);
        }
    }
    history.merge_key = None;
    Fold::Mixed
}

/// "Undo all" can take the last request back only while it is still the latest undo step.
pub fn can_undo_all(app: &FilmcraftApp) -> bool {
    match (&app.jbd.director.undo_all, app.session.history.undo.last()) {
        (Some((start, label)), Some((top_label, before))) => Arc::ptr_eq(start, before) && label == top_label,
        _ => false,
    }
}

pub fn undo_all(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if can_undo_all(app) {
        let _ = crate::menus::invoke(app, ctx, "edit.undo", json!({}));
        app.jbd.director.undo_all = None;
        lock(&app.jbd.director.shared).push(Entry::Said("Undid that request.".into()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_requests_steps_fold_into_one() {
        let p0 = Arc::new(filmcraft_project::Project::default());
        let p1 = Arc::new(filmcraft_project::Project::default());
        let p2 = Arc::new(filmcraft_project::Project::default());
        let p3 = Arc::new(filmcraft_project::Project::default());
        let mut h = filmcraft_engine::History { limit: 100, ..Default::default() };
        // an earlier edit by hand, then three by the Director (each stores the project before it)
        h.undo = vec![("Razor".into(), p0), ("Ripple Delete".into(), p1.clone()), ("Apply Effect".into(), p2.clone()), ("Place".into(), p3.clone())];
        assert_eq!(fold_history(&mut h, &p1, "Director: make a reel", &[p1.clone(), p2, p3]), Fold::Folded);
        assert_eq!(h.undo.len(), 2);
        assert_eq!(h.undo[1].0, "Director: make a reel");
        assert!(Arc::ptr_eq(&h.undo[1].1, &p1), "undoing returns to the project before the request");
        assert_eq!(h.undo[0].0, "Razor", "the hand edit before it is untouched");
    }

    #[test]
    fn nothing_to_fold_when_the_request_changed_nothing() {
        let mut h = filmcraft_engine::History { limit: 100, ..Default::default() };
        let start = Arc::new(filmcraft_project::Project::default());
        assert_eq!(fold_history(&mut h, &start, "Director: hi", &[]), Fold::Nothing);
    }

    #[test]
    fn an_edit_by_hand_while_the_director_works_is_never_folded_into_its_undo() {
        let p0 = Arc::new(filmcraft_project::Project::default());
        let d1 = Arc::new(filmcraft_project::Project::default());
        let you = Arc::new(filmcraft_project::Project::default());
        let d2 = Arc::new(filmcraft_project::Project::default());
        let mut h = filmcraft_engine::History { limit: 100, ..Default::default() };
        // before the request; the Director's edit; yours (a razor) while it worked; the Director's again
        h.undo = vec![("Razor".into(), p0), ("Place".into(), d1.clone()), ("Razor".into(), you), ("Apply Effect".into(), d2.clone())];
        assert_eq!(fold_history(&mut h, &d1, "Director: make a reel", &[d1.clone(), d2]), Fold::Mixed);
        assert_eq!(h.undo.len(), 4, "nothing folded away: your razor is still its own step");
        let labels: Vec<&str> = h.undo.iter().map(|s| s.0.as_str()).collect();
        assert_eq!(labels, vec!["Razor", "Director: Place", "Razor", "Director: Apply Effect"]);
    }
}
