//! A short, plain-text picture of the project, sent with every request.
//!
//! The raw `sequence.inspect` answer is long (every effect parameter of every clip: ~18 KB for
//! the demo) and in ticks. The model needs the shape: which timeline is open, how big, which
//! clips are where in seconds, and what media there is. Without it, it guesses (and once called
//! a full timeline "empty").
//!
//! Built from two read-only queries; nothing here changes the project. Change what the Director
//! sees about the project here.

use serde_json::Value;

/// Ticks per second (`filmcraft_time::Tick`).
const TICKS: f64 = 254_016_000_000.0;

/// Longest digest; past this, clips and media are summarised by count.
const MAX: usize = 7_000;

fn secs(v: Option<&Value>) -> f64 {
    v.and_then(Value::as_f64).map(|t| t / TICKS).unwrap_or(0.0)
}

/// The digest, from `project.inspect` and `sequence.inspect` results (either may be missing).
pub fn digest(project: Option<&Value>, sequence: Option<&Value>) -> String {
    let mut out = String::from("PROJECT NOW (times in seconds; ids are what commands take)\n");
    if let Some(p) = project {
        let name = p.get("name").and_then(Value::as_str).unwrap_or("Untitled");
        let saved = if p.get("dirty").and_then(Value::as_bool).unwrap_or(false) { "unsaved changes" } else { "saved" };
        out.push_str(&format!("Project: {name} ({saved})\n"));
    }
    match sequence {
        Some(s) if s.get("id").is_some() => sequence_part(s, &mut out),
        _ => out.push_str("Open timeline: none. Create one with file.newSequence before placing clips.\n"),
    }
    if let Some(p) = project {
        out.push_str("Media:\n");
        let mut items = Vec::new();
        if let Some(root) = p.get("root") {
            collect_items(root, "", &mut items);
        }
        if items.is_empty() {
            out.push_str("  (no media yet)\n");
        }
        let total = items.len();
        for (i, line) in items.into_iter().enumerate() {
            if out.len() > MAX {
                out.push_str(&format!("  … and {} more items\n", total - i));
                break;
            }
            out.push_str(&line);
        }
    }
    out
}

fn sequence_part(s: &Value, out: &mut String) {
    let set = s.get("settings");
    let w = set.and_then(|x| x.get("width")).and_then(Value::as_u64).unwrap_or(0);
    let h = set.and_then(|x| x.get("height")).and_then(Value::as_u64).unwrap_or(0);
    let fps = set.and_then(|x| x.get("frame_rate")).and_then(|r| Some(r.get("num")?.as_f64()? / r.get("den")?.as_f64().filter(|d| *d > 0.0)?)).unwrap_or(0.0);
    let aspect = if w > 0 && h > 0 {
        match (w * 100 / h.max(1)) as u32 {
            170..=180 => " (16:9)",
            55..=57 => " (9:16)",
            99..=101 => " (1:1)",
            79..=81 => " (4:5)",
            _ => "",
        }
    } else {
        ""
    };
    out.push_str(&format!(
        "Open timeline: \"{}\" (id {}), {w}x{h}{aspect}, {fps:.3} fps, {:.1} s long, playhead at {:.1} s\n",
        s.get("name").and_then(Value::as_str).unwrap_or("Sequence"),
        s.get("id").cloned().unwrap_or(Value::Null),
        secs(s.get("duration")),
        secs(s.get("playhead")),
    ));
    let mut any = false;
    for (kind, key) in [("Video", "video"), ("Audio", "audio")] {
        for track in s.get(key).and_then(Value::as_array).into_iter().flatten() {
            let name = track.get("name").and_then(Value::as_str).unwrap_or(kind);
            let items = track.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
            out.push_str(&format!(
                "  {name} (track id {}): {} clip{}\n",
                track.get("id").cloned().unwrap_or(Value::Null),
                items.len(),
                if items.len() == 1 { "" } else { "s" }
            ));
            for (i, c) in items.iter().enumerate() {
                if out.len() > MAX {
                    out.push_str(&format!("    … and {} more clips\n", items.len() - i));
                    break;
                }
                any = true;
                let fx = c.get("effects").and_then(Value::as_array).map(|e| e.len()).unwrap_or(0);
                out.push_str(&format!(
                    "    clip {}: \"{}\" {:.1}–{:.1} s (item {}){}{}\n",
                    c.get("clip").cloned().unwrap_or(Value::Null),
                    c.get("name").and_then(Value::as_str).unwrap_or("?"),
                    secs(c.get("start")),
                    secs(c.get("end")),
                    c.get("item").cloned().unwrap_or(Value::Null),
                    if c.get("speed").and_then(Value::as_f64).is_some_and(|s| (s - 1.0).abs() > 1e-6) { " speed changed" } else { "" },
                    if fx > 3 { format!(", {} effects", fx) } else { String::new() },
                ));
            }
        }
    }
    if !any {
        out.push_str("  The timeline has no clips yet.\n");
    }
}

