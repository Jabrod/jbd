//! The Director tab (right side, next to Inspector and Transcript): JBD's AI agent.
//!
//! Laid out top to bottom:
//! - header: "Director" and "New chat";
//! - the plan (its to-do list), when it has one;
//! - the transcript: your messages, its replies, and one line per step (click to expand);
//! - a permission card when a step needs your yes (generating video always does);
//! - the prompt box, Claude-style: attached files as chips, the text (Enter sends, Shift+Enter
//!   is a new line), and one row of controls inside the box: attach (+), the thinking model, the
//!   video model, the mode (Ask / Auto edits / Plan), and the round send arrow (a stop square
//!   while it works);
//! - credits left, and "Undo all" for the last request.
//!
//! The state and the thread live in `director_run.rs`; the agent itself in `crates/jbd-director`.
//! Colours come from the theme tokens; the logo blue marks only what is active.

use egui::containers::menu::MenuButton;
use egui::{Align, Color32, Id, Key, KeyboardShortcut, Layout, Modifiers, Rect, RichText, Sense, Stroke, UiBuilder, pos2, vec2};
use jbd_director::{Entry, Mode, StepState, TodoState, lock};

use super::director_run::{self, Models};
use super::style;
use crate::FilmcraftApp;
use crate::icons::{self, Icon};
use crate::theme::Tokens;

const INPUT_ID: &str = "director-input";

/// Width of the chats sidebar in Director view.
const SIDEBAR_W: f32 = 230.0;

/// Director view's left column: New chat, then every saved chat, newest first. Click one to
/// open it; × deletes it. The list is read from disk every few seconds, not every frame.
fn chats_sidebar(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect, t: &Tokens) {
    let now = ui.input(|i| i.time);
    if now - app.jbd.director.chats_read_at > 4.0 || app.jbd.director.chats_read_at == 0.0 {
        app.jbd.director.chats = super::director_history::list();
        app.jbd.director.chats_read_at = now;
    }
    ui.painter().rect_filled(rect, 0.0, t.app_bg);
    let busy = app.jbd.director.busy();
    let current = app.jbd.director.chat_id.clone();
    let mut open: Option<String> = None;
    let mut delete: Option<String> = None;
    let mut new_chat = false;
    let inner = rect.shrink2(vec2(10.0, 12.0));
    let mut col = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)));
    col.spacing_mut().item_spacing = vec2(4.0, 4.0);
    let new =
        col.add_enabled(!busy, egui::Button::new(RichText::new("+  New chat").size(12.5).color(t.text)).min_size(vec2(inner.width(), 30.0)).corner_radius(8.0));
    app.auto.add("director.sidebar.new", new.rect, "New chat");
    if new.clicked() {
        new_chat = true;
    }
    col.add_space(8.0);
    col.label(RichText::new("Chats").size(11.0).color(t.text_faint));
    egui::ScrollArea::vertical().id_salt("director-sidebar").auto_shrink([false, false]).show(&mut col, |ui| {
        if app.jbd.director.chats.is_empty() {
            ui.label(RichText::new("Your chats will show here.").size(11.5).color(t.text_faint));
        }
        for c in &app.jbd.director.chats {
            let selected = c.id == current;
            let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
            let p = ui.painter();
            if selected {
                p.rect_filled(row, 8.0, t.field_bg);
            } else if resp.hovered() {
                p.rect_filled(row, 8.0, t.hover);
            }
            let title = p.layout_no_wrap(jbd_director::tools::clip(&c.title, 30), Tokens::ui(12.0), if selected { t.text } else { t.text_dim });
            p.galley(row.min + vec2(10.0, 7.0), title, t.text);
            p.text(row.min + vec2(10.0, 26.0), egui::Align2::LEFT_TOP, super::director_history::ago(c.updated), Tokens::ui(10.5), t.text_faint);
            let mut deleting = false;
            if resp.hovered() && !busy {
                let x = Rect::from_center_size(pos2(row.max.x - 14.0, row.center().y), vec2(18.0, 18.0));
                let xr = ui.interact(x, Id::new(("chat-delete", &c.id)), Sense::click());
                let s = Stroke::new(1.3, if xr.hovered() { t.danger } else { t.text_faint });
                ui.painter().line_segment([x.center() + vec2(-3.5, -3.5), x.center() + vec2(3.5, 3.5)], s);
                ui.painter().line_segment([x.center() + vec2(3.5, -3.5), x.center() + vec2(-3.5, 3.5)], s);
                if xr.on_hover_text("Delete this chat").clicked() {
                    delete = Some(c.id.clone());
                    deleting = true;
                }
            }
            if resp.clicked() && !deleting && !busy && !selected {
                open = Some(c.id.clone());
            }
        }
    });
    if new_chat {
        app.jbd.director.clear();
        app.jbd.director.chats_read_at = 0.0;
    }
    if let Some(id) = delete {
        super::director_history::delete(&id);
        app.jbd.director.chats.retain(|c| c.id != id);
        if id == app.jbd.director.chat_id {
            app.jbd.director.clear();
        }
    }
    if let Some(id) = open {
        app.jbd.director.open_chat(&id);
        app.jbd.director.chats_read_at = 0.0;
    }
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    ui.painter().rect_filled(rect, 0.0, t.panel_bg);
    director_run::load_models(app, ui.ctx());
    // Director view with room to spare: the chats get a sidebar of their own on the left
    let sidebar = app.jbd.areas.focus && rect.width() > 720.0 && !app.jbd.director.sidebar_hidden;
    let rect = if sidebar {
        let side = Rect::from_min_max(rect.min, pos2(rect.min.x + SIDEBAR_W, rect.max.y));
        chats_sidebar(app, ui, side, &t);
        ui.painter().line_segment([pos2(side.max.x, side.min.y), pos2(side.max.x, side.max.y)], Stroke::new(1.0, t.separator));
        Rect::from_min_max(pos2(side.max.x + 1.0, rect.min.y), rect.max)
    } else {
        rect
    };
    let inner = rect.shrink2(vec2(14.0, 12.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)));
    child.spacing_mut().item_spacing = vec2(6.0, 6.0);
    let ui = &mut child;

    header(app, ui, &t);
    // the prompt box grows with its text up to about a third of the tab, then scrolls inside
    let max_text = (inner.height() * 0.32).max(72.0);

    // egui panels do the layout: the bottom panel holds the plan strip, a question card, the
    // prompt box and a status line, and grows to exactly what they need; the conversation fills
    // the rest. (Measured each frame, never estimated, so the prompt box always sits at the
    // bottom of the tab and can’t be pushed off it.)
    egui::Panel::bottom(Id::new("director-bottom")).frame(egui::Frame::NONE).resizable(false).show_separator_line(false).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        ui.add_space(4.0);
        if !lock(&app.jbd.director.shared).todos.is_empty() {
            plan(app, ui, &t);
        }
        if lock(&app.jbd.director.shared).ask.is_some() {
            ask_card(app, ui, &t);
        }
        pending_strip(app, ui, &t);
        regen_strip(app, ui, &t);
        composer(app, ui, &t, max_text);
        footer(app, ui, &t);
    });
    egui::CentralPanel::no_frame().show(ui, |ui| transcript(app, ui, &t));
}

