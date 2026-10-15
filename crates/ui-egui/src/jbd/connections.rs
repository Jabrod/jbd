//! JBD ▸ Settings ▸ Connections and ▸ Connectors.
//!
//! - **Connections**: other AI apps (Claude Code, Claude Desktop, any MCP client) can drive JBD
//!   the way the Director does. "Let AI apps control JBD" starts the local control server
//!   (127.0.0.1 only, port [`PORT`]); JBD's MCP program (`jbd-mcp.exe` beside `jbd.exe`, or
//!   `filmcraft-cli.exe` in a developer build) bridges an AI app to it. The choice is saved in
//!   `<JBD data>/connections.json` and applied at the next launch too.
//! - **Connectors**: MCP servers the Director may use (Google Drive, Figma, Notion, stock
//!   libraries…), saved in `<JBD data>/connectors.json` in Claude Desktop's format, so a config
//!   can be pasted from their docs. The client is `jbd-director/src/mcp.rs`.

use egui::RichText;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::style;
use crate::FilmcraftApp;
use crate::theme::Tokens;

/// The control server's port (the MCP program's `--bridge 127.0.0.1:9876` points here).
pub const PORT: u16 = 9876;

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Prefs {
    #[serde(default)]
    allow_control: bool,
}

fn data_dir() -> Option<std::path::PathBuf> {
    filmcraft_engine::autosave::default_data_dir()
}

fn load_prefs() -> Prefs {
    data_dir().and_then(|d| std::fs::read_to_string(d.join("connections.json")).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_prefs(p: &Prefs) {
    if let (Some(d), Ok(text)) = (data_dir(), serde_json::to_string_pretty(p)) {
        let _ = std::fs::create_dir_all(&d);
        let _ = std::fs::write(d.join("connections.json"), text);
    }
}

/// Where the Director's connectors are configured.
pub fn connectors_path() -> Option<std::path::PathBuf> {
    data_dir().map(|d| d.join("connectors.json"))
}

/// What the Connections page shows about the control server.
#[derive(Default)]
pub struct State {
    checked: bool,
    pub status: Option<Result<(), String>>,
    /// The connector list being edited, loaded when the page opens.
    #[cfg(not(target_arch = "wasm32"))]
    connectors: Option<Vec<jbd_director::mcp::ConnectorConfig>>,
    new_name: String,
    new_command: String,
}

/// At launch: start the control server if the person allowed it last time.
pub fn autostart(app: &mut FilmcraftApp) {
    if app.jbd.connections.checked {
        return;
    }
    app.jbd.connections.checked = true;
    if load_prefs().allow_control && !app.has_control() {
        start(app);
    }
}

fn start(app: &mut FilmcraftApp) {
    let Some(hook) = app.hooks.start_control.as_mut() else {
        app.jbd.connections.status = Some(Err("This build can't start it (developer tools only).".into()));
        return;
    };
    match hook(PORT) {
        Ok(rx) => {
            app.set_control(rx);
            app.jbd.connections.status = Some(Ok(()));
        }
        Err(e) => app.jbd.connections.status = Some(Err(e)),
    }
}

/// JBD's MCP program: `jbd-mcp.exe` in an installed JBD, `filmcraft-cli.exe` in a developer build.
fn mcp_program() -> Option<std::path::PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    ["jbd-mcp.exe", "filmcraft-cli.exe", "jbd-mcp", "filmcraft-cli"].iter().map(|n| dir.join(n)).find(|p| p.exists())
}

/// Settings ▸ Connections.
pub fn connections(app: &mut FilmcraftApp, ui: &mut egui::Ui, ctx: &egui::Context, t: &Tokens) {
    let on = app.has_control();
    let mut allow = on || load_prefs().allow_control;
    let toggle = ui.checkbox(&mut allow, RichText::new("Let AI apps control JBD").color(t.text));
    app.auto.add("settings.connections.allow", toggle.rect, "Let AI apps control JBD");
    if toggle.changed() {
        save_prefs(&Prefs { allow_control: allow });
        if allow && !on {
            start(app);
        }
    }
    ui.label(
        RichText::new(
            "Claude Code, Claude Desktop or any MCP app can then edit in JBD with the same commands the Director uses. Only apps on this computer can connect.",
        )
        .size(12.0)
        .color(t.text_dim),
    );
    let status = match (&app.jbd.connections.status, on) {
        (_, true) => (format!("On: listening on 127.0.0.1:{PORT}"), t.text),
        (Some(Err(e)), _) => (format!("Couldn't start: {e}"), t.danger),
        _ if !allow => ("Off".to_string(), t.text_faint),
        _ => ("Off until JBD restarts".to_string(), t.text_faint),
    };
    ui.label(RichText::new(status.0).size(12.0).color(status.1));
    if !allow && on {
        ui.label(RichText::new("It stays on until you close JBD.").size(11.5).color(t.text_faint));
    }
    ui.add_space(16.0);

    let Some(program) = mcp_program() else {
        ui.label(RichText::new("JBD's MCP program (jbd-mcp.exe) wasn't found next to JBD. Reinstall JBD to get it.").size(12.0).color(t.text_dim));
        return;
    };
    let program = program.to_string_lossy().to_string();
    let bridge = format!("127.0.0.1:{PORT}");

    ui.label(RichText::new("Claude Code").font(Tokens::semibold(14.0)).color(t.text));
    ui.label(RichText::new("Run this once in a terminal, then ask Claude Code to edit your JBD project.").size(12.0).color(t.text_dim));
    let cmd = format!("claude mcp add jbd -- \"{program}\" mcp --bridge {bridge}");
    copy_block(app, ui, ctx, t, "settings.connections.copyClaudeCode", &cmd);
    ui.add_space(12.0);

    ui.label(RichText::new("Claude Desktop and other MCP apps").font(Tokens::semibold(14.0)).color(t.text));
    ui.label(RichText::new("Add this to the app's MCP settings (in Claude Desktop: Settings ▸ Developer ▸ Edit Config).").size(12.0).color(t.text_dim));
    let config = serde_json::to_string_pretty(&json!({"mcpServers": {"jbd": {"command": program, "args": ["mcp", "--bridge", bridge]}}})).unwrap_or_default();
    copy_block(app, ui, ctx, t, "settings.connections.copyDesktop", &config);
}

fn copy_block(app: &mut FilmcraftApp, ui: &mut egui::Ui, ctx: &egui::Context, t: &Tokens, id: &str, text: &str) {
    egui::Frame::new().fill(t.field_bg).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add(egui::Label::new(RichText::new(text).monospace().size(11.5).color(t.text)).wrap());
    });
    let b = style::small_primary_button(ui, "Copy", t);
    app.auto.add(id, b.rect, "Copy");
    if b.clicked() {
        ctx.copy_text(text.to_string());
    }
}

