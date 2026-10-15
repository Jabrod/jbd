//! The Director's tools: what the model is offered ([`definitions`]) and what each one does
//! ([`run`]).
//!
//! The tools are deliberately few and general. Instead of one tool per editor feature, the model
//! gets the editor's own command list (`list_commands`) and runs any of them (`run_command`), the
//! same commands the menus, shortcuts and the MCP server use. A feature added to the editor is
//! usable by the Director the day it lands.
//!
//! To add a tool: add its JSON schema to [`definitions`], a branch to [`run`], and a line about
//! it to the system prompt in `agent.rs`.

use serde_json::{Value, json};

use crate::{Ask, Catalogue, Editor, Entry, Mode, Model, Shared, Step, StepState, Todo, TodoState, lock};

/// Longest tool result sent back to the model. Inspect results of a big project are long; the
/// model needs the shape, not every byte, and every character costs credits.
const MAX_RESULT: usize = 12_000;

/// Commands the Director may never run.
///
/// Two reasons. Some would throw the person's work away or can't be undone (opening another
/// project, reverting, discarding the recovery file). The rest are the ones that could turn the
/// Director into a general-purpose tool for someone who asked it to do something other than edit
/// their video: writing files anywhere on the disk (`file.save*`, the exporters), reading other
/// files into the project (`file.open`), or changing JBD's settings. Exporting is the person's
/// own choice, made in JBD's export window, not the agent's.
///
/// Undoable timeline edits are all allowed: that is the Director's job, and Undo covers it.
const BLOCKED: &[&str] = &[
    "file.open",
    "file.openDemoProject",
    "file.newProject",
    "file.revert",
    "file.recover",
    "file.discardRecovery",
    "file.save",
    "file.saveAs",
    "file.saveCopy",
    "file.exportEdl",
    "file.exportFcpxml",
    "file.exportOtio",
    "file.importAaf",
    "prefs.set",
    "prefs.reset",
    "sequence.close",
    "sequence.deleteRenderFiles",
    "sequence.deleteRenderFilesInToOut",
];

/// Where generated clips may be imported from. `file.import` is allowed (the Director imports
/// what it generates), but only from JBD's own folder: without this, "import C:\\Users\\…\\
/// Documents\\*" would make the editor a file reader for whoever asked.
fn import_allowed(params: &Value, media_dir: &std::path::Path) -> Result<(), String> {
    let paths = params.get("paths").and_then(Value::as_array).cloned().unwrap_or_default();
    let single = params.get("path").and_then(Value::as_str).map(|p| Value::String(p.to_string()));
    let all: Vec<String> = paths.iter().chain(single.iter()).filter_map(|v| v.as_str().map(str::to_string)).collect();
    if all.is_empty() {
        return Err("needs `paths`".into());
    }
    let dir = media_dir.canonicalize().unwrap_or_else(|_| media_dir.to_path_buf());
    for p in &all {
        let path = std::path::Path::new(p);
        let real = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if !real.starts_with(&dir) {
            return Err("the Director can only import clips it generated. Ask the person to import their own footage with File ▸ Import.".into());
        }
    }
    Ok(())
}