fn header(app: &mut FilmcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    // Left: what this chat is about. Right: History, New chat, Director view.
    let busy = app.jbd.director.busy();
    let mut open: Option<String> = None;
    let mut delete: Option<String> = None;
    ui.horizontal(|ui| {
        ui.set_height(26.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let focus = app.jbd.areas.focus;
            let view = icons::button(
                ui,
                Icon::Fullscreen,
                26.0,
                focus,
                t,
                if focus { "Back to the editor (show the timeline and media)" } else { "Director view: just the Preview and the Director" },
            );
            app.auto.add("director.view", view.rect, if focus { "Back to editor" } else { "Director view" });
            if view.clicked() {
                super::director_view(app, !focus);
            }

            // Director view with room for the chats column: show or hide it
            if focus && ui.ctx().content_rect().width() > 720.0 {
                let hidden = app.jbd.director.sidebar_hidden;
                let side = icons::button(ui, Icon::Sidebar, 26.0, !hidden, t, if hidden { "Show the chats sidebar" } else { "Hide the chats sidebar" });
                app.auto.add("director.sidebar.toggle", side.rect, if hidden { "Show sidebar" } else { "Hide sidebar" });
                if side.clicked() {
                    app.jbd.director.sidebar_hidden = !hidden;
                }
            }

            let new = icons::button(ui, Icon::NewItem, 26.0, false, t, "New chat");
            app.auto.add("director.newChat", new.rect, "New chat");
            if new.clicked() && !busy {
                app.jbd.director.clear();
            }

            // with the sidebar showing, the history button would only repeat it
            let sidebar = app.jbd.areas.focus && ui.max_rect().width() > 720.0 - SIDEBAR_W && !app.jbd.director.sidebar_hidden;
            if !sidebar {
                let hist = icons::button(ui, Icon::Clock, 26.0, false, t, "Chat history");
                app.auto.add("director.history", hist.rect, "Chat history");
                if hist.clicked() {
                    // read the list when the menu opens, not every frame
                    app.jbd.director.chats = super::director_history::list();
                }
                let current = app.jbd.director.chat_id.clone();
                egui::Popup::menu(&hist).width(300.0).show(|ui| {
                    ui.label(RichText::new("Chats").size(11.0).color(t.text_faint));
                    if app.jbd.director.chats.is_empty() {
                        ui.label(RichText::new("No saved chats yet. Each conversation is saved here as you go.").size(11.5).color(t.text_dim));
                    }
                    egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                        for c in &app.jbd.director.chats {
                            ui.horizontal(|ui| {
                                let x = ui.add(egui::Button::new(RichText::new("×").size(12.0).color(t.text_faint)).frame(false));
                                if x.on_hover_text("Delete this chat").clicked() {
                                    delete = Some(c.id.clone());
                                }
                                let row = ui.add_enabled(
                                    !busy,
                                    egui::Button::selectable(
                                        c.id == current,
                                        RichText::new(format!("{}   ·   {}", c.title, super::director_history::ago(c.updated))).size(12.0),
                                    ),
                                );
                                if row.clicked() {
                                    open = Some(c.id.clone());
                                }
                            });
                        }
                    });
                });
            }

            // the chat's title fills what is left on the left
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let title = {
                    let s = lock(&app.jbd.director.shared);
                    super::director_history::title_of(&s.entries)
                };
                ui.add(egui::Label::new(RichText::new(title).size(12.0).color(t.text_dim)).truncate());
            });
        });
    });
    if let Some(id) = delete {
        super::director_history::delete(&id);
        app.jbd.director.chats.retain(|c| c.id != id);
        if id == app.jbd.director.chat_id {
            app.jbd.director.clear();
        }
    }
    if let Some(id) = open {
        app.jbd.director.open_chat(&id);
        egui::Popup::close_all(ui.ctx());
    }
    ui.add_space(2.0);
}

/// When the one selected clip was generated by the Director: what made it, and three ways to
/// get another take. Regenerate and Variation ask the Director straight away; Edit prompt puts
/// the original prompt in the box to change first.
/// Clips that were still being made when JBD closed (or Stop was pressed): paid for, so offer
/// to fetch them into the project.
fn pending_strip(app: &mut FilmcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let now = ui.input(|i| i.time);
    let n = director_run::pending_clips(app, now);
    let fetching = app.jbd.director.fetching.load(std::sync::atomic::Ordering::Relaxed);
    if n == 0 || app.jbd.director.busy() {
        return;
    }
    let mut fetch = false;
    egui::Frame::new().fill(t.field_bg).corner_radius(8.0).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            if fetching {
                super::cube::loader(ui, 12.0);
                ui.label(RichText::new(format!("Fetching {n} clip{}…", if n == 1 { "" } else { "s" })).size(11.5).color(t.text_dim));
            } else {
                ui.label(
                    RichText::new(format!("{n} clip{} still being made when the Director stopped", if n == 1 { " was" } else { "s were" }))
                        .size(11.5)
                        .color(t.text_dim),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let b = ui.add(egui::Button::new(RichText::new("Fetch them").size(11.5).color(t.text)).frame(false));
                    app.auto.add("director.fetchPending", b.rect, "Fetch them");
                    if b.on_hover_text("Download them into the project (they were already paid for)").clicked() {
                        fetch = true;
                    }
                });
            }
        });
    });
    if fetch {
        director_run::fetch_pending(app, ui.ctx());
    }
}

fn regen_strip(app: &mut FilmcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    if app.jbd.director.busy() {
        return;
    }
    let Some((clip_id, g)) = director_run::selected_generation(app) else { return };
    let mut action: Option<&str> = None;
    egui::Frame::new().fill(t.field_bg).corner_radius(8.0).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Generated clip · {} · {} s", g.model_label, g.seconds)).size(11.5).color(t.text_dim)).on_hover_text(&g.prompt);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                for (id, label, hint) in [
                    ("edit", "Edit prompt", "Change the prompt, then regenerate"),
                    ("variation", "Variation", "A new take on the track above, to compare"),
                    ("regenerate", "Regenerate", "A new take in place of this one"),
                ] {
                    let b = ui.add(egui::Button::new(RichText::new(label).size(11.5).color(t.text)).frame(false));
                    app.auto.add(&format!("director.regen.{id}"), b.rect, label);
                    if b.on_hover_text(hint).clicked() {
                        action = Some(id);
                    }
                }
            });
        });
    });
    match action {
        Some("regenerate") => {
            app.jbd.director.input = format!("Regenerate clip {clip_id} in place.");
            director_run::send(app, ui.ctx());
        }
        Some("variation") => {
            app.jbd.director.input = format!("Make a variation of clip {clip_id} on the track above so I can compare.");
            director_run::send(app, ui.ctx());
        }
        Some("edit") => {
            app.jbd.director.input = format!("Regenerate clip {clip_id} with this prompt: {}", g.prompt);
            ui.memory_mut(|m| m.request_focus(Id::new(INPUT_ID)));
        }
        _ => {}
    }
}

