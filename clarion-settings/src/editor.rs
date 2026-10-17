use eframe::egui::{Color32, FontId, TextFormat};
use eframe::epaint::text::LayoutJob;

pub fn highlight_kdl(text: &str) -> LayoutJob {
    let mut job = LayoutJob::default();

    let font_id = FontId::monospace(13.0);
    // Syntax colors follow the selected palette: the light field and pale-blue
    // selection need darker inks, while the dark field needs luminous ones.
    let dark = crate::theme::colors::is_dark();
    let default_color = crate::theme::colors::text();
    let (keyword_color, prop_color, string_color, number_color, bool_color) = if dark {
        (
            Color32::from_rgb(255, 122, 178),
            Color32::from_rgb(130, 194, 255),
            Color32::from_rgb(255, 138, 128),
            Color32::from_rgb(255, 184, 108),
            Color32::from_rgb(216, 146, 255),
        )
    } else {
        (
            Color32::from_rgb(148, 0, 148),
            Color32::from_rgb(0, 88, 150),
            Color32::from_rgb(163, 21, 21),
            Color32::from_rgb(112, 60, 0),
            Color32::from_rgb(123, 31, 162),
        )
    };
    let comment_color = crate::theme::colors::muted();

    let lines = text.split('\n');
    let total_lines = lines.clone().count();

    for (line_idx, line) in lines.enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with("/-") {
            // Entire line comment
            job.append(
                line,
                0.0,
                TextFormat {
                    font_id: font_id.clone(),
                    color: comment_color,
                    ..Default::default()
                },
            );
        } else {
            // Parse tokens
            let mut i = 0;
            let chars: Vec<char> = line.chars().collect();
            while i < chars.len() {
                // Inline comments
                if i + 1 < chars.len() && chars[i] == '/' && chars[i + 1] == '/' {
                    let comment_part: String = chars[i..].iter().collect();
                    job.append(
                        &comment_part,
                        0.0,
                        TextFormat {
                            font_id: font_id.clone(),
                            color: comment_color,
                            ..Default::default()
                        },
                    );
                    break;
                }

                // Strings
                if chars[i] == '"' {
                    let mut s = String::new();
                    s.push('"');
                    i += 1;
                    while i < chars.len() && chars[i] != '"' {
                        if chars[i] == '\\' && i + 1 < chars.len() {
                            s.push(chars[i]);
                            s.push(chars[i + 1]);
                            i += 2;
                        } else {
                            s.push(chars[i]);
                            i += 1;
                        }
                    }
                    if i < chars.len() {
                        s.push('"');
                        i += 1;
                    }
                    job.append(
                        &s,
                        0.0,
                        TextFormat {
                            font_id: font_id.clone(),
                            color: string_color,
                            ..Default::default()
                        },
                    );
                    continue;
                }

                // Whitespace
                if chars[i].is_whitespace() {
                    let mut ws = String::new();
                    while i < chars.len() && chars[i].is_whitespace() {
                        ws.push(chars[i]);
                        i += 1;
                    }
                    job.append(
                        &ws,
                        0.0,
                        TextFormat {
                            font_id: font_id.clone(),
                            color: default_color,
                            ..Default::default()
                        },
                    );
                    continue;
                }

                // Punctuation / Brackets
                if matches!(chars[i], '{' | '}' | ';' | '=' | '(' | ')') {
                    let c = chars[i].to_string();
                    job.append(
                        &c,
                        0.0,
                        TextFormat {
                            font_id: font_id.clone(),
                            color: crate::theme::colors::muted(),
                            ..Default::default()
                        },
                    );
                    i += 1;
                    continue;
                }

                // Words / Identifiers / Numbers
                let mut word = String::new();
                while i < chars.len()
                    && !chars[i].is_whitespace()
                    && !matches!(chars[i], '{' | '}' | ';' | '=' | '"' | '(' | ')')
                {
                    word.push(chars[i]);
                    i += 1;
                }

                let color = if matches!(
                    word.as_str(),
                    "input"
                        | "layout"
                        | "output"
                        | "binds"
                        | "gestures"
                        | "hot-corners"
                        | "window-rule"
                        | "layer-rule"
                        | "animations"
                        | "hotkey-overlay"
                        | "keyboard"
                        | "touchpad"
                        | "mouse"
                        | "trackpoint"
                        | "focus-ring"
                        | "border"
                        | "shadow"
                        | "struts"
                        | "xkb"
                ) {
                    keyword_color
                } else if matches!(word.as_str(), "true" | "false" | "off" | "on") {
                    bool_color
                } else if word
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == '.' || c == '-')
                {
                    number_color
                } else if matches!(
                    word.as_str(),
                    "width"
                        | "active-color"
                        | "inactive-color"
                        | "active-gradient"
                        | "inactive-gradient"
                        | "gaps"
                        | "center-focused-column"
                        | "always-center-single-column"
                        | "default-column-width"
                        | "top-left"
                        | "top-right"
                        | "bottom-left"
                        | "bottom-right"
                        | "tap"
                        | "natural-scroll"
                        | "accel-speed"
                        | "numlock"
                        | "spawn"
                        | "mode"
                        | "scale"
                        | "transform"
                        | "rotate-deg-per-sec"
                ) {
                    prop_color
                } else {
                    default_color
                };

                job.append(
                    &word,
                    0.0,
                    TextFormat {
                        font_id: font_id.clone(),
                        color,
                        ..Default::default()
                    },
                );
            }
        }

        if line_idx + 1 < total_lines {
            job.append(
                "\n",
                0.0,
                TextFormat {
                    font_id: font_id.clone(),
                    color: default_color,
                    ..Default::default()
                },
            );
        }
    }

    job
}