fn collect_items(node: &Value, bin: &str, out: &mut Vec<String>) {
    for child in node.get("children").and_then(Value::as_array).into_iter().flatten() {
        if child.get("children").is_some() {
            let name = child.get("name").and_then(Value::as_str).unwrap_or("Bin");
            collect_items(child, name, out);
        } else if let Some(item) = child.get("item") {
            let kind = child.get("type").and_then(Value::as_str).unwrap_or("Item");
            let dur = child.get("duration").and_then(Value::as_f64).map(|d| format!(", {:.1} s", d / TICKS)).unwrap_or_default();
            let place = if bin.is_empty() { String::new() } else { format!(" in {bin}") };
            out.push(format!("  item {item}: \"{}\" ({kind}{dur}){place}\n", child.get("name").and_then(Value::as_str).unwrap_or("?")));
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_full_timeline_reads_as_full() {
        let seq = json!({
            "id": 21, "name": "Main Edit", "duration": 254_016_000_000_i64 * 26, "playhead": 0,
            "settings": {"width": 1920, "height": 1080, "frame_rate": {"num": 24000, "den": 1001}},
            "video": [{"id": 3, "name": "Video 1", "items": [
                {"clip": 29, "name": "Ocean_Sunset.mp4", "start": 0, "end": 254_016_000_000_i64 * 5, "item": 5, "speed": 1.0, "effects": []}
            ]}],
            "audio": [{"id": 18, "name": "Audio 1", "items": []}]
        });
        let proj = json!({"name": "JBD Demo", "dirty": false, "root": {"children": [
            {"name": "Footage", "children": [{"item": 5, "name": "Ocean_Sunset.mp4", "type": "Movie", "duration": 254_016_000_000_i64 * 18}]}
        ]}});
        let d = digest(Some(&proj), Some(&seq));
        assert!(d.contains("\"Main Edit\" (id 21), 1920x1080 (16:9), 23.976 fps, 26.0 s long"), "{d}");
        assert!(d.contains("clip 29: \"Ocean_Sunset.mp4\" 0.0–5.0 s (item 5)"), "{d}");
        assert!(d.contains("item 5: \"Ocean_Sunset.mp4\" (Movie, 18.0 s) in Footage"), "{d}");
        assert!(!d.contains("no clips yet"));
    }

    #[test]
    fn no_timeline_says_how_to_make_one() {
        let d = digest(Some(&json!({"name": "Untitled", "root": {"children": []}})), None);
        assert!(d.contains("file.newSequence"));
        assert!(d.contains("no media yet"));
    }

    #[test]
    fn a_huge_project_is_capped() {
        let items: Vec<Value> = (0..2000).map(|i| json!({"item": i, "name": format!("clip {i}.mp4"), "type": "Movie"})).collect();
        let d = digest(Some(&json!({"root": {"children": items}})), None);
        assert!(d.len() < MAX + 200);
        assert!(d.contains("more items"));
    }
}