/// The plan as one slim line above the prompt box: the step being worked on and how far along
/// ("Generate hero shot · 2 of 7"). Click it to see every step. Keeping it to one line leaves
/// the panel to the conversation.
fn plan(app: &mut FilmcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let todos = lock(&app.jbd.director.shared).todos.clone();
    if todos.is_empty() {
        return;
    }
    let done = todos.iter().filter(|x| x.state == TodoState::Done).count();
    let current = todos.iter().find(|x| x.state == TodoState::Doing).or_else(|| todos.iter().find(|x| x.state == TodoState::Todo));
    let open = app.jbd.director.plan_open;
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 6.0, t.hover);
    }
    let y = rect.center().y;
    let (dot, label) = match current {
        Some(x) => (if x.state == TodoState::Doing { style::BLUE } else { t.text_faint }, x.text.as_str()),
        None => (t.text_faint, "Plan done"),
    };
    p.circle_filled(pos2(rect.left() + 8.0, y), 3.0, dot);
    let count = format!("{} of {}", (done + usize::from(current.is_some())).min(todos.len()), todos.len());
    let count_w = 64.0;
    let galley = p.layout_no_wrap(jbd_director::tools::clip(label, 200), Tokens::ui(12.0), t.text);
    let room = (rect.width() - 20.0 - count_w - 18.0).max(20.0);
    let clipped = p.with_clip_rect(Rect::from_min_max(pos2(rect.left() + 18.0, rect.top()), pos2(rect.left() + 18.0 + room, rect.bottom())));
    clipped.galley(pos2(rect.left() + 18.0, y - galley.size().y / 2.0), galley, t.text);
    p.text(pos2(rect.right() - 20.0, y), egui::Align2::RIGHT_CENTER, count, Tokens::ui(11.0), t.text_faint);
    // a chevron that points up when the list is open
    let c = pos2(rect.right() - 9.0, y);
    let s = Stroke::new(1.3, t.text_faint);
    let d = if open { -1.5 } else { 1.5 };
    p.line_segment([c + vec2(-3.0, -d), c + vec2(0.0, d)], s);
    p.line_segment([c + vec2(0.0, d), c + vec2(3.0, -d)], s);
    app.auto.add("director.plan", rect, "Plan");
    if resp.on_hover_text(if open { "Hide the plan" } else { "Show every step" }).clicked() {
        app.jbd.director.plan_open = !open;
    }
    if open {
        egui::Frame::new().fill(t.field_bg).corner_radius(6.0).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical().id_salt("director-plan").max_height(150.0).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for todo in &todos {
                    let (mark, color) = match todo.state {
                        TodoState::Done => ("✓", t.text_faint),
                        TodoState::Doing => ("●", style::BLUE),
                        TodoState::Todo => ("○", t.text_faint),
                    };
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(mark).size(11.0).color(color));
                        let text = RichText::new(&todo.text).size(11.5).color(if todo.state == TodoState::Done { t.text_faint } else { t.text_dim });
                        ui.add(egui::Label::new(text).truncate());
                    });
                }
            });
        });
    }
}

fn transcript(app: &mut FilmcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let shared = app.jbd.director.shared.clone();
    let open = app.jbd.director.open_groups.clone();
    let open_messages = app.jbd.director.open_messages.clone();
    let mut toggle: Option<u64> = None;
    let mut toggle_message: Option<usize> = None;
    egui::ScrollArea::vertical().id_salt("director-transcript").auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let mut s = lock(&shared);
        if s.entries.is_empty() {
            empty_state(ui, t);
        }
        let busy = s.busy;
        let n = s.entries.len();
        // what the Director is doing between steps, in an editor's words while it only thinks
        let doing = s.activity.clone().map(|a| working_words(&a, ui.input(|i| i.time)));
        let mut i = 0;
        while i < n {
            // A run of steps is one collapsible line, like Claude Code: the conversation stays
            // readable, and every step is one click away.
            if matches!(s.entries.get(i), Some(Entry::Step(_))) {
                let end = (i..n).find(|&j| !matches!(s.entries.get(j), Some(Entry::Step(_)))).unwrap_or(n);
                let key = match s.entries.get(i) {
                    Some(Entry::Step(st)) => st.id,
                    _ => 0,
                };
                if let Some(group) = s.entries.get_mut(i..end)
                    && group_ui(ui, group, open.contains(&key), busy && end == n, doing.as_deref(), t)
                {
                    toggle = Some(key);
                }
                i = end;
                continue;
            }
            match s.entries.get(i) {
                Some(Entry::User(text)) => {
                    // a long message (a pasted brief) shows its start; "Show more" opens it
                    let long = is_long_message(text);
                    let expanded = open_messages.contains(&i);
                    ui.add_space(6.0);
                    egui::Frame::new().fill(t.field_bg).corner_radius(8.0).inner_margin(egui::Margin::symmetric(10, 7)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let shown = if long && !expanded { message_preview(text) } else { text.clone() };
                        ui.add(egui::Label::new(RichText::new(shown).size(12.5).color(t.text)).wrap());
                        if long {
                            let more = ui.add(
                                egui::Label::new(RichText::new(if expanded { "Show less" } else { "Show more" }).size(11.0).color(t.text_dim))
                                    .sense(Sense::click()),
                            );
                            if more.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                toggle_message = Some(i);
                            }
                        }
                    });
                    ui.add_space(2.0);
                }
                Some(Entry::Said(text)) => {
                    markdown(ui, text, t);
                    ui.add_space(2.0);
                }
                Some(Entry::Error(text)) => {
                    ui.label(RichText::new(text.as_str()).size(12.0).color(t.danger));
                }
                Some(Entry::Spent(text)) => {
                    ui.label(RichText::new(text.as_str()).size(10.5).color(t.text_faint));
                }
                // what was attached to the message above, as pictures (like Claude's chat)
                Some(Entry::Attached(items)) => {
                    let ctx = ui.ctx().clone();
                    ui.horizontal_wrapped(|ui| {
                        for (item, label) in items {
                            let thumb = app.thumbnail(&ctx, filmcraft_project::ItemId(*item), filmcraft_engine::time::Tick(0), 160);
                            picture_tile(ui, label, thumb, t);
                        }
                    });
                }
                _ => {}
            }
            i += 1;
        }
        // Thinking between steps, when no group line is already showing it
        let last_is_steps = matches!(s.entries.last(), Some(Entry::Step(_)));
        if let Some(a) = doing.clone().filter(|_| !last_is_steps && s.ask.is_none()) {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                super::cube::loader(ui, 12.0);
                ui.label(RichText::new(a).size(11.5).color(t.text_faint));
            });
        }
    });
    if let Some(k) = toggle
        && !app.jbd.director.open_groups.remove(&k)
    {
        app.jbd.director.open_groups.insert(k);
    }
    if let Some(k) = toggle_message
        && !app.jbd.director.open_messages.remove(&k)
    {
        app.jbd.director.open_messages.insert(k);
    }
}

