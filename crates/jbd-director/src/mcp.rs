//! Connectors: a small MCP (Model Context Protocol) client, so the Director can use tools from
//! other programs (a Notion server, a file server, a stock-footage search…), the same servers
//! Claude Desktop runs.
//!
//! What is here:
//!
//! - [`ConnectorConfig`], [`load_config`], [`save_config`]: the connectors file. It has Claude
//!   Desktop's shape, so a person can paste their `mcpServers` block straight in:
//!   `{"mcpServers": {"notion": {"command": "npx", "args": ["-y", "@notionhq/notion-mcp-server"],
//!   "env": {"NOTION_TOKEN": "…"}, "enabled": true, "trusted": false}}}`.
//! - [`McpClient`]: one running server. It starts the program, speaks newline-delimited
//!   JSON-RPC 2.0 over its stdin/stdout (`initialize`, `tools/list`, `tools/call`) and kills it
//!   when dropped.
//! - [`Connectors`]: every enabled server, started in parallel, with their tools renamed to
//!   names the model's tool API accepts (`mcp__notion__search`) and mapped back on a call.
//!
//! How a call travels:
//!
//! ```text
//!  call_tool ──► request(): id N, a channel waits in `pending[N]`
//!       │             │ bytes
//!       │             ▼
//!       │        writer thread ──► server stdin
//!       │
//!       └─ recv_timeout ◄── reader thread ◄── server stdout   (routes the reply with id N)
//!                                     stderr ──► stderr thread (keeps the last 4 KB for errors)
//! ```
//!
//! Writing happens on its own thread so a server that stops reading can never freeze the
//! Director: the timeout still fires. Everything a server sends is treated as hostile: lines
//! are capped at 4 MB, tool lists at 100, texts are clipped, and every wait has a timeout.
//!
//! Not supported (yet): the HTTP/SSE transports, resources, prompts, sampling, roots and
//! progress notifications. A server that asks JBD for one of those gets a JSON-RPC
//! "not supported" error back; `ping` is answered.
//!
//! Common changes:
//! - Limits (message size, tool count, timeouts): the constants below and [`Timeouts`].
//! - How tool results become text for the model: [`content_text`].
//! - How a tool is named for the model: [`openai_tool_name`].
//! - How a program is found on Windows (`npx` → `npx.cmd`): `resolve_program`.
//!
//! Every call here blocks, so only call it from a worker thread, never from the UI.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::lock;
use crate::tools::clip;

/// The MCP revision JBD speaks. Servers answer with the one they speak; tools work the same in
/// every revision since 2024-11-05, so JBD accepts whatever comes back.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Largest single message JBD reads from a server. A bigger line is thrown away.
const MAX_MESSAGE: usize = 4 * 1024 * 1024;
/// Most tools one server may offer; the model's tool list has to stay small to stay useful.
const MAX_TOOLS: usize = 100;
/// `tools/list` pages to follow before giving up on a server that never stops paging.
const MAX_PAGES: usize = 20;
/// Bytes of the server's stderr kept for error messages.
const STDERR_KEEP: usize = 4096;
/// Of those, how many characters an error message shows.
const STDERR_SHOWN: usize = 600;
/// A tool result longer than this is clipped before it goes to the model.
const MAX_RESULT_CHARS: usize = 100_000;
const MAX_DESCRIPTION_CHARS: usize = 2_000;
/// A tool's input schema bigger than this is replaced by "any object".
const MAX_SCHEMA_BYTES: usize = 64 * 1024;
/// The connectors file is a few KB; refuse to read something huge.
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
/// OpenAI-style function names: `^[A-Za-z0-9_-]{1,64}$`.
const MAX_NAME: usize = 64;

/// Windows: start console programs without flashing a console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// ---------------------------------------------------------------------------------------------
// The connectors file
// ---------------------------------------------------------------------------------------------

/// One server the person has added.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorConfig {
    /// The key in `mcpServers`; also the middle of the tool names the model sees.
    pub name: String,
    /// The program, e.g. `npx`, `node`, `uvx` or a full path to an `.exe`.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment variables (API keys usually live here).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Trusted connectors' tools run without asking first.
    #[serde(default)]
    pub trusted: bool,
}

fn yes() -> bool {
    true
}

/// One `mcpServers` entry as the file stores it (the name is the key, not a field).
#[derive(Serialize, Deserialize)]
struct FileEntry {
    #[serde(default)]
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default = "yes")]
    enabled: bool,
    #[serde(default)]
    trusted: bool,
}

/// Read the connectors file. A missing or unreadable file means "no connectors": the Director
/// still works without them. Entries without a `command` (HTTP servers) are skipped.
pub fn load_config(path: &Path) -> Vec<ConnectorConfig> {
    let Ok(file) = std::fs::File::open(path) else { return Vec::new() };
    let mut text = String::new();
    if file.take(MAX_CONFIG_BYTES).read_to_string(&mut text).is_err() {
        return Vec::new();
    }
    parse_config(&text)
}

fn parse_config(text: &str) -> Vec<ConnectorConfig> {
    let Ok(root) = serde_json::from_str::<Value>(text) else { return Vec::new() };
    let Some(servers) = root.get("mcpServers").and_then(Value::as_object) else { return Vec::new() };
    servers
        .iter()
        .filter_map(|(name, v)| {
            // One bad entry must not hide the others.
            let e: FileEntry = serde_json::from_value(v.clone()).ok()?;
            if e.command.trim().is_empty() {
                return None;
            }
            Some(ConnectorConfig { name: name.clone(), command: e.command, args: e.args, env: e.env, enabled: e.enabled, trusted: e.trusted })
        })
        .collect()
}