/// The tools, as OpenAI-style function definitions.
pub fn definitions() -> Vec<Value> {
    let f = |name: &str, description: &str, parameters: Value| json!({"type": "function", "function": {"name": name, "description": description, "parameters": parameters}});
    vec![
        f(
            "list_commands",
            "Search JBD's editor commands. Returns `id — label — params` lines. Use it to find the command for a job before running it.",
            json!({"type": "object", "properties": {"filter": {"type": "string", "description": "Words to match in the id or label, e.g. \"razor\", \"effect\", \"caption\", \"marker\". Empty lists everything."}}}),
        ),
        f(
            "inspect",
            "Read the current state. `sequence`: a summary of the open timeline (every track and clip with ids and times in seconds). Add `clip` for one clip in full (its effects, keyframes and, for titles, the text layers with their measured bounds), or `track` (\"V1\", \"A2\") for one track's clips. `project`: bins and media items. `state`: editor state. `history`: the undo history.",
            json!({"type": "object", "properties": {
                "what": {"type": "string", "enum": ["project", "sequence", "state", "history"]},
                "clip": {"type": "integer", "description": "One clip in full detail."},
                "track": {"type": "string", "description": "One track, e.g. \"V2\"."}
            }, "required": ["what"]}),
        ),
        f(
            "run_command",
            "Run one editor command by id with JSON params (see list_commands for each command's params). Times can be given as {\"seconds\": 3.5} style keys: `seconds` instead of `time`, or `<prefix>Seconds` instead of `<prefix>Time`. Every edit is undoable.",
            json!({"type": "object", "properties": {"id": {"type": "string"}, "params": {"type": "object"}}, "required": ["id"]}),
        ),
        f(
            "generate_shots",
            "Generate new video clips with the person's video model, ALL IN PARALLEL in one call (always put every shot of a plan in one call: it's several times faster than one by one), import them and lay them out back to back on V1 in shot order. Costs credits. Each prompt is concrete and visual: subject with its exact look, action, setting, camera move and lens, light, mood, style; repeat the same look in every prompt so the shots cut together.",
            json!({"type": "object", "properties": {
                "shots": {"type": "array", "items": {"type": "object", "properties": {
                    "prompt": {"type": "string"},
                    "seconds": {"type": "integer", "description": "Length (snapped to what the model allows; 4-8 is typical)."},
                    "audio": {"type": "boolean", "description": "Sound with the clip (ambient, effects): yes for action, product and lifestyle shots in ads and reels; no when music or a voiceover will cover it. Costs more."}
                }, "required": ["prompt"]}},
                "aspect": {"type": "string", "description": "\"16:9\", \"9:16\" or \"1:1\": match the timeline."},
                "place_at_seconds": {"type": "number", "description": "Where the first shot starts. Omit for the end of V1; -1 to only import."}
            }, "required": ["shots"]}),
        ),
        f(
            "regenerate_clip",
            "Make a new take of a clip the Director generated earlier, from its saved recipe (prompt, model, length). By default it replaces the clip in place (same position, length and effects). With variation: true the new take goes on the track above for comparison. Pass a new `prompt` to change what's in the shot.",
            json!({"type": "object", "properties": {
                "clip": {"type": "integer", "description": "The clip id on the timeline (from PROJECT NOW)."},
                "prompt": {"type": "string", "description": "A replacement prompt; omit to reuse the original."},
                "variation": {"type": "boolean", "description": "Keep the original and add the new take above it."}
            }, "required": ["clip"]}),
        ),
        f(
            "assemble",
            "Put existing media on the timeline in order in one call: each item trimmed to its best part, back to back, with an optional transition between them. Use it to build or rebuild an edit from the media list instead of many timeline.place calls.",
            json!({"type": "object", "properties": {
                "clips": {"type": "array", "items": {"type": "object", "properties": {
                    "item": {"type": "integer", "description": "A media item id from PROJECT NOW."},
                    "in_seconds": {"type": "number", "description": "Start inside the source (default 0)."},
                    "out_seconds": {"type": "number", "description": "End inside the source (default: its end)."}
                }, "required": ["item"]}},
                "start_at_seconds": {"type": "number", "description": "Where the first clip starts (default: the end of V1)."},
                "track": {"type": "string", "description": "Video track, default \"V1\"."},
                "transition": {"type": "string", "description": "\"none\" (default; hard cuts), \"cross_dissolve\", or another transition from list_commands/effects."},
                "transition_frames": {"type": "integer", "description": "Transition length in frames (default 12)."}
            }, "required": ["clips"]}),
        ),
        f(
            "add_title",
            "The ONLY way to add or change on-screen text. It is measured with the real font and corrected until it sits centred inside the safe area (never clipped), wrapped onto two balanced lines if long, with a shadow so it reads on any picture. Pass `clip` to change an existing title (text, colour, font, position, animation) instead of adding another.",
            json!({"type": "object", "properties": {
                "text": {"type": "string", "description": "Short: 2-6 words reads best."},
                "clip": {"type": "integer", "description": "An existing title to change instead of adding one."},
                "start_seconds": {"type": "number"},
                "duration_seconds": {"type": "number", "description": "Default 3."},
                "position": {"type": "string", "enum": ["center", "lower_third", "top", "end_card"]},
                "color": {"type": "string", "description": "\"#rrggbb\", default white."},
                "font": {"type": "string", "description": "A font family installed on this computer; default Inter."},
                "weight": {"type": "string", "description": "\"Bold\" (default), \"Black\", \"SemiBold\", \"Regular\"."},
                "background": {"type": "boolean", "description": "A box behind the text (for busy footage)."},
                "animation": {"type": "string", "enum": ["none", "fade_in", "fade_in_out", "pop_in", "slide_up"]}
            }}),
        ),
        f(
            "animate",
            "Animate a clip or title with keyframes. Times are SECONDS FROM THE CLIP'S OWN START (0 = its first frame), not timeline time; the tool converts them and checks the keyframes really exist. Use a preset, or keyframes for scale (100 = normal), opacity (0-100), position ([x, y] in pixels of the frame) or rotation (degrees).",
            json!({"type": "object", "properties": {
                "clip": {"type": "integer"},
                "preset": {"type": "string", "enum": ["fade_in", "fade_out", "fade_in_out", "pop_in", "slide_up", "zoom_in", "zoom_out"]},
                "keyframes": {"type": "array", "items": {"type": "object", "properties": {
                    "param": {"type": "string", "enum": ["scale", "opacity", "position", "rotation"]},
                    "at_seconds": {"type": "number", "description": "From the clip's start."},
                    "value": {}
                }, "required": ["param", "at_seconds", "value"]}}
            }, "required": ["clip"]}),
        ),
        f(
            "voiceover",
            "Record narration from a script with an AI voice and put it on its own audio track. Write the script for the ear: short sentences, in the person's language (English, Hindi or Hinglish written in Roman script). About 2.5 words per second of video.",
            json!({"type": "object", "properties": {
                "script": {"type": "string"},
                "tone": {"type": "string", "description": "How to read it, e.g. \"energetic, confident sports ad\"."},
                "voice": {"type": "string", "description": "Optional voice name."},
                "place_at_seconds": {"type": "number", "description": "Default 0."}
            }, "required": ["script"]}),
        ),
        f(
            "place_on_object",
            "Put a logo, badge or any picture ON a thing in a shot (a football, a bottle, a shirt, a sign) and keep it there while the thing moves: it goes on the track above the shot, sized and positioned on the point, its solid background keyed out if asked, and JBD's tracker makes it follow the thing. Look at the shot first (look with the clip: the frames get a 10 % grid) and give the thing's centre as [x %, y %] of the frame at the time you saw it. Never put an overlay in a corner when the person asked for it ON something.",
            json!({"type": "object", "properties": {
                "target_clip": {"type": "integer", "description": "The shot the thing is in."},
                "point": {"type": "array", "items": {"type": "number"}, "description": "[x %, y %] of the frame, from the top-left: the centre of the thing."},
                "at_seconds": {"type": "number", "description": "Timeline time where you saw it there (default: the middle of the shot)."},
                "item": {"type": "integer", "description": "The overlay's media item (the attached logo), placed over exactly that shot."},
                "overlay_clip": {"type": "integer", "description": "Or an overlay clip already on the timeline, to move onto the thing."},
                "size_pct": {"type": "number", "description": "Its width as % of the frame width; match the thing (a ball in a wide shot is often 5-10 %). Default 12."},
                "remove_background": {"type": "boolean", "description": "Key out the overlay's solid background (a logo on white)."},
                "background_color": {"type": "string", "description": "That background's colour, default \"#ffffff\"."},
                "track": {"type": "boolean", "description": "Follow the thing as it moves (default true); false for something that doesn't move."}
            }, "required": ["target_clip", "point"]}),
        ),
        f(
            "look",
            "SEE the edit: renders frames as it plays now into one contact sheet you can see. Use it to find things in shots (where the ball, the product, the face is), to check framing and titles, and to review your work before you say you're done. Give `clip` for frames across one clip (with a 10 % grid to read positions from), `seconds` for exact timeline times, or nothing for the whole timeline (its first and last frames and every transition included): you must look at the whole timeline after your last change before you reply.",
            json!({"type": "object", "properties": {
                "clip": {"type": "integer", "description": "Frames from the start, middle and end of this clip."},
                "seconds": {"type": "array", "items": {"type": "number"}, "description": "Exact timeline times (up to 9)."},
                "count": {"type": "integer", "description": "How many frames (default 3 for a clip, 6 for the timeline)."},
                "grid": {"type": "boolean", "description": "A grid line every 10 % of the frame (default on for a clip)."}
            }}),
        ),
        f(
            "web_search",
            "Search the web and get a short answer with sources. Use it whenever the request names something you aren't sure about: a product (\"Dunk Low Panda\"), a brand's style, a place, a person, an event, a trend, a song. Research before planning, not after.",
            json!({"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}),
        ),
        f(
            "read_webpage",
            "Read a web page (title, headings, text, image links). Use it on a source from web_search when you need details: a product's exact look and colours, a campaign's tagline, a brand's visual style.",
            json!({"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]}),
        ),
        f(
            "find_images",
            "Find freely licensed stock photos (with author, licence and a ready credit line). Import one with import_media.",
            json!({"type": "object", "properties": {
                "query": {"type": "string"},
                "orientation": {"type": "string", "enum": ["landscape", "portrait", "square"]},
                "count": {"type": "integer"}
            }, "required": ["query"]}),
        ),
        f(
            "find_music",
            "Find Creative Commons music or sound effects that may be used in a video (with a ready credit line). Import one with import_media. Pick music that fits the mood and pace of the edit.",
            json!({"type": "object", "properties": {
                "query": {"type": "string", "description": "Mood, genre or sound, e.g. \"upbeat hip hop instrumental\", \"whoosh\", \"crowd cheering\"."},
                "kind": {"type": "string", "enum": ["music", "sound_effect"]},
                "count": {"type": "integer"}
            }, "required": ["query"]}),
        ),
        f(
            "find_video",
            "Find freely licensed stock video clips (with a ready credit line). Import one with import_media. Use real footage when it serves the edit better than a generated clip, or to save credits.",
            json!({"type": "object", "properties": {
                "query": {"type": "string"},
                "orientation": {"type": "string", "enum": ["landscape", "portrait", "square"]},
                "count": {"type": "integer"}
            }, "required": ["query"]}),
        ),
        f(
            "import_media",
            "Download a picture, music/sound or video from an https link (from find_images, find_music, find_video or read_webpage) into the project, and optionally place it. Sound goes on its own audio track. Only use media that is licensed for use (stock results) or that the person asked for; keep the credit line it came with.",
            json!({"type": "object", "properties": {
                "url": {"type": "string"},
                "place_at_seconds": {"type": "number", "description": "Put it on the timeline here; omit to only import."},
                "attribution": {"type": "string", "description": "The credit line from the search result (shown back to you to keep)."},
                "role": {"type": "string", "enum": ["footage", "logo", "overlay"], "description": "footage (default) is scaled to fill the frame; a logo or overlay keeps its own size."}
            }, "required": ["url"]}),
        ),
        f(
            "todo_write",
            "Replace your to-do list for this request (shown to the person). Use it for anything with 3+ steps; keep exactly one item `doing` while you work.",
            json!({"type": "object", "properties": {"todos": {"type": "array", "items": {"type": "object", "properties": {
                "text": {"type": "string"}, "state": {"type": "string", "enum": ["todo", "doing", "done"]}
            }, "required": ["text", "state"]}}}, "required": ["todos"]}),
        ),
    ]
}

/// Everything a tool needs.
pub struct Ctx<'a> {
    pub editor: &'a dyn Editor,
    pub model: &'a dyn Model,
    pub shared: &'a std::sync::Mutex<Shared>,
    pub cancel: &'a crate::Cancel,
    pub mode: Mode,
    pub catalogue: &'a Catalogue,
    /// The video model the person picked.
    pub video_model: &'a str,
    /// Called after `shared` changes, so the UI redraws.
    pub repaint: &'a (dyn Fn() + Send + Sync),
    /// Credits this request has spent on generation so far (for [`AUTO_CAP`]).
    pub spent: std::cell::Cell<u32>,
    /// Sound on generated clips, as the person set it on the video chip.
    pub sound: crate::Sound,
    /// The person's connectors (`mcp__…` tools).
    pub external: Option<&'a dyn crate::ExternalTools>,
    /// Pictures a tool wants the model to see (contact sheets from `look`): the agent sends them
    /// in the next message.
    pub seen: std::cell::RefCell<Vec<String>>,
}

/// In Auto edits, clips generate without asking until one request has spent this many credits
/// (a six-shot ad on Veo 3.1 Lite is about 110); past that, each clip asks. So a runaway loop
/// can never drain a month of credits on its own. Ask mode asks for every clip.
pub const AUTO_CAP: u32 = 120;

/// What a tool returns to the model, plus what the run should remember.
pub struct Outcome {
    pub text: String,
    pub ok: bool,
    /// The project changed (for "Undo all").
    pub edited: bool,
    pub credits: u32,
}

/// How a result that is only partly done starts, so the model (and the transcript, which shows
/// it as a partial step) never mistakes it for a success.
pub const PARTLY: &str = "Partly done:";

impl Outcome {
    pub(crate) fn ok(text: impl Into<String>) -> Self {
        Outcome { text: text.into(), ok: true, edited: false, credits: 0 }
    }
    /// Some of the work happened (`done`), some didn't (`missing`): not ok, so the model fixes
    /// the rest, but the project did change.
    pub(crate) fn partial(done: impl Into<String>, missing: &[String], credits: u32) -> Self {
        Outcome { text: format!("{PARTLY} {} NOT done: {}", done.into(), missing.join("; ")), ok: false, edited: true, credits }
    }
    pub(crate) fn err(text: impl Into<String>) -> Self {
        Outcome { text: format!("Error: {}", text.into()), ok: false, edited: false, credits: 0 }
    }
}

/// The one-line title the transcript shows for a call.
pub fn title(name: &str, args: &Value) -> String {
    let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("");
    match name {
        "list_commands" => match s("filter") {
            "" => "Looked at the command list".into(),
            f => format!("Searched commands: {f}"),
        },
        "inspect" => format!("Read the {}", if s("what").is_empty() { "project" } else { s("what") }),
        "run_command" => {
            let params = args.get("params").map(short_params).unwrap_or_default();
            if params.is_empty() { s("id").to_string() } else { format!("{} {params}", s("id")) }
        }
        "generate_video" => format!("Generate video: {}", clip(s("prompt"), 60)),
        "generate_shots" => format!("Generate {} shots", args.get("shots").and_then(Value::as_array).map_or(0, Vec::len)),
        "assemble" => format!("Assemble {} clips", args.get("clips").and_then(Value::as_array).map_or(0, Vec::len)),
        "animate" => format!("Animate clip {}", args.get("clip").cloned().unwrap_or(Value::Null)),
        "look" => match args.get("clip").and_then(Value::as_u64) {
            Some(c) => format!("Looked at clip {c}"),
            None => "Looked at the edit".into(),
        },
        "regenerate_clip" => {
            if args.get("variation").and_then(Value::as_bool).unwrap_or(false) {
                "Make a variation".into()
            } else {
                "Regenerate a clip".into()
            }
        }
        "add_title" => format!("Title: {}", clip(s("text"), 50)),
        "voiceover" => "Record voiceover".into(),
        "place_on_object" => format!("Put the overlay on the shot (clip {})", args.get("target_clip").cloned().unwrap_or(Value::Null)),
        "web_search" => format!("Searched the web: {}", clip(s("query"), 60)),
        "read_webpage" => format!("Read {}", clip(s("url").trim_start_matches("https://").trim_start_matches("www."), 60)),
        "find_images" => format!("Found images: {}", clip(s("query"), 60)),
        "find_music" => format!("Found music: {}", clip(s("query"), 60)),
        "find_video" => format!("Found stock video: {}", clip(s("query"), 60)),
        "import_media" | "import_image" => "Imported media".into(),
        "todo_write" => "Updated the plan".into(),
        other => other.to_string(),
    }
}

/// `{"seconds":4,"track":"V1"}` → `seconds 4, track V1`, kept short.
fn short_params(p: &Value) -> String {
    let Some(map) = p.as_object() else { return String::new() };
    let parts: Vec<String> = map
        .iter()
        .take(4)
        .map(|(k, v)| match v {
            Value::String(s) => format!("{k} {}", clip(s, 24)),
            Value::Array(a) => format!("{k} [{}]", a.len()),
            Value::Object(_) => format!("{k} {{…}}"),
            other => format!("{k} {other}"),
        })
        .collect();
    parts.join(", ")
}

/// Cut text to `max` characters (on a character boundary), with an ellipsis.
pub fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// What a tool does, for the permission rules in [`gate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Reads, looks, researches, plans: always allowed, never asks.
    Read,
    /// Changes the project (undoable).
    Edit,
    /// Spends credits (generation, voice). The tool asks with its exact cost itself.
    Spend,
    /// A connector's tool: runs outside JBD.
    External,
}

/// The kind of a tool call.
pub fn kind(name: &str, args: &Value) -> Kind {
    match name {
        "list_commands" | "inspect" | "look" | "web_search" | "read_webpage" | "find_images" | "find_music" | "find_video" | "todo_write" => Kind::Read,
        "generate_video" | "generate_shots" | "regenerate_clip" | "voiceover" => Kind::Spend,
        "run_command" => match args.get("id").and_then(Value::as_str) {
            Some(id) if is_query(id) => Kind::Read,
            _ => Kind::Edit,
        },
        n if n.starts_with("mcp__") => Kind::External,
        _ => Kind::Edit,
    }
}

/// The permission rules, in one place for every tool:
/// - Plan: only reading, looking and research;
/// - Ask: every change and every connector call asks first;
/// - Auto edits: changes run; connectors not marked trusted ask.
///
/// Spending tools are allowed through here (except in Plan) and ask with their exact cost
/// themselves ([`may_spend`]). Returns the result to give the model when the call must not run.
pub fn gate(ctx: &Ctx, step: u64, name: &str, args: &Value) -> Option<Outcome> {
    let k = kind(name, args);
    if k == Kind::Read {
        return None;
    }
    if ctx.mode == Mode::Plan {
        return Some(Outcome::err(
            "Plan mode is on, so nothing can be changed, generated or sent outside JBD. Describe the step instead; the person will switch modes to run it.",
        ));
    }
    let ask_now = match k {
        Kind::Edit => ctx.mode == Mode::Ask,
        Kind::External => ctx.mode == Mode::Ask || !ctx.external.is_some_and(|x| x.is_trusted(name)),
        _ => false,
    };
    if ask_now {
        let (heading, detail) = match k {
            Kind::External => (format!("Use {}", name.trim_start_matches("mcp__").replacen("__", " · ", 1)), clip(&args.to_string(), 400)),
            _ => ("Make this change".to_string(), title(name, args)),
        };
        if !ask(ctx, step, &heading, &detail, 0) {
            return Some(Outcome {
                text: "The person said no to this step. Don't retry it; ask or try something else.".into(),
                ok: false,
                edited: false,
                credits: 0,
            });
        }
    }
    None
}

/// May a spending tool go ahead with `credits` without asking? Ask mode asks for every one;
/// Auto edits only past [`AUTO_CAP`] for this request.
pub fn may_spend(ctx: &Ctx, credits: u32) -> bool {
    ctx.mode == Mode::AutoEdits && ctx.spent.get().saturating_add(credits) <= AUTO_CAP
}

/// Run one tool call. Never panics and never returns an error to the caller: a failure is text
/// for the model ("Error: …") so it can fix its call, as a person would. Every call passes
/// [`gate`] first.
pub fn run(ctx: &Ctx, step: u64, name: &str, args: &Value) -> Outcome {
    if let Some(refused) = gate(ctx, step, name, args) {
        return refused;
    }
    match name {
        "list_commands" => list_commands(args.get("filter").and_then(Value::as_str).unwrap_or("")),
        "inspect" => inspect(ctx, args),
        "run_command" => run_command(ctx, args),
        "generate_video" | "generate_shots" => crate::production::generate_shots(ctx, step, args),
        "assemble" => crate::production::assemble(ctx, args),
        "animate" => crate::production::animate(ctx, args),
        "look" => crate::look::look(ctx, args),
        "regenerate_clip" => crate::production::regenerate_clip(ctx, step, args),
        "add_title" => crate::production::add_title(ctx, args),
        "place_on_object" => crate::overlay::place_on_object(ctx, args),
        "voiceover" => crate::production::voiceover(ctx, step, args),
        "web_search" | "read_webpage" | "find_images" | "find_music" | "find_video" => web(ctx, name, args),
        "import_media" | "import_image" => import_media(ctx, args),
        "todo_write" => todo_write(ctx, args),
        other if other.starts_with("mcp__") => external(ctx, other, args),
        other => Outcome::err(format!("there is no tool called `{other}`")),
    }
}

fn list_commands(filter: &str) -> Outcome {
    let words: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
    let lines: Vec<String> = filmcraft_engine::command_specs()
        .iter()
        .filter(|c| !BLOCKED.contains(&c.id))
        .filter(|c| {
            let hay = format!("{} {}", c.id, c.label).to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .map(|c| if c.params.is_empty() || c.params == "{}" { format!("{} — {}", c.id, c.label) } else { format!("{} — {} — {}", c.id, c.label, c.params) })
        .collect();
    if lines.is_empty() {
        return Outcome::ok(format!("No command matches \"{filter}\". Try a shorter word."));
    }
    let n = lines.len();
    let mut text = lines.into_iter().take(120).collect::<Vec<_>>().join("\n");
    if n > 120 {
        text.push_str(&format!("\n… and {} more; narrow the filter.", n - 120));
    }
    Outcome::ok(text)
}

/// Read the state. A whole timeline as JSON can run past what the model should read (every
/// effect parameter of every clip), and cutting JSON short leaves the model half a picture
/// that looks whole. So the sequence comes as the compact summary from `digest.rs`, and the full
/// detail only for the one clip or track asked for.
fn inspect(ctx: &Ctx, args: &Value) -> Outcome {
    let what = args.get("what").and_then(Value::as_str).unwrap_or("project");
    let call = |command: &str| ctx.editor.call("engine.execute", json!({"command": command, "params": {}}));
    match what {
        "sequence" => {
            let seq = match call("sequence.inspect") {
                Ok(v) => v,
                Err(e) => return Outcome::err(e),
            };
            if let Some(clip_id) = args.get("clip").and_then(Value::as_u64) {
                let mut found = None;
                for kind in ["video", "audio"] {
                    for c in seq
                        .get(kind)
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .flat_map(|t| t.get("items").and_then(Value::as_array).into_iter().flatten())
                    {
                        if c.get("clip").and_then(Value::as_u64) == Some(clip_id) {
                            found = Some(c.clone());
                        }
                    }
                }
                let Some(mut c) = found else { return Outcome::err(format!("there is no clip {clip_id} on the timeline")) };
                // titles: their text layers with the measured bounds
                if let Ok(g) = ctx.editor.call("engine.execute", json!({"command": "graphics.list", "params": {"clip": clip_id}}))
                    && g.get("layers").and_then(Value::as_array).is_some_and(|l| !l.is_empty())
                {
                    c["graphic"] = json!({"canvas": g.get("canvas"), "layers": g.get("layers")});
                }
                return Outcome::ok(complete_json(&c));
            }
            if let Some(track) = args.get("track").and_then(Value::as_str) {
                let (kind, n) = match track.chars().next() {
                    Some('A') | Some('a') => ("audio", track[1..].parse::<usize>().unwrap_or(1)),
                    _ => ("video", track.get(1..).and_then(|x| x.parse::<usize>().ok()).unwrap_or(1)),
                };
                let Some(t) = seq.get(kind).and_then(Value::as_array).and_then(|a| a.get(n.saturating_sub(1))) else {
                    return Outcome::err(format!("there is no track {track}"));
                };
                let mut t = t.clone();
                // the clips' effect parameters are long; inspect one clip for them
                if let Some(items) = t.get_mut("items").and_then(Value::as_array_mut) {
                    for c in items.iter_mut() {
                        if let Some(o) = c.as_object_mut() {
                            let fx: Vec<Value> = o
                                .get("effects")
                                .and_then(Value::as_array)
                                .map(|e| e.iter().filter_map(|x| x.get("effect").cloned()).collect())
                                .unwrap_or_default();
                            o.insert("effects".into(), json!(fx));
                        }
                    }
                }
                return Outcome::ok(complete_json(&t));
            }
            Outcome::ok(crate::digest::digest(None, Some(&seq)))
        }
        "project" | "state" | "history" => {
            let command = match what {
                "project" => "project.inspect",
                "state" => "state.inspect",
                _ => "history.list",
            };
            match call(command) {
                Ok(v) => Outcome::ok(complete_json(&v)),
                Err(e) => Outcome::err(e),
            }
        }
        other => Outcome::err(format!("can't inspect `{other}`; use project, sequence, state or history")),
    }
}

/// JSON for the model, whole or not at all: if it is too long, say so and how to narrow it,
/// instead of cutting it mid-object.
pub fn complete_json(v: &Value) -> String {
    let text = v.to_string();
    if text.len() <= MAX_RESULT {
        return text;
    }
    let keys = v.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>().join(", ")).unwrap_or_default();
    format!(
        "(The full result is {} KB, too long to read at once. It has: {keys}. Ask for less: inspect one clip or one track, or the summary.)",
        text.len() / 1024
    )
}

/// Is this command a read-only query (allowed in Plan mode, never asks)?
fn is_query(id: &str) -> bool {
    filmcraft_engine::find_command(id).is_some_and(|c| !c.journal)
}

fn run_command(ctx: &Ctx, args: &Value) -> Outcome {
    let Some(id) = args.get("id").and_then(Value::as_str) else { return Outcome::err("missing `id`") };
    // On-screen text goes through add_title, which measures and centres it; raw text layers
    // were placed by their left edge and ran off the frame.
    if matches!(id, "graphics.newText" | "graphics.newVerticalText") {
        return Outcome::err("use add_title for on-screen text: it measures, centres and fits the text (graphics.newText places it by its left edge).");
    }
    if BLOCKED.contains(&id) {
        return Outcome::err(format!("`{id}` is not available to the Director (it would close the project or change settings). Ask the person to do it."));
    }
    if filmcraft_engine::find_command(id).is_none() {
        return Outcome::err(format!("there is no command `{id}`; use list_commands to find the right one"));
    }
    let params = args.get("params").cloned().filter(Value::is_object).unwrap_or_else(|| json!({}));
    if id == "file.import"
        && let Err(e) = import_allowed(&params, &ctx.editor.media_dir())
    {
        return Outcome::err(e);
    }
    // (Plan and Ask were handled by `gate` before this ran)
    let query = is_query(id);
    match ctx.editor.call("engine.execute", json!({"command": id, "params": params})) {
        Ok(v) => Outcome { text: complete_json(&v), ok: true, edited: !query, credits: 0 },
        Err(e) => Outcome::err(e),
    }
}

/// A connector's tool (an MCP server the person added in Settings ▸ Connectors). It runs
/// outside JBD, so `gate` asked first unless the person marked that connector trusted.
fn external(ctx: &Ctx, name: &str, args: &Value) -> Outcome {
    let Some(x) = ctx.external else { return Outcome::err(format!("`{name}` isn't available: its connector is off")) };
    match x.call(name, args.clone()) {
        Ok(text) => Outcome::ok(clip(&text, MAX_RESULT)),
        Err(e) => Outcome::err(e),
    }
}

/// The internet tools run on jabrod.com (the server keeps the keys and counts the credits).
fn web(ctx: &Ctx, tool: &str, args: &Value) -> Outcome {
    let args = match tool {
        "web_search" => json!({"query": args.get("query").and_then(Value::as_str).unwrap_or("").trim()}),
        "read_webpage" => json!({"url": args.get("url").and_then(Value::as_str).unwrap_or("").trim()}),
        _ => {
            let mut a = json!({"query": args.get("query").and_then(Value::as_str).unwrap_or("").trim()});
            if let Some(o) = args.get("orientation").and_then(Value::as_str) {
                a["orientation"] = json!(o);
            }
            if let Some(k) = args.get("kind").and_then(Value::as_str) {
                a["kind"] = json!(k);
            }
            a["count"] = json!(args.get("count").and_then(Value::as_u64).unwrap_or(6).clamp(1, 12));
            a
        }
    };
    match ctx.model.web(tool, args) {
        Ok(v) => Outcome::ok(clip(&v.to_string(), MAX_RESULT)),
        Err(e) => Outcome::err(e),
    }
}

/// Bring a picture, sound or video from a link into the project. Sound goes on a new audio track
/// of its own, so it never replaces the clips' sound; pictures and video go on V1.
fn import_media(ctx: &Ctx, args: &Value) -> Outcome {
    let Some(url) = args.get("url").and_then(Value::as_str) else { return Outcome::err("missing `url`") };
    let path = match ctx.model.download_media(url.trim(), &ctx.editor.media_dir()) {
        Ok(p) => p,
        Err(e) => return Outcome::err(e),
    };
    let audio = path.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e, "mp3" | "wav" | "ogg" | "flac" | "m4a"));
    let imported = match ctx.editor.call("engine.execute", json!({"command": "file.import", "params": {"paths": [path.to_string_lossy()]}})) {
        Ok(v) => v,
        Err(e) => return Outcome::err(format!("it downloaded but importing it failed: {e}")),
    };
    let item = imported.get("items").and_then(Value::as_array).and_then(|a| a.first()).cloned();
    let mut text = format!("Imported {} as item {}.", if audio { "the sound" } else { "it" }, item.clone().unwrap_or(Value::Null));
    if let (Some(item), Some(at)) = (item, args.get("place_at_seconds").and_then(Value::as_f64)) {
        let mut params = json!({"item": item, "seconds": at.max(0.0)});
        if audio {
            // its own track: count the audio tracks, add one, place there
            let tracks = ctx
                .editor
                .call("engine.execute", json!({"command": "sequence.inspect", "params": {}}))
                .ok()
                .and_then(|s| s.get("audio").and_then(Value::as_array).map(Vec::len))
                .unwrap_or(1);
            let _ = ctx.editor.call("engine.execute", json!({"command": "sequence.addTracks", "params": {"video": 0, "audio": 1}}));
            params["track"] = json!(format!("A{}", tracks + 1));
        }
        match ctx.editor.call("engine.execute", json!({"command": "timeline.place", "params": params})) {
            Ok(v) => {
                // footage covers the frame; a logo or overlay keeps its own size
                let role = args.get("role").and_then(Value::as_str).unwrap_or("footage");
                if !audio && role == "footage" {
                    let placed: Vec<u64> = v.get("clips").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
                    crate::production::fit_clips(ctx, &placed, "fill");
                }
                text.push_str(&format!(" Placed at {at:.1} s{}.", if audio { " on its own audio track" } else { "" }))
            }
            Err(e) => text.push_str(&format!(" Placing it failed ({e}).")),
        }
    }
    if let Some(credit) = args.get("attribution").and_then(Value::as_str).filter(|c| !c.trim().is_empty()) {
        text.push_str(&format!(" Credit to keep: {}", clip(credit, 300)));
    }
    Outcome { text, ok: true, edited: true, credits: 0 }
}

fn todo_write(ctx: &Ctx, args: &Value) -> Outcome {
    let Some(items) = args.get("todos").and_then(Value::as_array) else { return Outcome::err("missing `todos`") };
    let todos: Vec<Todo> = items
        .iter()
        .take(20)
        .filter_map(|t| {
            let text = t.get("text").and_then(Value::as_str)?.trim();
            let state = match t.get("state").and_then(Value::as_str) {
                Some("doing") => TodoState::Doing,
                Some("done") => TodoState::Done,
                _ => TodoState::Todo,
            };
            (!text.is_empty()).then(|| Todo { text: clip(text, 120), state })
        })
        .collect();
    lock(ctx.shared).todos = todos;
    (ctx.repaint)();
    Outcome::ok("Plan updated.")
}

/// Show a permission ask and wait for the answer. False when the person says no or stops.
pub(crate) fn ask(ctx: &Ctx, step: u64, title: &str, detail: &str, credits: u32) -> bool {
    lock(ctx.shared).ask = Some(Ask { id: step, title: title.into(), detail: detail.into(), credits, answer: None });
    (ctx.repaint)();
    loop {
        if ctx.cancel.stopped() {
            lock(ctx.shared).ask = None;
            return false;
        }
        {
            let mut s = lock(ctx.shared);
            match s.ask.as_ref().map(|a| (a.id, a.answer)) {
                Some((id, Some(yes))) if id == step => {
                    s.ask = None;
                    return yes;
                }
                Some(_) => {}
                // the UI dropped the ask (closed the project): treat it as no
                None => return false,
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Start a step in the transcript; returns its id.
pub fn begin_step(shared: &std::sync::Mutex<Shared>, name: &str, args: &Value) -> u64 {
    let mut s = lock(shared);
    let id = s.next_id();
    let detail = format!("{name} {}", clip(&args.to_string(), 2000));
    s.push(Entry::Step(Step {
        id,
        title: title(name, args),
        detail,
        result: String::new(),
        state: StepState::Running,
        took: 0.0,
        credits: 0,
        progress: None,
        expanded: false,
        started: crate::now_seconds(),
    }));
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_a_valid_schema() {
        for d in definitions() {
            let f = &d["function"];
            assert!(f["name"].as_str().is_some_and(|n| !n.is_empty()));
            assert!(f["description"].as_str().is_some_and(|n| n.len() > 20));
            assert_eq!(f["parameters"]["type"], "object");
        }
    }

    #[test]
    fn command_search_finds_the_razor_and_hides_blocked_ones() {
        let out = list_commands("razor");
        assert!(out.ok);
        assert!(out.text.contains("timeline.razor"), "{}", out.text);
        let all = list_commands("");
        assert!(!all.text.lines().any(|l| l.starts_with("file.open ")), "blocked commands must not be offered");
    }

    #[test]
    fn nothing_that_writes_files_or_settings_is_offered() {
        let all = list_commands("");
        for id in ["file.saveAs", "file.exportEdl", "prefs.set", "file.open"] {
            assert!(!all.text.lines().any(|l| l.starts_with(&format!("{id} "))), "{id} must not be offered");
        }
        // every blocked id is a real command (a typo would silently block nothing)
        for id in BLOCKED {
            assert!(filmcraft_engine::find_command(id).is_some(), "{id} is not a command");
        }
    }

    #[test]
    fn import_only_from_the_directors_own_folder() {
        let dir = std::env::temp_dir().join(format!("jbd-guard-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let inside = dir.join("clip.mp4");
        let _ = std::fs::write(&inside, b"x");
        assert!(import_allowed(&json!({"paths": [inside.to_string_lossy()]}), &dir).is_ok());
        assert!(import_allowed(&json!({"paths": ["C:\\Windows\\win.ini"]}), &dir).is_err());
        assert!(import_allowed(&json!({"path": "/etc/passwd"}), &dir).is_err());
        // `..` can't climb out of the folder
        let sneaky = dir.join("..").join("..").join("secret.txt");
        assert!(import_allowed(&json!({"paths": [sneaky.to_string_lossy()]}), &dir).is_err());
        assert!(import_allowed(&json!({}), &dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn queries_are_read_only() {
        assert!(is_query("sequence.inspect"));
        assert!(!is_query("timeline.razor"));
        // an unknown id is not a query (it fails later with a helpful message)
        assert!(!is_query("no.such.command"));
    }

    #[test]
    fn titles_are_short_and_readable() {
        assert_eq!(title("inspect", &json!({"what": "sequence"})), "Read the sequence");
        assert_eq!(title("run_command", &json!({"id": "timeline.razor", "params": {"seconds": 4}})), "timeline.razor seconds 4");
        let long = title("generate_video", &json!({"prompt": "x".repeat(200)}));
        assert!(long.chars().count() < 90);
    }

    #[test]
    fn clip_respects_character_boundaries() {
        assert_eq!(clip("नमस्ते दुनिया", 4).chars().count(), 4);
        assert_eq!(clip("short", 10), "short");
    }
}