/// Messages longer than this fold to a preview in the transcript.
const MESSAGE_LINES: usize = 10;
const MESSAGE_CHARS: usize = 1_200;

fn is_long_message(text: &str) -> bool {
    text.lines().count() > MESSAGE_LINES || text.chars().count() > MESSAGE_CHARS
}

/// The first lines of a long message, with an ellipsis.
fn message_preview(text: &str) -> String {
    let head: String = text.lines().take(MESSAGE_LINES).collect::<Vec<_>>().join("\n");
    let head = jbd_director::tools::clip(&head, MESSAGE_CHARS);
    format!("{}…", head.trim_end_matches('…').trim_end())
}

/// One group of steps: "Worked · 14 steps · 48 s · 120 cr" (click to see them), or, while the
/// last group is still running, the cube and what it is doing now. Returns true when clicked.
/// While the Director thinks, the line says what an editor would be doing, a new word every
/// couple of seconds; anything concrete it reports ("Generating 3 clips with Veo…") is shown as is.
fn working_words(activity: &str, time: f64) -> String {
    const WORDS: [&str; 12] = [
        "Thinking",
        "Directing",
        "Watching the cut",
        "Cutting",
        "Looking",
        "Framing",
        "Pacing",
        "Trimming",
        "Finding the beat",
        "Grading",
        "Reviewing",
        "Polishing",
    ];
    if activity != "Thinking…" && activity != "Checking the edit…" {
        return activity.to_string();
    }
    let i = ((time / 2.2) as usize) % WORDS.len();
    format!("{}…", WORDS.get(i).copied().unwrap_or("Thinking"))
}

fn group_ui(ui: &mut egui::Ui, steps: &mut [Entry], open: bool, live: bool, doing: Option<&str>, t: &Tokens) -> bool {
    let mut count = 0;
    let mut took = 0.0_f32;
    // wall time: from the first step's start to the last one's end (the thinking between them
    // included); chats saved before steps had a start time fall back to adding up the steps
    let (mut first, mut last) = (f64::MAX, 0.0_f64);
    let mut credits = 0u32;
    let mut failed = 0;
    let mut current: Option<String> = None;
    let mut progress: Option<f32> = None;
    for e in steps.iter() {
        if let Entry::Step(st) = e {
            count += 1;
            took += st.took;
            if st.started > 0.0 {
                first = first.min(st.started);
                last = last.max(st.started + f64::from(st.took));
            }
            credits = credits.saturating_add(st.credits);
            if st.state == StepState::Failed {
                failed += 1;
            }
            if st.state == StepState::Running {
                current = Some(st.title.clone());
                progress = st.progress.or(progress);
            }
        }
    }
    if last > first && first < f64::MAX {
        took = took.max((last - first) as f32);
    }
    // the newest group while the Director works: the cube turns, between steps too (it is
    // thinking about the next one), never a still "Worked" line that looks stuck
    let running = live && (current.is_some() || doing.is_some());
    ui.add_space(2.0);
    let line = ui.horizontal(|ui| {
        if running {
            super::cube::loader(ui, 12.0);
        } else {
            let (r, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
            // a chevron: right when closed, down when open
            let c = r.center();
            let s = Stroke::new(1.3, t.text_faint);
            if open {
                ui.painter().line_segment([c + vec2(-3.0, -1.5), c + vec2(0.0, 1.5)], s);
                ui.painter().line_segment([c + vec2(0.0, 1.5), c + vec2(3.0, -1.5)], s);
            } else {
                ui.painter().line_segment([c + vec2(-1.5, -3.0), c + vec2(1.5, 0.0)], s);
                ui.painter().line_segment([c + vec2(1.5, 0.0), c + vec2(-1.5, 3.0)], s);
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if credits > 0 {
                ui.label(RichText::new(format!("{credits} cr")).size(10.5).color(t.text_faint));
            }
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let time = if took >= 60.0 { format!("{:.1} min", took / 60.0) } else { format!("{took:.0} s") };
                let label = if running {
                    // what it is doing in an editor's words, then how far along (Claude Code style);
                    // the tool calls themselves are listed under this line
                    let word = doing.unwrap_or("Working…");
                    let mut job = egui::text::LayoutJob::default();
                    job.append(word, 0.0, egui::TextFormat { font_id: Tokens::ui(11.5), color: t.text, ..Default::default() });
                    job.append(
                        &format!("  ·  {count} step{} · {time}", if count == 1 { "" } else { "s" }),
                        0.0,
                        egui::TextFormat { font_id: Tokens::ui(11.0), color: t.text_faint, ..Default::default() },
                    );
                    return ui.add(egui::Label::new(job).truncate().sense(Sense::click()));
                } else {
                    let fails = if failed > 0 { format!(" · {failed} failed") } else { String::new() };
                    format!("Worked · {count} step{} · {time}{fails}", if count == 1 { "" } else { "s" })
                };
                ui.add(egui::Label::new(RichText::new(label).size(11.5).color(t.text_dim)).truncate().sense(Sense::click()))
            })
            .inner
        })
        .inner
    });
    let clicked = line.inner.clicked();
    // a clip being made shows its progress even when the group is closed
    if let Some(p) = progress.filter(|_| running) {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 3.0), Sense::hover());
        style::meter(ui.painter(), r, p, t, Some(style::BLUE));
    }
    // while it works, the newest tool calls show under the line as they happen (the running one
    // with its own cube); click the line for all of them
    if running && !open {
        const TAIL: usize = 3;
        let n = steps.iter().filter(|e| matches!(e, Entry::Step(_))).count();
        egui::Frame::new().inner_margin(egui::Margin { left: 16, right: 0, top: 1, bottom: 3 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            if n > TAIL {
                ui.label(RichText::new(format!("… {} earlier step{}", n - TAIL, if n - TAIL == 1 { "" } else { "s" })).size(10.5).color(t.text_faint));
            }
            for e in steps.iter_mut().filter(|e| matches!(e, Entry::Step(_))).skip(n.saturating_sub(TAIL)) {
                if let Entry::Step(st) = e {
                    step_line(ui, st, t);
                }
            }
        });
    }
    if open {
        egui::Frame::new().inner_margin(egui::Margin { left: 16, right: 0, top: 2, bottom: 4 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            for e in steps.iter_mut() {
                if let Entry::Step(st) = e {
                    step_line(ui, st, t);
                }
            }
        });
    }
    clicked
}