/// Write the connectors file. Other top-level keys already in the file are kept; the
/// `mcpServers` block is replaced by `configs`. Written to a temporary file first so a crash
/// mid-write never leaves half a file.
pub fn save_config(path: &Path, configs: &[ConnectorConfig]) -> Result<(), String> {
    let mut root = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| match v {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default();
    let mut servers = Map::new();
    for c in configs {
        let entry = FileEntry { command: c.command.clone(), args: c.args.clone(), env: c.env.clone(), enabled: c.enabled, trusted: c.trusted };
        let v = serde_json::to_value(entry).map_err(|e| format!("couldn't save connector {}: {e}", c.name))?;
        servers.insert(c.name.clone(), v);
    }
    root.insert("mcpServers".into(), Value::Object(servers));
    let text = serde_json::to_string_pretty(&Value::Object(root)).map_err(|e| format!("couldn't save the connectors: {e}"))?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("couldn't write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("couldn't write {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------------------------
// Tool names
// ---------------------------------------------------------------------------------------------

/// One tool a connector offers.
#[derive(Clone, Debug, PartialEq)]
pub struct McpTool {
    pub connector: String,
    /// The server's own name for it (what `tools/call` is sent).
    pub name: String,
    pub description: String,
    /// JSON Schema of the arguments (always an object).
    pub input_schema: Value,
    /// The name the model sees, unique across all connectors (see [`openai_tool_name`]).
    pub openai_name: String,
}

impl McpTool {
    /// The tool in the shape the chat API takes (`{"type": "function", "function": {…}}`).
    pub fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {"name": self.openai_name, "description": self.description, "parameters": self.input_schema},
        })
    }
}