/// Settings ▸ Connectors.
pub fn connectors(app: &mut FilmcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (app, t);
        ui.label("Connectors work in the desktop app.");
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(path) = connectors_path() else { return };
        let st = &mut app.jbd.connections;
        let list = st.connectors.get_or_insert_with(|| jbd_director::mcp::load_config(&path));
        ui.label(
            RichText::new("MCP servers the Director can use, for example Google Drive, Figma or Notion. Their tools ask before each use unless you trust them. Changes apply to the next request.")
                .size(12.0)
                .color(t.text_dim),
        );
        ui.add_space(8.0);
        let mut changed = false;
        let mut remove = None;
        if list.is_empty() {
            ui.label(RichText::new("No connectors yet.").size(12.0).color(t.text_faint));
        }
        for (i, c) in list.iter_mut().enumerate() {
            egui::Frame::new().fill(t.field_bg).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&c.name).font(Tokens::semibold(13.0)).color(t.text));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Remove").clicked() {
                            remove = Some(i);
                        }
                        changed |= ui.checkbox(&mut c.trusted, "Trusted").on_hover_text("Its tools run without asking first").changed();
                        changed |= ui.checkbox(&mut c.enabled, "On").changed();
                    });
                });
                ui.label(RichText::new(format!("{} {}", c.command, c.args.join(" "))).monospace().size(11.0).color(t.text_dim));
            });
        }
        if let Some(i) = remove
            && i < list.len()
        {
            list.remove(i);
            changed = true;
        }
        ui.add_space(10.0);
        ui.label(RichText::new("Add a connector").font(Tokens::semibold(13.0)).color(t.text));
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut st.new_name).hint_text("Name, e.g. drive").desired_width(140.0));
            ui.add(
                egui::TextEdit::singleline(&mut st.new_command)
                    .hint_text("Command, e.g. npx -y @modelcontextprotocol/server-gdrive")
                    .desired_width(f32::INFINITY),
            );
        });
        let add = ui.add_enabled(!st.new_name.trim().is_empty() && !st.new_command.trim().is_empty(), egui::Button::new("Add"));
        if add.clicked() {
            let mut parts = st.new_command.split_whitespace().map(str::to_string);
            if let Some(command) = parts.next() {
                let list = st.connectors.get_or_insert_with(Vec::new);
                list.push(jbd_director::mcp::ConnectorConfig {
                    name: st.new_name.trim().to_string(),
                    command,
                    args: parts.collect(),
                    env: Default::default(),
                    enabled: true,
                    trusted: false,
                });
                st.new_name.clear();
                st.new_command.clear();
                changed = true;
            }
        }
        ui.label(
            RichText::new(format!("Servers that need API keys: add them under \"env\" in {} (same format as Claude Desktop).", path.display()))
                .size(11.5)
                .color(t.text_faint),
        );
        if changed {
            if let Some(list) = &app.jbd.connections.connectors
                && let Err(e) = jbd_director::mcp::save_config(&path, list)
            {
                app.jbd.connections.status = Some(Err(e));
            }
            // the Director restarts its connectors on the next request
            app.jbd.director.restart_connectors();
        }
    }
}