/// One step inside an opened group: its mark, title, time and credits; click for the details.
fn step_line(ui: &mut egui::Ui, step: &mut jbd_director::Step, t: &Tokens) {
    let (mark, color) = match step.state {
        StepState::Running => ("", style::BLUE),
        StepState::Ok => ("✓", t.text_faint),
        StepState::Failed => ("✗", t.danger),
        StepState::Skipped => ("–", t.text_faint),
        StepState::Partial => ("!", Color32::from_rgb(0xf5, 0xa5, 0x24)),
    };
    // The time and credits are laid out first, from the right, so the title gets exactly the
    // width that is left and is cut short instead of running under them.
    let line = ui.horizontal(|ui| {
        if step.state == StepState::Running {
            super::cube::loader(ui, 11.0);
        } else {
            ui.label(RichText::new(mark).size(11.0).color(color));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if step.credits > 0 {
                ui.label(RichText::new(format!("{} cr", step.credits)).size(10.5).color(t.text_faint));
            }
            if step.state != StepState::Running && step.took >= 0.1 {
                ui.label(RichText::new(format!("{:.1} s", step.took)).size(10.5).color(t.text_faint));
            }
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let title = RichText::new(step.title.as_str()).size(11.0).color(t.text_dim).family(egui::FontFamily::Monospace);
                ui.add(egui::Label::new(title).truncate().sense(Sense::click()))
            })
            .inner
        })
        .inner
    });
    if line.inner.clicked() {
        step.expanded = !step.expanded;
    }
    if step.expanded {
        egui::Frame::new().fill(t.field_bg).corner_radius(4.0).inner_margin(6.0).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(RichText::new(step.detail.as_str()).size(10.5).family(egui::FontFamily::Monospace).color(t.text_dim)).wrap());
            if !step.result.is_empty() {
                ui.separator();
                ui.add(egui::Label::new(RichText::new(step.result.as_str()).size(10.5).family(egui::FontFamily::Monospace).color(t.text_dim)).wrap());
            }
        });
    }
}

fn empty_state(ui: &mut egui::Ui, t: &Tokens) {
    ui.add_space(8.0);
    ui.label(RichText::new("Tell the Director what you want. It works in steps you can watch, and every change can be undone.").size(12.0).color(t.text_dim));
    ui.add_space(6.0);
    for example in [
        "Make a 30-second 9:16 reel from my clips",
        "Remove the pauses from the interview",
        "Generate a 6-second drone shot of a city at dawn for the intro",
        "Add a cross dissolve between every clip",
    ] {
        ui.label(RichText::new(format!("“{example}”")).size(11.5).color(t.text_faint));
    }
}

/// A chip-style picker inside the prompt box: the current choice and a small chevron.
fn picker<R>(ui: &mut egui::Ui, label: &str, hint: &str, t: &Tokens, content: impl FnOnce(&mut egui::Ui) -> R) -> egui::Response {
    let shown = jbd_director::tools::clip(label, 22);
    // The trailing spaces leave room for the chevron painted on top.
    let text = RichText::new(format!("{shown}      ")).size(11.5).color(t.text_dim);
    let resp = MenuButton::from_button(egui::Button::new(text).frame(false).min_size(vec2(0.0, 24.0))).ui(ui, content).0;
    if resp.hovered() {
        ui.painter().rect_filled(resp.rect, 6.0, t.hover);
        ui.painter().text(resp.rect.left_center() + vec2(4.0, 0.0), egui::Align2::LEFT_CENTER, &shown, Tokens::ui(11.5), t.text);
    }
    chevron(ui.painter(), pos2(resp.rect.right() - 8.0, resp.rect.center().y + 1.0), t.text_faint);
    resp.on_hover_text(hint)
}

fn chevron(p: &egui::Painter, c: egui::Pos2, color: Color32) {
    let s = Stroke::new(1.3, color);
    p.line_segment([c + vec2(-3.0, -1.5), c + vec2(0.0, 1.5)], s);
    p.line_segment([c + vec2(0.0, 1.5), c + vec2(3.0, -1.5)], s);
}

/// The round send button (an up arrow), or a stop square while the Director works.
fn send_button(ui: &mut egui::Ui, busy: bool, can_send: bool, t: &Tokens) -> egui::Response {
    let active = busy || can_send;
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), if active { Sense::click() } else { Sense::hover() });
    let p = ui.painter();
    let c = rect.center();
    let fill = match (active, resp.hovered()) {
        (true, true) => style::PRIMARY_BG_HOVER,
        (true, false) => style::PRIMARY_BG,
        _ => t.hover,
    };
    p.circle_filled(c, 14.0, fill);
    let ink = if active { style::PRIMARY_TEXT } else { t.text_faint };
    if busy {
        p.rect_filled(Rect::from_center_size(c, vec2(9.0, 9.0)), 2.0, ink);
    } else {
        let s = Stroke::new(1.8, ink);
        p.line_segment([c + vec2(0.0, 5.5), c + vec2(0.0, -5.5)], s);
        p.line_segment([c + vec2(0.0, -5.5), c + vec2(-4.5, -1.0)], s);
        p.line_segment([c + vec2(0.0, -5.5), c + vec2(4.5, -1.0)], s);
    }
    resp
}