pub const SNIPPET_HOT_CORNERS: &str = r#"hot-corners {
    top-left
    top-right
    bottom-left
}
"#;

pub const SNIPPET_WINDOW_RULE: &str = r##"window-rule {
    match app-id=r#"^org\.gnome\."#
    default-column-width { proportion 0.5; }
    open-floating false
}
"##;

pub const SNIPPET_ANIMATED_BORDER: &str = r##"border {
    width 3
    active-gradient from="#ff007f" to="#00f0ff" angle=45 rotate-deg-per-sec=60
    inactive-color "#24283b"
}
"##;

pub const SNIPPET_KEYBIND: &str = r#"Mod+Shift+C { spawn "clarion-settings"; }
Mod+Shift+E { quit; }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn contrast(foreground: Color32, background: Color32) -> f32 {
        let luminance = |color: Color32| {
            let linear = |channel: u8| {
                let value = f32::from(channel) / 255.;
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
        };
        let a = luminance(foreground);
        let b = luminance(background);
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn all_highlight_classes_are_readable_on_light_editor_and_selection() {
        let source =
            "// 中文注释\nlayout { width 12.5; active-color \"#fff\"; custom true }\n/- ignored";
        let job = highlight_kdl(source);
        let mut unique_colors = std::collections::HashSet::new();
        for section in &job.sections {
            unique_colors.insert(section.format.color);
            for background in [
                crate::theme::colors::surface(),
                crate::theme::colors::background(),
                crate::theme::colors::accent_soft(),
            ] {
                assert!(
                    contrast(section.format.color, background) >= 4.5,
                    "{:?} on {background:?}: {}",
                    section.format.color,
                    &job.text[section.byte_range.clone()]
                );
            }
        }
        assert_eq!(
            unique_colors.len(),
            7,
            "exercise every syntax color, including punctuation/comments"
        );
        assert_eq!(job.text, source);
    }

    #[test]
    fn all_highlight_classes_are_readable_on_dark_editor_and_selection() {
        crate::theme::colors::set_dark(true);
        let source =
            "// 中文注释\nlayout { width 12.5; active-color \"#fff\"; custom true }\n/- ignored";
        let job = highlight_kdl(source);
        for section in &job.sections {
            for background in [
                crate::theme::colors::surface(),
                crate::theme::colors::background(),
                crate::theme::colors::accent_soft(),
            ] {
                assert!(
                    contrast(section.format.color, background) >= 4.5,
                    "{:?} on {background:?}: {}",
                    section.format.color,
                    &job.text[section.byte_range.clone()]
                );
            }
        }
        crate::theme::colors::set_dark(false);
        assert_eq!(job.text, source);
    }

    #[test]
    fn highlighting_preserves_unicode_escapes_comments_and_unfinished_input() {
        for source in [
            "",
            "\n\n",
            "output \"显示器\\\" A\" { scale 1.25 } // 注释\n",
            "window-rule { match app-id=r#\"^org\\.gnome\\.\"# }\n",
            "layout { name \"未完成的字符串",
            SNIPPET_HOT_CORNERS,
            SNIPPET_WINDOW_RULE,
            SNIPPET_ANIMATED_BORDER,
            SNIPPET_KEYBIND,
        ] {
            let job = highlight_kdl(source);
            assert_eq!(job.text, source);
            let mut end = 0;
            for section in job.sections {
                assert_eq!(section.byte_range.start, end);
                assert!(source.is_char_boundary(section.byte_range.start));
                assert!(source.is_char_boundary(section.byte_range.end));
                end = section.byte_range.end;
            }
            assert_eq!(end, source.len());
        }
    }
}
