//! Design tokens. Every widget reads colours/sizes from [`Tokens`] so themes apply everywhere.
//!
//! The default theme is JBD's black-and-white Jabrod look (see the dark tokens in
//! [`Tokens::for_kind`]); the Medium and Light variants derive from it.
//!
//! JBD's type system, the same as jabrod.com (all SIL OFL 1.1, in `assets/jbd/fonts/`):
//!
//! | Use | Font | Helper |
//! |---|---|---|
//! | Everything in the UI | Geist Regular / Medium / SemiBold | [`Tokens::ui`], [`Tokens::medium`], [`Tokens::semibold`] |
//! | Timecodes, numbers in columns | Geist Mono | [`Tokens::mono`] |
//! | The JBD wordmark, big titles | Archivo Black | [`Tokens::display`] |
//!
//! The scale: 12 pt body and controls, 13 pt for the header (mode tabs, menus, title), 11 pt
//! small print, 15 pt dialog headings. Don't invent sizes in panels: use these.

use std::sync::Arc;

use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, Visuals};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeKind {
    /// JBD black (default).
    #[default]
    Dark,
    /// Slightly lighter grey panels (Premiere's brightness slider mid position).
    Medium,
    Light,
}

impl ThemeKind {
    /// Settings ▸ Appearance ▸ Color Theme value (`darkest`, `dark`, `light`).
    pub fn pref_name(self) -> &'static str {
        match self {
            ThemeKind::Dark => "darkest",
            ThemeKind::Medium => "dark",
            ThemeKind::Light => "light",
        }
    }
    pub fn from_pref(s: &str) -> ThemeKind {
        match s {
            "dark" => ThemeKind::Medium,
            "light" => ThemeKind::Light,
            _ => ThemeKind::Dark,
        }
    }
    pub fn from_name(s: &str) -> Option<ThemeKind> {
        match s.to_ascii_lowercase().as_str() {
            "dark" | "darkest" => Some(ThemeKind::Dark),
            "medium" | "grey" | "gray" => Some(ThemeKind::Medium),
            "light" => Some(ThemeKind::Light),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    pub kind: ThemeKind,
    /// Space between panels / app background.
    pub app_bg: Color32,
    /// Header bar (top).
    pub header_bg: Color32,
    /// Panel body.
    pub panel_bg: Color32,
    /// Panel tab strip.
    pub tab_bg: Color32,
    pub tab_text: Color32,
    pub tab_text_active: Color32,
    /// Blue focus rectangle around the active panel.
    pub focus: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    pub icon: Color32,
    pub icon_active: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub field_bg: Color32,
    pub field_border: Color32,
    pub separator: Color32,
    pub row_alt: Color32,
    pub row_selected: Color32,
    /// Scrubby (hot-text) numeric value colour.
    pub hot_text: Color32,
    // timeline
    pub tl_bg: Color32,
    pub tl_track_bg: Color32,
    pub tl_track_bg_alt: Color32,
    pub tl_header_bg: Color32,
    pub tl_ruler_bg: Color32,
    pub tl_ruler_tick: Color32,
    pub tl_ruler_text: Color32,
    pub playhead: Color32,
    pub in_out_shade: Color32,
    pub clip_selected_border: Color32,
    pub render_red: Color32,
    pub render_yellow: Color32,
    pub render_green: Color32,
    pub monitor_bg: Color32,
    pub timecode: Color32,
    pub danger: Color32,
    pub radius: f32,
    pub radius_sm: f32,
    pub gap: f32,
    pub tab_h: f32,
}

impl Tokens {
    pub fn for_kind(kind: ThemeKind) -> Self {
        // JBD's black-and-white theme (the Jabrod tokens from the old app's src/styles.css).
        // Greys carry the UI; the logo's violet / blue / cyan appear only where something needs
        // to be seen: selection and focus (blue), values you can drag and timecode (cyan).
        // JBD palette, named once so the token list below reads like the old CSS:
        let black = Color32::from_rgb(0x0b, 0x0b, 0x0c); // --jbd-black: the window background
        let panel = Color32::from_rgb(0x11, 0x11, 0x13); // --jbd-panel
        let raised = Color32::from_rgb(0x18, 0x18, 0x1b); // --jbd-raised: inputs, cards, menus
        let hover = Color32::from_rgb(0x23, 0x23, 0x27); // --jbd-hover
        let line = Color32::from_rgb(0x26, 0x26, 0x2b); // --jbd-line: hairlines
        let line_strong = Color32::from_rgb(0x3b, 0x3b, 0x42); // --jbd-line-strong
        let white = Color32::from_rgb(0xf4, 0xf4, 0xf5); // --jbd-white: main text, the playhead
        let grey = Color32::from_rgb(0x9b, 0x9b, 0xa3); // --jbd-grey: secondary text
        let faint = Color32::from_rgb(0x5f, 0x5f, 0x68); // --jbd-faint: hints, disabled
        let blue = Color32::from_rgb(0x3d, 0x5c, 0xff); // --jbd-blue: the logo's middle
        let cyan = Color32::from_rgb(0x00, 0xd4, 0xfa); // --jbd-cyan: the logo's timeline face
        let dark = Tokens {
            kind,
            app_bg: black,
            header_bg: black,
            panel_bg: panel,
            tab_bg: panel,
            tab_text: grey,
            tab_text_active: white,
            focus: line_strong, // the focused panel's frame: a quiet hairline
            accent: blue,
            accent_hover: Color32::from_rgb(0x56, 0x72, 0xff),
            text: white,
            text_dim: grey,
            text_faint: faint,
            icon: grey,
            icon_active: white,
            hover,
            pressed: line_strong,
            field_bg: raised,
            field_border: line_strong,
            separator: line,
            row_alt: Color32::from_rgb(0x15, 0x15, 0x17),
            row_selected: hover,
            hot_text: cyan,
            tl_bg: panel,
            tl_track_bg: panel,
            tl_track_bg_alt: panel,
            tl_header_bg: panel,
            tl_ruler_bg: panel,
            tl_ruler_tick: faint,
            tl_ruler_text: grey,
            playhead: white,
            in_out_shade: Color32::from_rgb(0x2a, 0x2a, 0x30),
            clip_selected_border: white,
            render_red: Color32::from_rgb(0xff, 0x3b, 0x3b),    // --jbd-red
            render_yellow: Color32::from_rgb(0xf5, 0xb4, 0x00), // --jbd-amber
            render_green: Color32::from_rgb(0x16, 0xc9, 0x6f),  // --jbd-green
            monitor_bg: black,
            timecode: cyan,
            danger: Color32::from_rgb(0xff, 0x3b, 0x3b),
            radius: 0.0,
            radius_sm: 6.0,
            gap: 4.0,
            tab_h: 32.0,
        };
        match kind {
            ThemeKind::Dark => dark,
            ThemeKind::Medium => Tokens {
                app_bg: Color32::from_rgb(20, 20, 20),
                header_bg: Color32::from_rgb(0x32, 0x32, 0x32),
                panel_bg: Color32::from_rgb(0x32, 0x32, 0x32),
                tab_bg: Color32::from_rgb(0x32, 0x32, 0x32),
                field_bg: Color32::from_rgb(0x22, 0x22, 0x22),
                tl_bg: Color32::from_rgb(0x32, 0x32, 0x32),
                tl_track_bg: Color32::from_rgb(0x32, 0x32, 0x32),
                tl_track_bg_alt: Color32::from_rgb(0x32, 0x32, 0x32),
                tl_header_bg: Color32::from_rgb(0x32, 0x32, 0x32),
                tl_ruler_bg: Color32::from_rgb(0x32, 0x32, 0x32),
                monitor_bg: Color32::from_rgb(0x32, 0x32, 0x32),
                row_alt: Color32::from_rgb(50, 50, 50),
                hover: Color32::from_rgb(64, 64, 64),
                ..dark
            },
            ThemeKind::Light => Tokens {
                app_bg: Color32::from_rgb(180, 180, 180),
                header_bg: Color32::from_rgb(214, 214, 214),
                panel_bg: Color32::from_rgb(232, 232, 232),
                tab_bg: Color32::from_rgb(232, 232, 232),
                tab_text: Color32::from_rgb(90, 90, 90),
                tab_text_active: Color32::from_rgb(20, 20, 20),
                text: Color32::from_rgb(34, 34, 34),
                text_dim: Color32::from_rgb(90, 90, 90),
                text_faint: Color32::from_rgb(140, 140, 140),
                icon: Color32::from_rgb(60, 60, 60),
                hover: Color32::from_rgb(210, 210, 210),
                pressed: Color32::from_rgb(196, 196, 196),
                field_bg: Color32::from_rgb(250, 250, 250),
                field_border: Color32::from_rgb(170, 170, 170),
                separator: Color32::from_rgb(200, 200, 200),
                row_alt: Color32::from_rgb(224, 224, 224),
                row_selected: Color32::from_rgb(170, 200, 240),
                tl_bg: Color32::from_rgb(210, 210, 210),
                tl_track_bg: Color32::from_rgb(222, 222, 222),
                tl_track_bg_alt: Color32::from_rgb(228, 228, 228),
                tl_header_bg: Color32::from_rgb(214, 214, 214),
                tl_ruler_bg: Color32::from_rgb(226, 226, 226),
                tl_ruler_text: Color32::from_rgb(80, 80, 80),
                ..dark
            },
        }
    }

    /// Settings ▸ Appearance on top of a theme: the highlight colour (selections, focus, primary
    /// buttons) and accessible colour contrast (brighter secondary text and borders).
    pub fn with_appearance(mut self, highlight: Option<[u8; 3]>, accessible_contrast: bool) -> Self {
        if let Some([r, g, b]) = highlight {
            let c = Color32::from_rgb(r, g, b);
            let lift = |v: u8| v.saturating_add(16);
            self.accent = c;
            self.accent_hover = Color32::from_rgb(lift(r), lift(g), lift(b));
        }
        if accessible_contrast {
            if self.kind == ThemeKind::Light {
                self.text_dim = Color32::from_rgb(40, 40, 40);
                self.text_faint = Color32::from_rgb(80, 80, 80);
                self.field_border = Color32::from_rgb(110, 110, 110);
            } else {
                self.text_dim = self.text;
                self.text_faint = Color32::from_rgb(0xa8, 0xa8, 0xa8);
                self.tab_text = self.tab_text_active;
                self.field_border = Color32::from_rgb(0x6a, 0x6a, 0x6a);
                self.separator = Color32::from_rgb(0x50, 0x50, 0x50);
            }
        }
        self
    }

    /// Mono font for timecode.
    pub fn mono(size: f32) -> FontId {
        FontId::new(size, FontFamily::Monospace)
    }
    pub fn ui(size: f32) -> FontId {
        FontId::new(size, FontFamily::Proportional)
    }
    pub fn medium(size: f32) -> FontId {
        FontId::new(size, FontFamily::Name("medium".into()))
    }
    pub fn semibold(size: f32) -> FontId {
        FontId::new(size, FontFamily::Name("semibold".into()))
    }
    /// Archivo Black: the JBD wordmark and big display titles only.
    pub fn display(size: f32) -> FontId {
        FontId::new(size, FontFamily::Name("display".into()))
    }
}

/// Every font family the theme defines (fallback fonts such as the Japanese fonts are added to each
/// of them).
pub fn font_families() -> Vec<FontFamily> {
    vec![
        FontFamily::Proportional,
        FontFamily::Monospace,
        FontFamily::Name("semibold".into()),
        FontFamily::Name("medium".into()),
        FontFamily::Name("display".into()),
    ]
}

const GEIST_REGULAR: &[u8] = include_bytes!("../../../assets/jbd/fonts/Geist-Regular.ttf");
const GEIST_MEDIUM: &[u8] = include_bytes!("../../../assets/jbd/fonts/Geist-Medium.ttf");
const GEIST_SEMIBOLD: &[u8] = include_bytes!("../../../assets/jbd/fonts/Geist-SemiBold.ttf");
const GEIST_MONO: &[u8] = include_bytes!("../../../assets/jbd/fonts/GeistMono-Regular.ttf");
const ARCHIVO_BLACK: &[u8] = include_bytes!("../../../assets/jbd/fonts/ArchivoBlack-Regular.ttf");

/// Install fonts (Geist, Geist Mono, Archivo Black; Inter stays as a fallback for characters
/// Geist lacks; then the Japanese craft-fonts when built with `CRAFT_FONTS_DIR`) and egui visuals.
pub fn install(ctx: &egui::Context, t: &Tokens) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("geist".into(), Arc::new(FontData::from_static(GEIST_REGULAR)));
    fonts.font_data.insert("geist-medium".into(), Arc::new(FontData::from_static(GEIST_MEDIUM)));
    fonts.font_data.insert("geist-semibold".into(), Arc::new(FontData::from_static(GEIST_SEMIBOLD)));
    fonts.font_data.insert("geist-mono".into(), Arc::new(FontData::from_static(GEIST_MONO)));
    fonts.font_data.insert("archivo-black".into(), Arc::new(FontData::from_static(ARCHIVO_BLACK)));
    fonts.font_data.insert("inter".into(), Arc::new(FontData::from_static(filmcraft_text::fonts::INTER_REGULAR)));
    fonts.font_data.insert("jbmono".into(), Arc::new(FontData::from_static(filmcraft_text::fonts::JETBRAINS_MONO_REGULAR)));
    {
        let prop = fonts.families.entry(FontFamily::Proportional).or_default();
        prop.insert(0, "geist".into());
        prop.insert(1, "inter".into());
    }
    {
        let mono = fonts.families.entry(FontFamily::Monospace).or_default();
        mono.insert(0, "geist-mono".into());
        mono.insert(1, "jbmono".into());
    }
    fonts.families.insert(FontFamily::Name("semibold".into()), vec!["geist-semibold".into(), "geist".into(), "inter".into()]);
    fonts.families.insert(FontFamily::Name("medium".into()), vec!["geist-medium".into(), "geist".into(), "inter".into()]);
    fonts.families.insert(FontFamily::Name("display".into()), vec!["archivo-black".into(), "geist-semibold".into(), "inter".into()]);
    add_craft_fonts(&mut fonts);
    ctx.set_fonts(fonts);
    apply_visuals(ctx, t);
}