fn composer(app: &mut FilmcraftApp, ui: &mut egui::Ui, t: &Tokens, max_text: f32) {
    let busy = app.jbd.director.busy();
    let input_id = Id::new(INPUT_ID);
    let focused = ui.memory(|m| m.has_focus(input_id));
    let border = if focused { t.focus } else { t.field_border };
    let mut send = false;
    let mut stop = false;
    let mut attach = false;
    let mut remove: Option<usize> = None;
    let mut rects: Vec<(&str, Rect, &str)> = Vec::new();
    // Thumbnails are fetched before drawing the box (they need the whole app).
    let ctx = ui.ctx().clone();
    let attached = app.jbd.director.attachments.clone();
    let thumbs: Vec<(String, Option<(egui::TextureId, egui::Vec2)>)> = attached
        .iter()
        .map(|a| (a.name.clone(), a.item.and_then(|id| app.thumbnail(&ctx, filmcraft_project::ItemId(id), filmcraft_engine::time::Tick(0), 160))))
        .collect();
    // Timeline clips as context: the ones selected but not attached yet (for the pill), and the
    // attached ones (chips: a frame of the clip; hover shows its track, time and name)
    let fresh: Vec<director_run::ClipRef> =
        director_run::selected_clips(app).into_iter().filter(|c| !app.jbd.director.context.iter().any(|x| x.clip == c.clip)).collect();
    let context = app.jbd.director.context.clone();
    let context_thumbs: Vec<(String, Option<(egui::TextureId, egui::Vec2)>)> = context
        .iter()
        .map(|c| (format!("{}\n{}", c.label(), c.name), app.thumbnail(&ctx, filmcraft_project::ItemId(c.item), filmcraft_engine::time::Tick(0), 160)))
        .collect();
    let mut add_selected = false;
    let mut remove_clip: Option<usize> = None;
    let mut remove_paste: Option<usize> = None;

    ui.add_space(2.0);
    egui::Frame::new()
        .fill(t.field_bg)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(12.0)
        .inner_margin(egui::Margin { left: 10, right: 8, top: 8, bottom: 6 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());

            // "+ Add 3 selected clips": the timeline selection, attached as context in one click
            if !fresh.is_empty() {
                let n = fresh.len();
                let label = format!("+ Add {n} selected clip{} as context", if n == 1 { "" } else { "s" });
                let b = ui.add(egui::Button::new(RichText::new(label).size(11.5).color(t.text)).corner_radius(6.0).fill(t.hover));
                rects.push(("director.addSelection", b.rect, "Add selected clips"));
                if b.on_hover_text("Tell the Director these are the clips you mean").clicked() {
                    add_selected = true;
                }
            }
            // The attached timeline clips, files (a frame of the video or picture; audio shows
            // its name; hover for the name and an × to remove it) and pasted texts, in rows that
            // wrap; many of them scroll in a short strip instead of pushing the text box away.
            let pasted: Vec<usize> = app.jbd.director.pasted.iter().map(|p| p.chars().count()).collect();
            if !context_thumbs.is_empty() || !thumbs.is_empty() || !pasted.is_empty() {
                egui::ScrollArea::vertical().id_salt("director-chips").max_height(120.0).auto_shrink([false, true]).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for (i, (label, thumb)) in context_thumbs.iter().enumerate() {
                            if attachment_tile(ui, label, *thumb, t) {
                                remove_clip = Some(i);
                            }
                        }
                        for (i, (name, thumb)) in thumbs.iter().enumerate() {
                            if attachment_tile(ui, name, *thumb, t) {
                                remove = Some(i);
                            }
                        }
                        for (i, n) in pasted.iter().enumerate() {
                            if pasted_chip(ui, *n, t) {
                                remove_paste = Some(i);
                            }
                        }
                    });
                });
            }

            // A long paste becomes a chip (above) instead of filling the box, like Claude Code.
            if focused {
                let big: Vec<String> = ui.input_mut(|i| {
                    let mut out = Vec::new();
                    i.events.retain(|e| match e {
                        egui::Event::Paste(s) if s.chars().count() > director_run::PASTE_CHIP_CHARS => {
                            out.push(s.clone());
                            false
                        }
                        _ => true,
                    });
                    out
                });
                app.jbd.director.pasted.extend(big);
            }

            // The text. Shift+Enter makes a new line; plain Enter sends (below). It grows with
            // what is typed up to `max_text`, then scrolls inside the box.
            let hint = if busy { "Working… type your next request" } else { "Ask the Director to edit, research or make a video…" };
            let edit = egui::TextEdit::multiline(&mut app.jbd.director.input)
                .id(input_id)
                .frame(egui::Frame::NONE)
                .hint_text(RichText::new(hint).color(t.text_faint))
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter));
            let resp = egui::ScrollArea::vertical()
                .id_salt("director-input-scroll")
                // wheel and scrollbar only: drag-to-scroll would take the click meant for the text
                .scroll_source(egui::scroll_area::ScrollSource::SCROLL_BAR | egui::scroll_area::ScrollSource::MOUSE_WHEEL)
                .max_height(max_text)
                // as short as the text (egui's default minimum left a dead strip under it)
                .min_scrolled_height(0.0)
                .auto_shrink([false, true])
                .stick_to_bottom(true)
                .show(ui, |ui| ui.add(edit));
            // the part of the box on screen (the text itself can be far taller)
            let area = resp.inner_rect;
            rects.push(("director.input", area, "Ask the Director"));
            let resp = resp.inner;
            // a click anywhere in the text area puts the cursor in it
            if !resp.has_focus() && ui.input(|i| i.pointer.primary_clicked() && i.pointer.interact_pos().is_some_and(|p| area.contains(p))) {
                ui.memory_mut(|m| m.request_focus(input_id));
            }
            if resp.has_focus() && ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift) {
                send = true;
            }

            // One row of controls inside the box.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let plus = icons::button(ui, Icon::Plus, 24.0, false, t, "Attach files: video, audio or pictures");
                rects.push(("director.attach", plus.rect, "Attach files"));
                if plus.clicked() {
                    attach = true;
                }

                let models = app.jbd.director.models.clone();
                let guard = lock(&models);
                let d = &mut app.jbd.director;
                match &*guard {
                    Models::Ready(cat) => {
                        if d.chat_model.is_empty() {
                            d.chat_model = cat.default_chat().map(|m| m.id.clone()).unwrap_or_default();
                        }
                        if d.video_model.is_empty() {
                            d.video_model = cat.default_video().map(|m| m.id.clone()).unwrap_or_default();
                        }
                        let chat_label = cat
                            .chat
                            .iter()
                            .find(|m| m.id == d.chat_model)
                            .map(|m| m.label.trim_start_matches("Claude ").to_string())
                            .unwrap_or_else(|| "Model".into());
                        let r = picker(ui, &chat_label, "What the Director thinks with", t, |ui| {
                            ui.label(RichText::new("Thinks with").size(10.5).color(t.text_faint));
                            for m in &cat.chat {
                                ui.selectable_value(&mut d.chat_model, m.id.clone(), format!("{}  ·  {}", m.label, m.vendor));
                            }
                        });
                        rects.push(("director.chatModel", r.rect, "Thinking model"));
                        // the chip says when sound is forced on or off (Auto shows just the model)
                        let base = cat.video(&d.video_model).map(|m| m.label.clone()).unwrap_or_else(|| "Video".into());
                        let video_label = match d.sound {
                            jbd_director::Sound::Auto => base,
                            jbd_director::Sound::On => format!("{base} + sound"),
                            jbd_director::Sound::Off => format!("{base}, silent"),
                        };
                        let r = picker(ui, &video_label, "What the Director makes video with", t, |ui| {
                            ui.label(RichText::new("Makes video with  ·  credits for one clip").size(10.5).color(t.text_faint));
                            for m in &cat.video {
                                let secs = m.nearest_duration(6);
                                let cost = m.credits_for(secs, &m.best_resolution(None), false);
                                let sound = if m.audio { "" } else { "  ·  silent" };
                                ui.selectable_value(&mut d.video_model, m.id.clone(), format!("{}  ·  {cost} cr / {secs} s{sound}", m.label));
                            }
                            ui.separator();
                            for s in jbd_director::Sound::ALL {
                                ui.selectable_value(&mut d.sound, s, s.label());
                            }
                        });
                        rects.push(("director.videoModel", r.rect, "Video model"));
                    }
                    Models::Loading => {
                        // JBD's one loader (the turning cube), as everywhere else in the app
                        let r = super::cube::loader(ui, 14.0);
                        r.on_hover_text("Loading the models");
                    }
                    _ => {}
                }
                drop(guard);

                let mode = &mut app.jbd.director.mode;
                let r = picker(ui, mode.label(), "How much the Director does without asking", t, |ui| {
                    for (m, hint) in [
                        (Mode::Ask, "Asks before every change"),
                        (Mode::AutoEdits, "Edits right away (all undoable); asks before spending credits"),
                        (Mode::Plan, "Looks and researches, then proposes; changes nothing"),
                    ] {
                        ui.selectable_value(mode, m, m.label()).on_hover_text(hint);
                    }
                });
                rects.push(("director.mode", r.rect, "Director mode"));

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let can_send = !app.jbd.director.input.trim().is_empty() || !app.jbd.director.pasted.is_empty();
                    let b = send_button(ui, busy, can_send, t);
                    ui.add_space(4.0);
                    let credits = lock(&app.jbd.director.shared).credits;
                    if let Some(c) = credits {
                        let ring = usage_ring(ui, c, app.jbd.director.context_fraction(), t);
                        rects.push(("director.credits", ring.rect, "AI credits"));
                    }
                    if busy {
                        rects.push(("director.stop", b.rect, "Stop"));
                        if b.on_hover_text("Stop").clicked() {
                            stop = true;
                        }
                    } else {
                        rects.push(("director.send", b.rect, "Send"));
                        if b.on_hover_text("Send (Enter)").clicked() {
                            send = true;
                        }
                    }
                });
            });
        });

    for (id, r, label) in rects {
        app.auto.add(id, r, label);
    }
    if let Some(i) = remove
        && i < app.jbd.director.attachments.len()
    {
        app.jbd.director.attachments.remove(i);
    }
    if attach {
        director_run::attach(app);
    }
    if add_selected {
        app.jbd.director.context.extend(fresh);
    }
    if let Some(i) = remove_clip
        && i < app.jbd.director.context.len()
    {
        app.jbd.director.context.remove(i);
    }
    if let Some(i) = remove_paste
        && i < app.jbd.director.pasted.len()
    {
        app.jbd.director.pasted.remove(i);
    }
    if stop {
        app.jbd.director.stop();
    }
    if send && !busy {
        director_run::send(app, ui.ctx());
    }
}