/// `mcp__<connector>__<tool>`, made valid for OpenAI-style tool APIs: only `[A-Za-z0-9_-]`
/// (anything else becomes `_`) and at most 64 characters. A longer name is cut and ends in a
/// short hash of the original names, so two long names that start alike stay different.
pub fn openai_tool_name(connector: &str, tool: &str) -> String {
    let full = format!("mcp__{}__{}", sanitize(connector), sanitize(tool));
    if full.len() <= MAX_NAME { full } else { with_hash(&full, connector, tool) }
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

/// `base` cut to fit, then `_` and 8 hex digits of a hash of the original names.
fn with_hash(base: &str, connector: &str, tool: &str) -> String {
    let keep: String = base.chars().take(MAX_NAME.saturating_sub(9)).collect();
    format!("{keep}_{:08x}", fnv1a(&[connector.as_bytes(), &[0], tool.as_bytes()]))
}

/// FNV-1a: tiny, and the same on every machine and Rust version (std's hasher is neither
/// promised), so a tool keeps its name between runs.
fn fnv1a(parts: &[&[u8]]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in parts.iter().flat_map(|p| p.iter()) {
        h ^= u32::from(*b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// A name for this tool that nothing in `taken` has yet. Two tools can sanitise to the same
/// name (`a.b` and `a_b`); the second gets the hashed form. `None` if even that is taken.
fn unique_name(taken: &HashMap<String, (usize, String)>, connector: &str, tool: &str) -> Option<String> {
    let name = openai_tool_name(connector, tool);
    if !taken.contains_key(&name) {
        return Some(name);
    }
    let hashed = with_hash(&name, connector, tool);
    (!taken.contains_key(&hashed)).then_some(hashed)
}

// ---------------------------------------------------------------------------------------------
// One server
// ---------------------------------------------------------------------------------------------

/// How long JBD waits for a server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    /// For `initialize` (an `npx` server may download itself the first time).
    pub init: Duration,
    /// For every other request.
    pub call: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts { init: Duration::from_secs(20), call: Duration::from_secs(60) }
    }
}

/// Why a request got no result.
enum Failure {
    /// The server answered with a JSON-RPC error.
    Rpc(String),
    /// The connection is gone (the reason, without the stderr tail).
    Closed(String),
}

type Answer = Result<Value, Failure>;

/// What the reader thread and the callers share.
#[derive(Default)]
struct State {
    /// Requests waiting for their answer, by id.
    pending: HashMap<u64, Sender<Answer>>,
    /// Set once the server's output has ended; no request can be added after that.
    closed: Option<String>,
}

/// The end of the server's stderr.
#[derive(Default)]
struct Tail {
    bytes: Vec<u8>,
    /// The stream has ended (so the last words are all in).
    done: bool,
}

/// The outgoing side: `None` once the client is dropped, which ends the writer thread and so
/// closes the server's stdin.
type Outbox = Arc<Mutex<Option<Sender<Vec<u8>>>>>;

/// One running MCP server.
pub struct McpClient {
    name: String,
    out: Outbox,
    state: Arc<Mutex<State>>,
    next_id: AtomicU64,
    timeouts: Timeouts,
    stderr: Arc<Mutex<Tail>>,
    child: Option<Child>,
    server: String,
}

impl McpClient {
    /// Start the server program and do the `initialize` handshake.
    pub fn spawn(cfg: &ConnectorConfig, timeouts: Timeouts) -> Result<McpClient, String> {
        if cfg.command.trim().is_empty() {
            return Err(format!("Connector \"{}\" has no command. Add one in Connectors.", cfg.name));
        }
        let program = resolve_program(cfg.command.trim());
        let mut cmd = Command::new(&program);
        // Arguments go to the program one by one, never through a shell string, so nothing in
        // them is interpreted. (Rust quotes them safely for `.cmd` files too, or refuses.)
        cmd.args(&cfg.args).envs(&cfg.env).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().map_err(|e| {
            format!("Couldn't start connector \"{}\" ({}): {e}. Check that it is installed and the command is right.", cfg.name, program.display())
        })?;
        let (Some(stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("Couldn't talk to connector \"{}\".", cfg.name));
        };
        let tail = Arc::new(Mutex::new(Tail::default()));
        let t = tail.clone();
        if let Err(e) = std::thread::Builder::new().name(format!("mcp-stderr-{}", cfg.name)).spawn(move || drain_stderr(stderr, &t)) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("Couldn't start connector \"{}\": {e}", cfg.name));
        }
        // If the handshake fails, dropping the half-made client kills the child.
        Self::start(&cfg.name, stdout, stdin, timeouts, tail, Some(child))
    }

    /// Talk MCP over any pair of streams (the tests use in-memory pipes). Does the handshake.
    pub fn connect<R, W>(name: &str, reader: R, writer: W, timeouts: Timeouts) -> Result<McpClient, String>
    where
        R: Read + Send + 'static,
        W: Write + Send + 'static,
    {
        Self::start(name, reader, writer, timeouts, Arc::new(Mutex::new(Tail::default())), None)
    }

    fn start<R, W>(name: &str, reader: R, writer: W, timeouts: Timeouts, stderr: Arc<Mutex<Tail>>, child: Option<Child>) -> Result<McpClient, String>
    where
        R: Read + Send + 'static,
        W: Write + Send + 'static,
    {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let mut client = McpClient {
            name: name.to_string(),
            out: Arc::new(Mutex::new(Some(tx))),
            state: Arc::new(Mutex::new(State::default())),
            next_id: AtomicU64::new(1),
            timeouts,
            stderr,
            child,
            server: String::new(),
        };
        std::thread::Builder::new()
            .name(format!("mcp-write-{name}"))
            .spawn(move || write_loop(writer, &rx))
            .map_err(|e| format!("Couldn't start connector \"{name}\": {e}"))?;
        let (state, out) = (client.state.clone(), client.out.clone());
        std::thread::Builder::new()
            .name(format!("mcp-read-{name}"))
            .spawn(move || read_loop(reader, &state, &out))
            .map_err(|e| format!("Couldn't start connector \"{name}\": {e}"))?;
        client.initialize()?;
        Ok(client)
    }

    /// The connector's name (the key in the file).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// What the server called itself in `initialize` ("notion-mcp 1.2.0"), for the UI.
    pub fn server_info(&self) -> &str {
        &self.server
    }

    /// The last ~4 KB the server wrote to stderr.
    pub fn stderr_tail(&self) -> String {
        String::from_utf8_lossy(&lock(&self.stderr).bytes).trim().to_string()
    }

    fn initialize(&mut self) -> Result<(), String> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "JBD", "version": env!("CARGO_PKG_VERSION")},
        });
        let result = self.request("initialize", params, self.timeouts.init)?;
        if result.get("protocolVersion").and_then(Value::as_str).is_none() {
            return Err(format!("Connector \"{}\" didn't answer like an MCP server. Check its command.", self.name));
        }
        let info = result.get("serverInfo");
        let field = |k: &str| info.and_then(|i| i.get(k)).and_then(Value::as_str).map(|s| clip(s, 80)).unwrap_or_default();
        self.server = format!("{} {}", field("name"), field("version")).trim().to_string();
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
    }

    /// Every tool the server offers (up to 100), following `nextCursor` pages.
    pub fn list_tools(&self) -> Result<Vec<McpTool>, String> {
        let mut tools = Vec::new();
        let mut seen_names = HashSet::new();
        let mut cursor: Option<String> = None;
        let mut seen_cursors = HashSet::new();
        for _ in 0..MAX_PAGES {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let page = self.request("tools/list", params, self.timeouts.call)?;
            for t in page.get("tools").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default() {
                if tools.len() >= MAX_TOOLS {
                    return Ok(tools);
                }
                if let Some(tool) = self.parse_tool(t)
                    && seen_names.insert(tool.name.clone())
                {
                    tools.push(tool);
                }
            }
            // A server that repeats a cursor would page forever.
            match page.get("nextCursor").and_then(Value::as_str) {
                Some(c) if !c.is_empty() && seen_cursors.insert(c.to_string()) => cursor = Some(c.to_string()),
                _ => break,
            }
        }
        Ok(tools)
    }

    /// One `tools` entry, or `None` when it is not something JBD can offer the model.
    fn parse_tool(&self, t: &Value) -> Option<McpTool> {
        let name = t.get("name").and_then(Value::as_str)?;
        if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
            return None;
        }
        let description = t.get("description").and_then(Value::as_str).or_else(|| t.get("title").and_then(Value::as_str)).unwrap_or("");
        let schema = match t.get("inputSchema") {
            Some(s @ Value::Object(_)) if s.to_string().len() <= MAX_SCHEMA_BYTES => s.clone(),
            // Missing, odd or huge: "any object" still lets the model call it.
            _ => json!({"type": "object", "properties": {}}),
        };
        Some(McpTool {
            connector: self.name.clone(),
            name: name.to_string(),
            description: clip(description, MAX_DESCRIPTION_CHARS),
            input_schema: schema,
            openai_name: openai_tool_name(&self.name, name),
        })
    }

    /// Run a tool. Its text comes back as `Ok`; a tool that reports `isError` comes back as
    /// `Err` with its text, so the model can read what went wrong.
    pub fn call_tool(&self, tool: &str, arguments: Value) -> Result<String, String> {
        let arguments = if arguments.is_object() { arguments } else { json!({}) };
        let result = self.request("tools/call", json!({"name": tool, "arguments": arguments}), self.timeouts.call)?;
        let text = content_text(&result);
        if result.get("isError").and_then(Value::as_bool).unwrap_or(false) {
            Err(if text.is_empty() { format!("{tool} failed") } else { text })
        } else {
            Ok(text)
        }
    }

    /// Send a request and wait for its answer.
    fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        {
            // Checked under the same lock the reader closes with, so a request can't slip in
            // after the server died and wait for nothing.
            let mut st = lock(&self.state);
            if let Some(reason) = &st.closed {
                let reason = reason.clone();
                drop(st);
                return Err(self.stopped(&reason));
            }
            st.pending.insert(id, tx);
        }
        if let Err(e) = self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})) {
            lock(&self.state).pending.remove(&id);
            return Err(e);
        }
        match rx.recv_timeout(timeout) {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(Failure::Rpc(message))) => Err(format!("{} said: {message}", self.name)),
            Ok(Err(Failure::Closed(reason))) => Err(self.stopped(&reason)),
            Err(RecvTimeoutError::Timeout) => {
                lock(&self.state).pending.remove(&id);
                // Tell the server to stop working on it (never for initialize, per the spec).
                if method != "initialize" {
                    let _ = self.send(&json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": id, "reason": "timed out"}}));
                }
                Err(format!(
                    "Connector \"{}\" didn't answer {method} within {} s. It may be stuck; try again, or turn it off in Connectors.",
                    self.name,
                    timeout.as_secs_f32().round()
                ))
            }
            Err(RecvTimeoutError::Disconnected) => Err(self.stopped("it closed the connection")),
        }
    }

    /// Queue one message for the writer thread.
    fn send(&self, msg: &Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(msg).map_err(|e| format!("couldn't encode a message for {}: {e}", self.name))?;
        bytes.push(b'\n');
        let out = lock(&self.out);
        match out.as_ref().map(|tx| tx.send(bytes)) {
            Some(Ok(())) => Ok(()),
            _ => Err(self.stopped("it stopped reading")),
        }
    }

    /// The error for a server that has gone, with the last thing it printed (usually the why).
    fn stopped(&self, reason: &str) -> String {
        if self.child.is_some() {
            // The stderr thread may still be reading the last words; give it a moment.
            for _ in 0..10 {
                if lock(&self.stderr).done {
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        let tail = self.stderr_tail();
        let count = tail.chars().count();
        let tail: String = tail.chars().skip(count.saturating_sub(STDERR_SHOWN)).collect();
        if tail.is_empty() {
            format!("Connector \"{}\" stopped ({reason}). Check its command and settings in Connectors.", self.name)
        } else {
            format!("Connector \"{}\" stopped ({reason}). It said: {tail}", self.name)
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        // Close stdin first (a well-behaved server exits on that), then make sure.
        lock(&self.out).take();
        if let Some(mut child) = self.child.take() {
            // `npx` runs through `cmd.exe`; killing it closes our pipes, and the node process
            // under it exits when its stdin closes.
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The tool result as text for the model: text parts joined, other parts as a placeholder.
fn content_text(result: &Value) -> String {
    let short = |v: Option<&Value>| clip(v.and_then(Value::as_str).unwrap_or(""), 200);
    let mut parts: Vec<String> = Vec::new();
    for item in result.get("content").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default() {
        let part = match item.get("type").and_then(Value::as_str).unwrap_or("") {
            "text" => item.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
            "image" => "[image]".to_string(),
            "audio" => "[audio]".to_string(),
            "resource" => format!("[resource: {}]", short(item.get("resource").and_then(|r| r.get("uri")))),
            "resource_link" => format!("[link: {}]", short(item.get("uri"))),
            other => format!("[{} content]", clip(other, 40)),
        };
        parts.push(part);
        // Stop collecting once there is more than the model will get anyway.
        if parts.iter().map(String::len).sum::<usize>() > MAX_RESULT_CHARS.saturating_mul(4) {
            break;
        }
    }
    let text = if parts.is_empty() {
        // Some servers only send `structuredContent`.
        result.get("structuredContent").map(Value::to_string).unwrap_or_default()
    } else {
        parts.join("\n")
    };
    clip(&text, MAX_RESULT_CHARS)
}

/// Windows can't start `npx` by that name: it is `npx.cmd`. Find the program on PATH the way a
/// shell would (`.exe`, `.cmd`, `.bat`), without running a shell. Elsewhere the name is used as is.
fn resolve_program(command: &str) -> PathBuf {
    let given = PathBuf::from(command);
    if !cfg!(windows) {
        return given;
    }
    const EXTS: [&str; 3] = ["exe", "cmd", "bat"];
    let candidates = |base: &Path| -> Vec<PathBuf> {
        if base.extension().is_some() {
            // `node.exe`, `server.cmd`; but also `mcp-server.v2`, so try the extensions too.
            let mut v = vec![base.to_path_buf()];
            v.extend(EXTS.iter().map(|e| PathBuf::from(format!("{}.{e}", base.display()))));
            v
        } else {
            EXTS.iter().map(|e| base.with_extension(e)).collect()
        }
    };
    if given.components().count() > 1 || given.is_absolute() {
        // A path: only the extension may be missing.
        return candidates(&given).into_iter().find(|p| p.is_file()).unwrap_or(given);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        if let Some(found) = candidates(&dir.join(&given)).into_iter().find(|p| p.is_file()) {
            return found;
        }
    }
    given
}

/// The writer thread: send queued messages until the client goes or the pipe breaks.
fn write_loop(mut w: impl Write, rx: &mpsc::Receiver<Vec<u8>>) {
    for msg in rx {
        if w.write_all(&msg).and_then(|()| w.flush()).is_err() {
            break;
        }
    }
}

/// The stderr thread: keep the last few KB, so errors can say what the server said.
fn drain_stderr(mut r: impl Read, tail: &Mutex<Tail>) {
    let mut buf = [0u8; 1024];
    loop {
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let mut t = lock(tail);
                t.bytes.extend_from_slice(buf.get(..n).unwrap_or_default());
                let extra = t.bytes.len().saturating_sub(STDERR_KEEP);
                if extra > 0 {
                    t.bytes.drain(..extra);
                }
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    lock(tail).done = true;
}

/// What one read from the server produced.
#[derive(Debug, PartialEq, Eq)]
enum Frame {
    /// A whole line is in the buffer (without its newline).
    Line,
    /// A line over [`MAX_MESSAGE`] went by and was thrown away.
    TooLong,
    Eof,
}

/// Read one newline-terminated message into `out`, never holding more than [`MAX_MESSAGE`].
fn read_frame(r: &mut impl BufRead, out: &mut Vec<u8>) -> std::io::Result<Frame> {
    out.clear();
    let mut too_long = false;
    loop {
        let buf = match r.fill_buf() {
            Ok(b) => b,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if buf.is_empty() {
            // End of stream: a last line without a newline still counts.
            return Ok(if too_long {
                Frame::TooLong
            } else if out.is_empty() {
                Frame::Eof
            } else {
                Frame::Line
            });
        }
        let newline = buf.iter().position(|&b| b == b'\n');
        let take = newline.unwrap_or(buf.len());
        if !too_long {
            if out.len().saturating_add(take) > MAX_MESSAGE {
                too_long = true;
                out.clear();
            } else {
                out.extend_from_slice(buf.get(..take).unwrap_or_default());
            }
        }
        match newline {
            Some(i) => {
                r.consume(i.saturating_add(1));
                return Ok(if too_long { Frame::TooLong } else { Frame::Line });
            }
            None => r.consume(take),
        }
    }
}

/// The reader thread: route each answer to the request waiting for it.
fn read_loop(r: impl Read, state: &Mutex<State>, out: &Outbox) {
    let mut r = BufReader::new(r);
    let mut line = Vec::new();
    let reason = loop {
        match read_frame(&mut r, &mut line) {
            Ok(Frame::Line) => handle_message(&line, state, out),
            Ok(Frame::TooLong) => {
                // The id is in the part we threw away, so whoever is waiting can't be told
                // precisely; failing everyone waiting beats making them sit out the timeout.
                let mut st = lock(state);
                for (_, tx) in st.pending.drain() {
                    let _ = tx.send(Err(Failure::Rpc("it sent a message over 4 MB, which JBD refuses".into())));
                }
            }
            Ok(Frame::Eof) => break "it closed its output".to_string(),
            Err(e) => break format!("reading from it failed: {e}"),
        }
    };
    let mut st = lock(state);
    st.closed = Some(reason.clone());
    for (_, tx) in st.pending.drain() {
        let _ = tx.send(Err(Failure::Closed(reason.clone())));
    }
}

/// One message from the server: an answer, a notification, or a request of its own.
fn handle_message(line: &[u8], state: &Mutex<State>, out: &Outbox) {
    // Anything that isn't a JSON object (garbage, a log line on stdout, a batch) is ignored.
    let Ok(Value::Object(msg)) = serde_json::from_slice::<Value>(line) else { return };
    let id = msg.get("id").filter(|id| !id.is_null());
    if let Some(method) = msg.get("method").and_then(Value::as_str) {
        // The server asks JBD something. Notifications (no id) need no answer.
        let Some(id) = id else { return };
        let reply = if method == "ping" {
            json!({"jsonrpc": "2.0", "id": id, "result": {}})
        } else {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("not supported by JBD: {}", clip(method, 80))}})
        };
        if let Ok(mut bytes) = serde_json::to_vec(&reply) {
            bytes.push(b'\n');
            if let Some(tx) = lock(out).as_ref() {
                let _ = tx.send(bytes);
            }
        }
        return;
    }
    let Some(id) = id.and_then(Value::as_u64) else { return };
    let Some(tx) = lock(state).pending.remove(&id) else { return };
    let answer = match msg.get("error") {
        Some(err) => {
            let message = err.get("message").and_then(Value::as_str).unwrap_or("unknown error");
            let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
            Err(Failure::Rpc(format!("{} (code {code})", clip(message, 2_000))))
        }
        None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
    };
    let _ = tx.send(answer);
}

// ---------------------------------------------------------------------------------------------
// All servers
// ---------------------------------------------------------------------------------------------

struct Server {
    name: String,
    trusted: bool,
    client: Option<McpClient>,
    status: Result<usize, String>,
}

/// Every enabled connector, running, with their tools under model-safe names.
#[derive(Default)]
pub struct Connectors {
    servers: Vec<Server>,
    tools: Vec<McpTool>,
    /// Model name → (index into `servers`, the server's own tool name).
    by_name: HashMap<String, (usize, String)>,
}

impl Connectors {
    /// Start every enabled connector, each on its own thread so a slow one doesn't hold up the
    /// rest. One that fails is reported in [`Connectors::status`]; the others still work.
    /// Takes at most about `init + call` timeouts, however many there are.
    pub fn start(configs: &[ConnectorConfig]) -> Connectors {
        Self::start_with(configs, Timeouts::default())
    }

    pub fn start_with(configs: &[ConnectorConfig], timeouts: Timeouts) -> Connectors {
        let mut names = HashSet::new();
        let mut jobs = Vec::new();
        for cfg in configs.iter().filter(|c| c.enabled) {
            if !names.insert(cfg.name.clone()) {
                continue;
            }
            let c = cfg.clone();
            let job = std::thread::Builder::new().name(format!("mcp-start-{}", cfg.name)).spawn(move || -> Result<(McpClient, Vec<McpTool>), String> {
                let client = McpClient::spawn(&c, timeouts)?;
                let tools = client.list_tools()?;
                Ok((client, tools))
            });
            jobs.push((cfg.name.clone(), cfg.trusted, job));
        }
        let started = jobs
            .into_iter()
            .map(|(name, trusted, job)| {
                let result = match job {
                    // `join` is Err only if the thread panicked; it must not take JBD with it.
                    Ok(handle) => handle.join().unwrap_or_else(|_| Err(format!("Connector \"{name}\" crashed while starting."))),
                    Err(e) => Err(format!("Couldn't start connector \"{name}\": {e}")),
                };
                (name, trusted, result)
            })
            .collect();
        Self::assemble(started)
    }

    /// Put started servers together and give every tool its unique model name.
    fn assemble(started: Vec<(String, bool, Result<(McpClient, Vec<McpTool>), String>)>) -> Connectors {
        let mut me = Connectors::default();
        for (name, trusted, result) in started {
            let index = me.servers.len();
            match result {
                Ok((client, tools)) => {
                    let mut count = 0;
                    for mut tool in tools {
                        let Some(openai) = unique_name(&me.by_name, &name, &tool.name) else { continue };
                        me.by_name.insert(openai.clone(), (index, tool.name.clone()));
                        tool.openai_name = openai;
                        me.tools.push(tool);
                        count += 1;
                    }
                    me.servers.push(Server { name, trusted, client: Some(client), status: Ok(count) });
                }
                Err(e) => me.servers.push(Server { name, trusted, client: None, status: Err(e) }),
            }
        }
        me
    }

    /// Every tool from every running connector.
    pub fn tools(&self) -> Vec<McpTool> {
        self.tools.clone()
    }

    /// Run a tool by the name the model used (`mcp__notion__search`).
    pub fn call(&self, openai_name: &str, args: Value) -> Result<String, String> {
        let (index, tool) = self.by_name.get(openai_name).ok_or_else(|| format!("There is no connector tool called {}.", clip(openai_name, 80)))?;
        let server = self.servers.get(*index).ok_or_else(|| format!("There is no connector tool called {}.", clip(openai_name, 80)))?;
        let client = server.client.as_ref().ok_or_else(|| format!("Connector \"{}\" isn't running.", server.name))?;
        client.call_tool(tool, args)
    }

    /// Whether this tool's connector is trusted to run without asking. Unknown names are not.
    pub fn is_trusted(&self, openai_name: &str) -> bool {
        self.by_name.get(openai_name).and_then(|(i, _)| self.servers.get(*i)).is_some_and(|s| s.trusted)
    }

    /// Each enabled connector: how many tools it offers, or why it isn't running.
    pub fn status(&self) -> Vec<(String, Result<usize, String>)> {
        self.servers.iter().map(|s| (s.name.clone(), s.status.clone())).collect()
    }
}

/// How many connector tools the Director is offered at most (each one costs context, and the
/// gateway accepts a limited number of tools per call).
pub const MAX_OFFERED: usize = 30;

impl crate::ExternalTools for Connectors {
    fn definitions(&self) -> Vec<Value> {
        self.tools().iter().take(MAX_OFFERED).map(McpTool::definition).collect()
    }
    fn call(&self, name: &str, args: Value) -> Result<String, String> {
        Connectors::call(self, name, args)
    }
    fn is_trusted(&self, name: &str) -> bool {
        Connectors::is_trusted(self, name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Receiver;

    // ---- in-memory pipes -----------------------------------------------------------------

    struct PipeWriter(Sender<Vec<u8>>);

    impl Write for PipeWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.send(buf.to_vec()).map_err(|_| std::io::Error::from(ErrorKind::BrokenPipe))?;
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct PipeReader {
        rx: Receiver<Vec<u8>>,
        buf: Vec<u8>,
        pos: usize,
    }

    impl Read for PipeReader {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            while self.pos >= self.buf.len() {
                match self.rx.recv() {
                    Ok(b) => {
                        self.buf = b;
                        self.pos = 0;
                    }
                    Err(_) => return Ok(0),
                }
            }
            let n = out.len().min(self.buf.len() - self.pos);
            out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    fn pipe() -> (PipeWriter, PipeReader) {
        let (tx, rx) = mpsc::channel();
        (PipeWriter(tx), PipeReader { rx, buf: Vec::new(), pos: 0 })
    }

    // ---- a fake MCP server ---------------------------------------------------------------

    /// What the fake server saw, for the asserts.
    #[derive(Default, Debug)]
    struct Seen {
        initialized: bool,
        cancelled: Vec<u64>,
        /// The client's answer to the server's own request.
        reply_to_server: Option<Value>,
        protocol: String,
    }

    fn send(w: &mut PipeWriter, v: Value) {
        let mut b = serde_json::to_vec(&v).unwrap();
        b.push(b'\n');
        w.write_all(&b).unwrap();
    }

    fn fake_server(input: PipeReader, mut output: PipeWriter, seen: Arc<Mutex<Seen>>) {
        let reader = BufReader::new(input);
        for line in reader.lines() {
            let Ok(line) = line else { return };
            let msg: Value = serde_json::from_str(&line).unwrap();
            let id = msg.get("id").cloned().unwrap_or(Value::Null);
            match msg.get("method").and_then(Value::as_str) {
                Some("initialize") => {
                    lock(&seen).protocol = msg["params"]["protocolVersion"].as_str().unwrap().to_string();
                    // Noise before the answer: a log line, a notification and a request of
                    // the server's own, which JBD must refuse politely.
                    output.write_all(b"not json at all\n").unwrap();
                    send(&mut output, json!({"jsonrpc": "2.0", "method": "notifications/message", "params": {"level": "info"}}));
                    send(&mut output, json!({"jsonrpc": "2.0", "id": "srv-1", "method": "sampling/createMessage", "params": {}}));
                    send(
                        &mut output,
                        json!({"jsonrpc": "2.0", "id": id, "result": {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "fake", "version": "1.0"}}}),
                    );
                }
                Some("notifications/initialized") => lock(&seen).initialized = true,
                Some("notifications/cancelled") => lock(&seen).cancelled.push(msg["params"]["requestId"].as_u64().unwrap()),
                Some("tools/list") => {
                    let result = match msg["params"].get("cursor").and_then(Value::as_str) {
                        None => json!({"tools": [
                            {"name": "echo", "description": "Says it back", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}},
                            {"name": "fail"},
                            {"name": ""},
                        ], "nextCursor": "page-2"}),
                        Some("page-2") => json!({"tools": [{"name": "slow"}, {"name": "huge"}, {"name": "die"}, {"name": "echo"}]}),
                        Some(other) => panic!("unexpected cursor {other}"),
                    };
                    send(&mut output, json!({"jsonrpc": "2.0", "id": id, "result": result}));
                }
                Some("tools/call") => match msg["params"]["name"].as_str().unwrap() {
                    "echo" => send(
                        &mut output,
                        json!({"jsonrpc": "2.0", "id": id, "result": {"content": [
                            {"type": "text", "text": msg["params"]["arguments"]["text"]},
                            {"type": "image", "data": "AAAA", "mimeType": "image/png"},
                        ]}}),
                    ),
                    "fail" => {
                        send(&mut output, json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": "boom"}], "isError": true}}))
                    }
                    "slow" => {} // never answers
                    "huge" => {
                        let big = "x".repeat(MAX_MESSAGE + 10);
                        send(&mut output, json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": big}]}}));
                    }
                    "die" => return,
                    other => send(&mut output, json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": format!("no tool {other}")}})),
                },
                // The client's answer to "srv-1".
                None => lock(&seen).reply_to_server = Some(msg),
                Some(other) => panic!("unexpected method {other}"),
            }
        }
    }

    fn start_fake(timeouts: Timeouts) -> (McpClient, Arc<Mutex<Seen>>) {
        let (to_server, server_in) = pipe();
        let (server_out, from_server) = pipe();
        let seen = Arc::new(Mutex::new(Seen::default()));
        let s = seen.clone();
        std::thread::spawn(move || fake_server(server_in, server_out, s));
        let client = McpClient::connect("fake", from_server, to_server, timeouts).unwrap();
        (client, seen)
    }

    fn quick() -> Timeouts {
        Timeouts { init: Duration::from_secs(5), call: Duration::from_millis(300) }
    }

    // ---- tests ---------------------------------------------------------------------------

    #[test]
    fn end_to_end_against_a_fake_server() {
        let (client, seen) = start_fake(quick());
        assert_eq!(client.server_info(), "fake 1.0");

        // Pagination followed; the nameless tool and the duplicate "echo" dropped.
        let tools = client.list_tools().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["echo", "fail", "slow", "huge", "die"]);
        assert_eq!(tools[0].openai_name, "mcp__fake__echo");
        assert_eq!(tools[0].input_schema["properties"]["text"]["type"], "string");
        // A tool without a schema still gets an object schema the chat API accepts.
        assert_eq!(tools[1].input_schema["type"], "object");

        {
            let s = lock(&seen);
            assert!(s.initialized);
            assert_eq!(s.protocol, PROTOCOL_VERSION);
            let reply = s.reply_to_server.as_ref().unwrap();
            assert_eq!(reply["id"], "srv-1");
            assert_eq!(reply["error"]["code"], -32601);
        }

        assert_eq!(client.call_tool("echo", json!({"text": "namaste"})).unwrap(), "namaste\n[image]");
        assert_eq!(client.call_tool("fail", json!({})).unwrap_err(), "boom");
        assert!(client.call_tool("nope", Value::Null).unwrap_err().contains("no tool nope"));

        // A timeout comes back as an error and tells the server to stop.
        let err = client.call_tool("slow", json!({})).unwrap_err();
        assert!(err.contains("didn't answer tools/call"), "{err}");
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(lock(&seen).cancelled.len(), 1);

        // An oversized message is refused, and the connection keeps working afterwards.
        let err = client.call_tool("huge", json!({})).unwrap_err();
        assert!(err.contains("4 MB"), "{err}");
        assert_eq!(client.call_tool("echo", json!({"text": "still here"})).unwrap(), "still here\n[image]");

        // The server going away fails the call and every later one, without waiting.
        let err = client.call_tool("die", json!({})).unwrap_err();
        assert!(err.contains("stopped"), "{err}");
        let started = std::time::Instant::now();
        assert!(client.call_tool("echo", json!({"text": "hi"})).is_err());
        assert!(started.elapsed() < Duration::from_millis(250));
    }

    #[test]
    fn connectors_route_calls_and_report_failures() {
        let (client, _seen) = start_fake(quick());
        let tools = client.list_tools().unwrap();
        let c =
            Connectors::assemble(vec![("fake".into(), true, Ok((client, tools))), ("broken".into(), false, Err("Couldn't start connector \"broken\"".into()))]);
        assert_eq!(c.tools().len(), 5);
        assert_eq!(c.call("mcp__fake__echo", json!({"text": "hello"})).unwrap(), "hello\n[image]");
        assert!(c.call("mcp__fake__missing", json!({})).is_err());
        assert!(c.is_trusted("mcp__fake__echo"));
        assert!(!c.is_trusted("mcp__broken__x"));
        let status = c.status();
        assert_eq!(status[0], ("fake".to_string(), Ok(5)));
        assert!(status[1].1.is_err());
        // The definitions are what the chat API takes.
        assert_eq!(c.tools()[0].definition()["function"]["name"], "mcp__fake__echo");
    }

    #[test]
    fn a_server_that_never_answers_times_out() {
        let (to_server, server_in) = pipe();
        let (server_out, from_server) = pipe();
        // Reads everything, answers nothing.
        std::thread::spawn(move || {
            let _keep = server_out;
            for _ in BufReader::new(server_in).lines() {}
        });
        let started = std::time::Instant::now();
        let err =
            McpClient::connect("mute", from_server, to_server, Timeouts { init: Duration::from_millis(200), call: Duration::from_millis(200) }).err().unwrap();
        assert!(err.contains("didn't answer initialize"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn spawning_a_missing_program_is_an_error_not_a_crash() {
        let cfg = ConnectorConfig {
            name: "ghost".into(),
            command: "jbd-no-such-program-xyz".into(),
            args: vec![],
            env: BTreeMap::new(),
            enabled: true,
            trusted: false,
        };
        let err = McpClient::spawn(&cfg, quick()).err().unwrap();
        assert!(err.contains("ghost"), "{err}");
        let c = Connectors::start(&[cfg]);
        assert!(c.status()[0].1.is_err());
        assert!(c.tools().is_empty());
    }

    #[test]
    fn config_round_trips_and_reads_claude_desktop_files() {
        let text = r#"{
            "globalShortcut": "Ctrl+Space",
            "mcpServers": {
                "notion": {"command": "npx", "args": ["-y", "@notionhq/notion-mcp-server"], "env": {"NOTION_TOKEN": "t"}, "somethingNew": 1},
                "files": {"command": "node", "args": ["server.js"], "enabled": false, "trusted": true},
                "remote": {"type": "http", "url": "https://example.com/mcp"},
                "broken": {"command": 42}
            }
        }"#;
        let configs = parse_config(text);
        assert_eq!(configs.len(), 2);
        let files = &configs[0];
        assert_eq!((files.name.as_str(), files.enabled, files.trusted), ("files", false, true));
        let notion = &configs[1];
        assert_eq!(notion.name, "notion");
        assert!(notion.enabled && !notion.trusted);
        assert_eq!(notion.env.get("NOTION_TOKEN").map(String::as_str), Some("t"));

        let dir = std::env::temp_dir().join(format!("jbd-mcp-test-{}", std::process::id()));
        let path = dir.join("connectors.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, text).unwrap();
        save_config(&path, &configs).unwrap();
        assert_eq!(load_config(&path), configs);
        // Other settings in the file survive a save.
        let raw: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["globalShortcut"], "Ctrl+Space");
        let _ = std::fs::remove_dir_all(&dir);

        assert!(load_config(&dir.join("missing.json")).is_empty());
        assert!(parse_config("{not json").is_empty());
        // The camelCase serde shape, for anything that stores a config on its own.
        assert!(serde_json::to_string(&configs[0]).unwrap().contains("\"trusted\":true"));
    }

    #[test]
    fn tool_names_are_valid_short_and_unique() {
        assert_eq!(openai_tool_name("notion", "search"), "mcp__notion__search");
        assert_eq!(openai_tool_name("my server", "get.page/v2"), "mcp__my_server__get_page_v2");
        assert_eq!(openai_tool_name("हिंदी", "x"), format!("mcp__{}__x", "_".repeat(5)));

        let long = openai_tool_name("a-very-long-connector-name-indeed", "and_an_even_longer_tool_name_that_goes_on");
        assert_eq!(long.len(), 64);
        assert!(long.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
        // Two long names that only differ at the end stay different, and stable between runs.
        let other = openai_tool_name("a-very-long-connector-name-indeed", "and_an_even_longer_tool_name_that_goes_on_2");
        assert_ne!(long, other);
        assert_eq!(long, openai_tool_name("a-very-long-connector-name-indeed", "and_an_even_longer_tool_name_that_goes_on"));

        // `a.b` and `a_b` sanitise alike; the second gets a hashed name.
        let mut taken = HashMap::new();
        let first = unique_name(&taken, "s", "a.b").unwrap();
        taken.insert(first.clone(), (0, "a.b".to_string()));
        let second = unique_name(&taken, "s", "a_b").unwrap();
        assert_eq!(first, "mcp__s__a_b");
        assert_ne!(first, second);
        assert!(second.starts_with("mcp__s__a_b_") && second.len() <= 64);
    }

    #[test]
    fn content_becomes_text() {
        let r = json!({"content": [
            {"type": "text", "text": "one"},
            {"type": "resource", "resource": {"uri": "file:///a.txt", "text": "…"}},
            {"type": "resource_link", "uri": "https://x.test/y"},
            {"type": "audio"},
            {"type": "hologram"},
        ]});
        assert_eq!(content_text(&r), "one\n[resource: file:///a.txt]\n[link: https://x.test/y]\n[audio]\n[hologram content]");
        assert_eq!(content_text(&json!({"structuredContent": {"n": 1}})), "{\"n\":1}");
        assert_eq!(content_text(&json!({"content": "not a list"})), "");
        let long = json!({"content": [{"type": "text", "text": "y".repeat(MAX_RESULT_CHARS * 2)}]});
        assert_eq!(content_text(&long).chars().count(), MAX_RESULT_CHARS);
    }

    #[test]
    fn frames_are_capped() {
        let mut data = b"{\"a\":1}\r\n".to_vec();
        data.extend(std::iter::repeat_n(b'z', MAX_MESSAGE + 1));
        data.extend_from_slice(b"\n{\"b\":2}");
        let mut r = BufReader::with_capacity(64 * 1024, &data[..]);
        let mut out = Vec::new();
        assert_eq!(read_frame(&mut r, &mut out).unwrap(), Frame::Line);
        assert_eq!(out, b"{\"a\":1}\r");
        assert_eq!(read_frame(&mut r, &mut out).unwrap(), Frame::TooLong);
        assert_eq!(read_frame(&mut r, &mut out).unwrap(), Frame::Line);
        assert_eq!(out, b"{\"b\":2}");
        assert_eq!(read_frame(&mut r, &mut out).unwrap(), Frame::Eof);
    }
}
