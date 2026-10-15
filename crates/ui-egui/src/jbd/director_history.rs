//! The Director's chat history: every conversation is saved, so you can go back to one and carry
//! on where you left off (its messages, steps, plan, and what the Director remembers).
//!
//! One JSON file per conversation in `<JBD data folder>/Director/` (on Windows
//! `%APPDATA%\JBD\Director\`), named by when it started. Saved when a request finishes and when
//! you switch chats; the newest 100 are kept.
//!
//! Used by `director_run.rs` (saving, loading) and `director.rs` (the History menu).

use std::path::PathBuf;

use jbd_director::{ChatMessage, Entry, Todo};
use serde::{Deserialize, Serialize};

/// How many conversations are kept (oldest go first).
const KEEP: usize = 100;

/// A saved conversation.
#[derive(Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    /// Unix seconds of the last change.
    pub updated: u64,
    pub entries: Vec<Entry>,
    #[serde(default)]
    pub todos: Vec<Todo>,
    /// What the Director remembers (the model's view of the conversation).
    #[serde(default)]
    pub memory: Vec<ChatMessage>,
}

/// One line of the History menu.
#[derive(Clone, Debug)]
pub struct Summary {
    pub id: String,
    pub title: String,
    pub updated: u64,
}

fn dir() -> Option<PathBuf> {
    filmcraft_engine::autosave::default_data_dir().map(|d| d.join("Director"))
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A new conversation id (sortable: when it started).
pub fn new_id() -> String {
    format!("{:012}", now())
}

/// The title a conversation shows: its first request, cut short.
pub fn title_of(entries: &[Entry]) -> String {
    entries
        .iter()
        .find_map(|e| match e {
            Entry::User(t) => Some(jbd_director::tools::clip(t.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or(t), 48)),
            _ => None,
        })
        .unwrap_or_else(|| "New chat".to_string())
}

/// Save (or overwrite) a conversation. Errors are returned for the caller to ignore or show:
/// a failed save must never interrupt editing.
pub fn save(c: &Conversation) -> Result<(), String> {
    let dir = dir().ok_or("no data folder")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let text = serde_json::to_string(c).map_err(|e| e.to_string())?;
    // write to a temporary file first, so a crash mid-write can't leave half a conversation
    let tmp = dir.join(format!("{}.json.tmp", c.id));
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dir.join(format!("{}.json", c.id))).map_err(|e| e.to_string())?;
    prune(&dir);
    Ok(())
}

pub fn load(id: &str) -> Option<Conversation> {
    // ids are digits only: nothing else may become a path
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let text = std::fs::read_to_string(dir()?.join(format!("{id}.json"))).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn delete(id: &str) {
    if let Some(dir) = dir().filter(|_| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit())) {
        let _ = std::fs::remove_file(dir.join(format!("{id}.json")));
    }
}

/// Saved conversations, newest first. Reads only the small header of each file's fields.
pub fn list() -> Vec<Summary> {
    #[derive(Deserialize)]
    struct Head {
        id: String,
        title: String,
        updated: u64,
    }
    let Some(dir) = dir() else { return Vec::new() };
    let Ok(read) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out: Vec<Summary> = read
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| serde_json::from_str::<Head>(&t).ok())
        .map(|h| Summary { id: h.id, title: h.title, updated: h.updated })
        .collect();
    out.sort_by_key(|s| std::cmp::Reverse(s.updated));
    out
}

fn prune(dir: &std::path::Path) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<PathBuf> = read.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
    if files.len() <= KEEP {
        return;
    }
    // names are start times, so sorting by name is oldest first
    files.sort();
    for f in files.iter().take(files.len() - KEEP) {
        let _ = std::fs::remove_file(f);
    }
}

/// "just now", "5 min ago", "3 h ago", "yesterday", "4 days ago".
pub fn ago(then: u64) -> String {
    let s = now().saturating_sub(then);
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86_399 => format!("{} h ago", s / 3600),
        86_400..=172_799 => "yesterday".into(),
        _ => format!("{} days ago", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_come_from_the_first_request() {
        let entries = vec![Entry::Said("hi".into()), Entry::User("Make a 20-second ad for the Nike Dunk Low Panda with five shots".into())];
        let t = title_of(&entries);
        assert!(t.starts_with("Make a 20-second ad"));
        assert!(t.chars().count() <= 48);
        assert_eq!(title_of(&[]), "New chat");
    }

    #[test]
    fn only_digit_ids_become_paths() {
        assert!(load("../../etc/passwd").is_none());
        assert!(load("").is_none());
    }

    #[test]
    fn ages_read_naturally() {
        assert_eq!(ago(now()), "just now");
        assert_eq!(ago(now() - 300), "5 min ago");
        assert_eq!(ago(now() - 90_000), "yesterday");
    }

    #[test]
    fn a_conversation_survives_a_round_trip_through_json() {
        let c = Conversation {
            id: "1".into(),
            title: "t".into(),
            updated: 5,
            entries: vec![Entry::User("a".into()), Entry::Said("b".into())],
            todos: vec![],
            memory: vec![ChatMessage::user("a")],
        };
        let back: Conversation = serde_json::from_str(&serde_json::to_string(&c).unwrap_or_default()).unwrap_or_else(|_| panic!("round trip"));
        assert_eq!(back.entries.len(), 2);
        assert_eq!(back.memory.len(), 1);
    }
}