/// The line under the prompt box only appears when it has something to say.
fn footer_needed(app: &FilmcraftApp) -> bool {
    !app.jbd.status.is_signed_in()
        || matches!(&*lock(&app.jbd.director.models), Models::Failed(..))
        || (director_run::can_undo_all(app) && !app.jbd.director.busy())
}

fn footer(app: &mut FilmcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    if !footer_needed(app) {
        return;
    }
    let failed = match &*lock(&app.jbd.director.models) {
        Models::Failed(e, _) => Some(e.clone()),
        _ => None,
    };
    ui.horizontal(|ui| {
        if !app.jbd.status.is_signed_in() {
            ui.label(RichText::new("Sign in to your Jabrod account to use the Director.").size(11.0).color(t.text_dim));
        } else if let Some(e) = failed {
            ui.label(RichText::new(format!("Couldn\u{2019}t load the models: {}", jbd_director::tools::clip(&e, 60))).size(11.0).color(t.text_dim));
            let retry = ui.small_button("Retry");
            app.auto.add("director.retryModels", retry.rect, "Retry");
            if retry.clicked() {
                *lock(&app.jbd.director.models) = Models::NotLoaded;
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if director_run::can_undo_all(app) && !app.jbd.director.busy() {
                let undo = ui.add(egui::Button::new(RichText::new("Undo all").size(11.0).color(t.text_dim)).frame(false));
                app.auto.add("director.undoAll", undo.rect, "Undo all");
                if undo.on_hover_text("Take back everything the last request changed").clicked() {
                    director_run::undo_all(app, ui.ctx());
                }
            }
        });
    });
}

/// This month’s AI credits as a small ring next to the send button: it fills as credits are
/// used, turns amber at 80 % and red when they run out; hovering shows the numbers.
fn usage_ring(ui: &mut egui::Ui, c: jbd_director::Credits, context: f32, t: &Tokens) -> egui::Response {
    const AMBER: Color32 = Color32::from_rgb(0xf5, 0xa5, 0x24);
    let (rect, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
    let p = ui.painter();
    let center = rect.center();
    let radius = 7.5;
    let used = c.fraction();
    let color = if c.left() == 0 {
        t.danger
    } else if used >= 0.8 {
        AMBER
    } else {
        style::BLUE
    };
    p.circle_stroke(center, radius, Stroke::new(2.5, t.hover));
    if used > 0.0 {
        // clockwise from twelve o’clock
        let steps = ((used * 48.0).ceil() as usize).max(2);
        let points: Vec<egui::Pos2> = (0..=steps)
            .map(|i| {
                let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * used * (i as f32 / steps as f32);
                center + vec2(a.cos(), a.sin()) * radius
            })
            .collect();
        p.add(egui::Shape::line(points, Stroke::new(2.5, color)));
    }
    // Two lines only: what this conversation holds, and what this month has used.
    let tip = format!(
        "Usage  {} / {} credits
Context  {:.0}%",
        style::thousands(c.used),
        style::thousands(c.limit),
        context * 100.0
    );
    resp.on_hover_text(tip)
}

/// The Director’s replies are written in light markdown: paragraphs, **bold**, lists
/// (`- ` / `1. `) and `#` headings. Render those; show anything else as plain text.
pub(crate) fn markdown(ui: &mut egui::Ui, text: &str, t: &Tokens) {
    for raw in text.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            ui.add_space(4.0);
            continue;
        }
        let trimmed = line.trim_start();
        let indent = (line.len() - trimmed.len()) as f32 * 4.0;
        let (prefix, body, heading) =
            if let Some(h) = trimmed.strip_prefix("### ").or_else(|| trimmed.strip_prefix("## ")).or_else(|| trimmed.strip_prefix("# ")) {
                (String::new(), h, true)
            } else if let Some(b) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
                ("•  ".to_string(), b, false)
            } else if let Some(dot) = trimmed.find(". ").filter(|&i| i > 0 && i <= 3 && trimmed[..i].chars().all(|c| c.is_ascii_digit())) {
                (format!("{}.  ", &trimmed[..dot]), &trimmed[dot + 2..], false)
            } else {
                (String::new(), trimmed, false)
            };
        let mut job = egui::text::LayoutJob::default();
        job.wrap.max_width = ui.available_width() - indent;
        let plain = egui::TextFormat { font_id: Tokens::ui(12.5), color: t.text, ..Default::default() };
        let bold = egui::TextFormat { font_id: Tokens::semibold(12.5), color: t.text, ..Default::default() };
        if !prefix.is_empty() {
            job.append(&prefix, 0.0, egui::TextFormat { color: t.text_dim, ..plain.clone() });
        }
        // split on ** : odd pieces are bold
        for (i, piece) in body.split("**").enumerate() {
            if piece.is_empty() {
                continue;
            }
            let fmt = if heading || i % 2 == 1 { bold.clone() } else { plain.clone() };
            job.append(piece, 0.0, fmt);
        }
        ui.horizontal(|ui| {
            ui.add_space(indent);
            // `wrap()`: rows don't wrap by default, so a long line would run off the panel
            ui.add(egui::Label::new(job).wrap());
        });
    }
}