/// Name of the egui font for a craft-fonts entry.
fn craft_font_name(f: &filmcraft_text::fonts::CraftFont) -> String {
    format!("craft:{} {}", f.family, f.style)
}

/// Append the Japanese craft-fonts (empty unless built with `CRAFT_FONTS_DIR`) as the last fallbacks
/// of every font family, after the app's own fonts: BIZ UDPGothic first (bold before regular in
/// the "semibold" and "medium" families), then the other Japanese faces.
fn add_craft_fonts(fonts: &mut FontDefinitions) {
    let jpan: Vec<_> = filmcraft_text::fonts::craft_japanese().collect();
    for f in &jpan {
        fonts.font_data.insert(craft_font_name(f), Arc::new(FontData::from_static(f.bytes)));
    }
    for (family, stack) in fonts.families.iter_mut() {
        let heavy = matches!(family, FontFamily::Name(n) if matches!(n.as_ref(), "semibold" | "medium" | "display"));
        let mut order = jpan.clone();
        order.sort_by_key(|f| (!f.family.contains("Gothic"), (f.style == "Bold") != heavy));
        stack.extend(order.iter().map(|f| craft_font_name(f)));
    }
}

pub fn apply_visuals(ctx: &egui::Context, t: &Tokens) {
    let mut v = if t.kind == ThemeKind::Light { Visuals::light() } else { Visuals::dark() };
    v.panel_fill = t.panel_bg;
    v.window_fill = t.panel_bg;
    v.extreme_bg_color = t.field_bg;
    v.faint_bg_color = t.row_alt;
    v.override_text_color = Some(t.text);
    v.selection.bg_fill = t.accent;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.hyperlink_color = t.accent;
    v.window_stroke = Stroke::new(1.0, t.field_border);
    v.window_corner_radius = egui::CornerRadius::same(6);
    v.menu_corner_radius = egui::CornerRadius::same(6);
    v.popup_shadow = egui::epaint::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(140) };
    v.window_shadow = v.popup_shadow;
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = egui::CornerRadius::same(t.radius_sm as u8);
    }
    v.widgets.noninteractive.bg_fill = t.panel_bg;
    v.widgets.noninteractive.weak_bg_fill = t.panel_bg;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.separator);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.inactive.bg_fill = t.field_bg;
    v.widgets.inactive.weak_bg_fill = t.field_bg;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, t.field_border);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.hovered.bg_fill = t.hover;
    v.widgets.hovered.weak_bg_fill = t.hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, t.field_border);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, t.tab_text_active);
    v.widgets.active.bg_fill = t.pressed;
    v.widgets.active.weak_bg_fill = t.pressed;
    v.widgets.active.fg_stroke = Stroke::new(1.0, t.tab_text_active);
    v.widgets.open.bg_fill = t.hover;
    v.widgets.open.weak_bg_fill = t.hover;
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(8.0, 3.0);
        s.spacing.interact_size.y = 24.0;
        s.spacing.menu_margin = egui::Margin::same(4);
        s.text_styles.insert(TextStyle::Body, FontId::new(12.0, FontFamily::Proportional));
        s.text_styles.insert(TextStyle::Button, FontId::new(12.0, FontFamily::Proportional));
        s.text_styles.insert(TextStyle::Small, FontId::new(11.0, FontFamily::Proportional));
        s.text_styles.insert(TextStyle::Heading, FontId::new(15.0, FontFamily::Name("semibold".into())));
        s.text_styles.insert(TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace));
        s.animation_time = 0.12;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const JAPANESE: &str = "日本語の文字";

    fn installed() -> egui::Context {
        let ctx = egui::Context::default();
        install(&ctx, &Tokens::for_kind(ThemeKind::default()));
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
        out.textures_delta.clear();
        ctx
    }

    /// Built with craft-fonts: every font family ends with the Japanese faces (BIZ UDPGothic
    /// first), and Japanese text gets real glyphs, not the replacement box.
    #[test]
    fn japanese_renders_with_craft_fonts() {
        if filmcraft_text::fonts::craft_japanese().next().is_none() {
            eprintln!("SKIPPED: built without craft-fonts (set CRAFT_FONTS_DIR to run)");
            return;
        }
        let ctx = installed();
        ctx.fonts_mut(|fonts| {
            let defs = fonts.definitions().clone();
            for (family, stack) in &defs.families {
                let first = stack.iter().position(|n| n.starts_with("craft:")).unwrap_or(stack.len());
                assert!(first > 0 && stack[first..].iter().all(|n| n.starts_with("craft:")), "{family:?}: {stack:?}");
                assert!(stack[first].starts_with("craft:BIZ UDPGothic"), "{family:?}: {stack:?}");
            }
            for family in [FontFamily::Proportional, FontFamily::Monospace] {
                let font = FontId::new(13.0, family);
                for ch in JAPANESE.chars() {
                    assert!(fonts.has_glyph(&font, ch), "missing {ch} in {font:?}");
                }
            }
        });
        // laid out and drawn: the replacement box (U+FFFD) would show as one glyph per character
        // from the fallback face; a real face gives each character its own width
        let galley = ctx.fonts_mut(|f| f.layout_no_wrap(JAPANESE.into(), FontId::new(13.0, FontFamily::Proportional), Color32::WHITE));
        assert_eq!(galley.rows.iter().map(|r| r.glyphs.len()).sum::<usize>(), JAPANESE.chars().count());
        assert!(galley.size().x > 13.0 * 4.0, "{:?}", galley.size());
    }

    /// Built without craft-fonts: the theme installs only the app's own fonts and Latin text
    /// works as before.
    #[test]
    fn works_without_craft_fonts() {
        let ctx = installed();
        let n = filmcraft_text::fonts::craft_japanese().count();
        ctx.fonts_mut(|fonts| {
            let defs = fonts.definitions().clone();
            assert_eq!(defs.font_data.keys().filter(|k| k.starts_with("craft:")).count(), n);
            assert_eq!(defs.families[&FontFamily::Proportional].first().map(String::as_str), Some("geist"));
            assert_eq!(defs.families[&FontFamily::Monospace].first().map(String::as_str), Some("geist-mono"));
            assert!(fonts.has_glyph(&FontId::new(13.0, FontFamily::Proportional), 'A'));
            if n == 0 {
                assert!(defs.families.values().flatten().all(|name| !name.starts_with("craft:")));
            }
        });
    }
}