/// A question from the Director (a clip past the credit cap, or any change in Ask mode), pinned
/// right above the prompt box so it is never scrolled away or hidden behind the plan.
fn ask_card(app: &mut FilmcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let (ask, credits) = {
        let s = lock(&app.jbd.director.shared);
        (s.ask.clone(), s.credits)
    };
    let Some(ask) = ask else { return };
    let mut answer = None;
    egui::Frame::new().fill(t.field_bg).stroke(Stroke::new(1.0, style::BLUE)).corner_radius(10.0).inner_margin(10.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(ask.title.as_str()).font(Tokens::semibold(12.5)).color(t.text));
        ui.add(egui::Label::new(RichText::new(ask.detail.as_str()).size(11.5).color(t.text_dim)).truncate());
        ui.horizontal(|ui| {
            let yes_label = if ask.credits > 0 { "Generate" } else { "Allow" };
            let yes = style::small_primary_button(ui, yes_label, t);
            let no = ui.add(egui::Button::new(RichText::new("Skip").size(12.0).color(t.text_dim)).frame(false));
            app.auto.add("director.allow", yes.rect, yes_label);
            app.auto.add("director.deny", no.rect, "Skip");
            if yes.clicked() {
                answer = Some(true);
            }
            if no.clicked() {
                answer = Some(false);
            }
            if ask.credits > 0 {
                let left = credits.map(|c| format!(" · {} left", style::thousands(c.left()))).unwrap_or_default();
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(format!("{} credits{left}", ask.credits)).size(11.0).color(t.text_faint));
                });
            }
        });
    });
    if let Some(yes) = answer {
        app.jbd.director.answer(yes);
    }
}

/// A pasted text waiting to be sent: "Pasted text · 23k chars", × on hover to drop it. Returns
/// true when its × was clicked.
fn pasted_chip(ui: &mut egui::Ui, chars: usize, t: &Tokens) -> bool {
    let size = if chars >= 1000 { format!("{}k chars", chars.div_ceil(1000)) } else { format!("{chars} chars") };
    let (rect, resp) = ui.allocate_exact_size(vec2(120.0, 52.0), Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 6.0, t.hover);
    p.rect_stroke(rect, 6.0, Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
    p.text(rect.min + vec2(9.0, 10.0), egui::Align2::LEFT_TOP, "Pasted text", Tokens::semibold(11.5), t.text);
    p.text(rect.min + vec2(9.0, 28.0), egui::Align2::LEFT_TOP, size, Tokens::ui(10.5), t.text_faint);
    let mut removed = false;
    if resp.hovered() {
        let x = Rect::from_center_size(pos2(rect.max.x - 10.0, rect.min.y + 10.0), vec2(14.0, 14.0));
        let s = Stroke::new(1.3, t.text_dim);
        ui.painter().line_segment([x.center() + vec2(-3.0, -3.0), x.center() + vec2(3.0, 3.0)], s);
        ui.painter().line_segment([x.center() + vec2(3.0, -3.0), x.center() + vec2(-3.0, 3.0)], s);
        removed = resp.clicked() && ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| x.expand(4.0).contains(p));
    }
    resp.on_hover_text("Sent with your message. × removes it.");
    removed
}

/// A picture attached to a sent message, in the chat: 72×72, no ×, the name on hover.
fn picture_tile(ui: &mut egui::Ui, name: &str, thumb: Option<(egui::TextureId, egui::Vec2)>, t: &Tokens) {
    let (rect, resp) = ui.allocate_exact_size(vec2(72.0, 72.0), Sense::hover());
    paint_thumb(ui.painter(), rect, name, thumb, t);
    resp.on_hover_text(name);
}

/// A picture cropped to fill `rect` (or the start of its name when there's no picture yet).
fn paint_thumb(p: &egui::Painter, rect: Rect, name: &str, thumb: Option<(egui::TextureId, egui::Vec2)>, t: &Tokens) {
    p.rect_filled(rect, 6.0, t.hover);
    match thumb {
        Some((tex, size)) if size.x > 0.0 && size.y > 0.0 => {
            let a = size.x / size.y;
            let uv = if a > 1.0 {
                let m = (1.0 - 1.0 / a) / 2.0;
                Rect::from_min_max(pos2(m, 0.0), pos2(1.0 - m, 1.0))
            } else {
                let m = (1.0 - a) / 2.0;
                Rect::from_min_max(pos2(0.0, m), pos2(1.0, 1.0 - m))
            };
            p.image(tex, rect.shrink(1.0), uv, Color32::WHITE);
        }
        _ => no_picture(p, rect, name, t),
    }
    p.rect_stroke(rect, 6.0, Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
}

/// A tile without a picture (audio, a title or graphic, a preview not ready yet). `name` is
/// "V2 · 17.6–20.0 s\nJUST DO IT." for a timeline clip, or a file name: the track goes small
/// at the top and the start of the name in the middle.
fn no_picture(p: &egui::Painter, rect: Rect, name: &str, t: &Tokens) {
    let (head, body) = match name.split_once('\n') {
        Some((label, n)) => (label.split(" · ").next().unwrap_or(""), n),
        None => ("", name),
    };
    if !head.is_empty() {
        p.text(rect.min + vec2(6.0, 5.0), egui::Align2::LEFT_TOP, head, Tokens::semibold(9.0), t.text_faint);
    }
    p.text(
        rect.center() + vec2(0.0, if head.is_empty() { 0.0 } else { 4.0 }),
        egui::Align2::CENTER_CENTER,
        jbd_director::tools::clip(body.trim(), 9),
        Tokens::ui(9.5),
        t.text_dim,
    );
}

/// One attachment preview: 52×52, the picture cropped to fill. Returns true when its × was
/// clicked.
fn attachment_tile(ui: &mut egui::Ui, name: &str, thumb: Option<(egui::TextureId, egui::Vec2)>, t: &Tokens) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(52.0, 52.0), Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 6.0, t.hover);
    match thumb {
        Some((tex, size)) if size.x > 0.0 && size.y > 0.0 => {
            // crop the middle so the tile is filled whatever the shape of the picture
            let a = size.x / size.y;
            let uv = if a > 1.0 {
                let m = (1.0 - 1.0 / a) / 2.0;
                Rect::from_min_max(pos2(m, 0.0), pos2(1.0 - m, 1.0))
            } else {
                let m = (1.0 - a) / 2.0;
                Rect::from_min_max(pos2(0.0, m), pos2(1.0, 1.0 - m))
            };
            p.image(tex, rect.shrink(1.0), uv, Color32::WHITE);
        }
        _ => no_picture(p, rect, name, t),
    }
    p.rect_stroke(rect, 6.0, Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
    let mut removed = false;
    if resp.hovered() {
        let c = pos2(rect.right() - 8.0, rect.top() + 8.0);
        let x_rect = Rect::from_center_size(c, vec2(14.0, 14.0));
        p.circle_filled(c, 7.0, Color32::from_black_alpha(200));
        let s = Stroke::new(1.3, Color32::WHITE);
        p.line_segment([c + vec2(-3.0, -3.0), c + vec2(3.0, 3.0)], s);
        p.line_segment([c + vec2(3.0, -3.0), c + vec2(-3.0, 3.0)], s);
        removed = resp.clicked() && ui.input(|i| i.pointer.interact_pos().is_some_and(|q| x_rect.contains(q)));
    }
    resp.on_hover_text(name);
    removed
}
