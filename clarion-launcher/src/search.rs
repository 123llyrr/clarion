// SPDX-License-Identifier: GPL-3.0-or-later
//! Toolkit-independent Spotlight ranking.
//!
//! The ladder mirrors the reference implementation (Clavis
//! `Common/functions/SpotlightLocalSearch.js`) so the palette and the
//! Launchpad rank identically: exact field matches beat prefixes, prefixes
//! beat word starts, word starts beat substrings, and substrings beat
//! subsequences. Usage history only breaks ties inside the same score.
use std::cmp::Ordering;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::catalog::DesktopApp;
use crate::usage::{usage_score, LaunchRecord};

/// Field weights: a name hit outranks a generic-name hit, which outranks a
/// keyword hit, which outranks a desktop-id hit.
pub const WEIGHT_NAME: i32 = 80;
pub const WEIGHT_GENERIC: i32 = 60;
pub const WEIGHT_KEYWORDS: i32 = 40;
pub const WEIGHT_ID: i32 = 20;

const EXACT: i32 = 5000;
const PREFIX: i32 = 4000;
const WORD_START: i32 = 3000;
const SUBSTRING: i32 = 2000;
const SUBSEQUENCE: i32 = 1000;

/// Application ordering offered by the palette. The preference stores the
/// same kebab-case values the slash commands use (`most-used`, ...).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppOrder {
    Smart,
    MostUsed,
    RecentlyUsed,
    #[default]
    Name,
}

impl AppOrder {
    pub fn from_value(value: &str) -> Self {
        match value {
            "smart" => Self::Smart,
            "most-used" => Self::MostUsed,
            "recently-used" => Self::RecentlyUsed,
            _ => Self::Name,
        }
    }

    pub fn value(self) -> &'static str {
        match self {
            Self::Smart => "smart",
            Self::MostUsed => "most-used",
            Self::RecentlyUsed => "recently-used",
            Self::Name => "name",
        }
    }

    /// Short label for the palette's ordering pill.
    pub fn label(self) -> &'static str {
        match self {
            Self::Smart => "智能排序",
            Self::MostUsed => "最常用",
            Self::RecentlyUsed => "最近使用",
            Self::Name => "按名称",
        }
    }
}

pub fn normalize(value: &str) -> String {
    value.trim().to_lowercase()
}

/// Word starts after a space or one of the separators desktop entries use.
fn is_word_start(text: &str, needle: &str) -> bool {
    if text.starts_with(needle) {
        return true;
    }
    for (index, character) in text.char_indices().skip(1) {
        if !matches!(character, ' ' | '-' | '_' | '.' | '/') {
            continue;
        }
        // Every separator is ASCII, so `index + 1` is a char boundary.
        if text[index + 1..].starts_with(needle) {
            return true;
        }
    }
    false
}

/// Extra characters spanned by the shortest subsequence match, or `None`.
fn subsequence_penalty(text: &str, needle: &str) -> Option<usize> {
    let mut needle_chars = needle.chars();
    let mut pending = needle_chars.next();
    if pending.is_none() {
        return Some(0);
    }
    let mut first = None;
    let mut last = 0;
    for (index, character) in text.chars().enumerate() {
        let Some(wanted) = pending else { break };
        if character != wanted {
            continue;
        }
        first.get_or_insert(index);
        last = index;
        pending = needle_chars.next();
    }
    if pending.is_some() {
        return None;
    }
    let first = first?;
    let length = needle.chars().count() as isize;
    let penalty = last as isize - first as isize - length + 1;
    Some(penalty.max(0) as usize)
}

/// Score one field. `-1` means the field does not match at all.
pub fn field_score(value: &str, needle: &str, weight: i32) -> i32 {
    let text = normalize(value);
    if text.is_empty() {
        return if needle.is_empty() { weight } else { -1 };
    }
    if needle.is_empty() {
        return weight;
    }
    if text == needle {
        return EXACT + weight;
    }
    if let Some(rest) = text.strip_prefix(needle) {
        return PREFIX + weight - (rest.chars().count().min(99) as i32);
    }
    if is_word_start(&text, needle) {
        return WORD_START + weight;
    }
    if let Some(index) = text.find(needle) {
        let distance = text[..index].chars().count().min(99) as i32;
        return SUBSTRING + weight - distance;
    }
    match subsequence_penalty(&text, needle) {
        Some(penalty) => SUBSEQUENCE + weight - (penalty.min(99) as i32),
        None => -1,
    }
}

/// Best field score for one application, or `-1` when nothing matches.
pub fn app_score(app: &DesktopApp, needle: &str) -> i32 {
    if needle.is_empty() {
        return 0;
    }
    let keywords = app.keywords.join(" ");
    [
        field_score(&app.name, needle, WEIGHT_NAME),
        field_score(&app.generic_name, needle, WEIGHT_GENERIC),
        field_score(&keywords, needle, WEIGHT_KEYWORDS),
        field_score(&app.id, needle, WEIGHT_ID),
    ]
    .into_iter()
    .max()
    .unwrap_or(-1)
}

/// Rank applications for one query.
///
/// Score first, then usage (only as a tie-breaker, never overriding textual
/// relevance), then localized name and desktop id for a stable order.
pub fn ranked_apps<'a>(
    apps: impl IntoIterator<Item = &'a DesktopApp>,
    query: &str,
    order: AppOrder,
    history: &HashMap<String, LaunchRecord>,
    now_ms: u64,
) -> Vec<&'a DesktopApp> {
    let needle = normalize(query);
    let mut hits: Vec<(i32, &'a DesktopApp)> = apps
        .into_iter()
        .filter_map(|app| {
            let score = app_score(app, &needle);
            (score >= 0).then_some((score, app))
        })
        .collect();
    hits.sort_by(|(left_score, left), (right_score, right)| {
        right_score.cmp(left_score).then_with(|| {
            let right_usage = usage_score(order, history.get(&right.id), now_ms);
            let left_usage = usage_score(order, history.get(&left.id), now_ms);
            right_usage
                .partial_cmp(&left_usage)
                .unwrap_or(Ordering::Equal)
                .then_with(|| {
                    left.name
                        .to_lowercase()
                        .cmp(&right.name.to_lowercase())
                        .then_with(|| left.id.cmp(&right.id))
                })
        })
    });
    hits.into_iter().map(|(_, app)| app).collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::catalog::AppCategory;

    fn app(id: &str, name: &str, generic: &str, keywords: &[&str]) -> DesktopApp {
        DesktopApp {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            generic_name: generic.into(),
            desktop_file: PathBuf::from(format!("/apps/{id}")),
            keywords: keywords.iter().map(|word| (*word).to_owned()).collect(),
            categories: Vec::new(),
            category: AppCategory::Other,
            icon_path: None,
            icon_name: None,
            mime_types: Vec::new(),
        }
    }

    fn history(entries: &[(&str, u64, u64)]) -> HashMap<String, LaunchRecord> {
        entries
            .iter()
            .map(|(id, count, at)| {
                (
                    (*id).to_owned(),
                    LaunchRecord {
                        launch_count: *count,
                        last_launched_at: *at,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn the_ladder_prefers_exact_then_prefix_then_word_start() {
        assert!(field_score("firefox", "firefox", 80) > field_score("firefox esr", "firefox", 80));
        assert!(field_score("firefox esr", "firefox", 80) > field_score("x firefox", "firefox", 80));
        assert!(field_score("x firefox", "firefox", 80) > field_score("myfirefox", "firefox", 80));
        assert!(field_score("myfirefox", "firefox", 80) > field_score("f-i-r-e-f-o-x", "firefox", 80));
        assert_eq!(field_score("terminal", "firefox", 80), -1);
    }

    #[test]
    fn separators_count_as_word_starts_but_plain_letters_do_not() {
        assert!(is_word_start("org.gnome.terminal", "terminal"));
        assert!(is_word_start("gnome-terminal", "terminal"));
        assert!(is_word_start("gnome_terminal", "terminal"));
        assert!(is_word_start("gnome terminal", "terminal"));
        assert!(!is_word_start("gnometerminal", "terminal"));
    }

    #[test]
    fn subsequence_penalty_measures_span_not_length() {
        assert_eq!(subsequence_penalty("firefox", "ffx"), Some(4));
        assert_eq!(subsequence_penalty("firefox", "firefox"), Some(0));
        assert_eq!(subsequence_penalty("firefox", "xyz"), None);
    }

    #[test]
    fn name_hits_outrank_keyword_hits() {
        let by_name = app("a.desktop", "Photos", "", &[]);
        let by_keyword = app("b.desktop", "Shotwell", "", &["photos"]);
        assert!(app_score(&by_name, "photos") > app_score(&by_keyword, "photos"));
    }

    #[test]
    fn empty_query_matches_every_application() {
        let apps = [app("a.desktop", "Alpha", "", &[]), app("b.desktop", "Beta", "", &[])];
        let ranked = ranked_apps(&apps, "", AppOrder::Name, &HashMap::new(), 0);
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].name, "Alpha");
    }

    #[test]
    fn usage_never_overrides_a_better_text_score() {
        let strong = app("strong.desktop", "Terminal", "", &[]);
        let weak = app("weak.desktop", "X Terminal Emulator", "", &[]);
        let usage = history(&[("weak.desktop", 500, 9_000_000)]);
        let ranked = ranked_apps([&strong, &weak], "terminal", AppOrder::MostUsed, &usage, 10_000_000);
        assert_eq!(ranked[0].id, "strong.desktop");
    }

    #[test]
    fn usage_breaks_ties_between_equal_scores() {
        let left = app("left.desktop", "Editor", "", &[]);
        let right = app("right.desktop", "Editor", "", &[]);
        let usage = history(&[("right.desktop", 3, 42)]);
        let ranked = ranked_apps([&left, &right], "editor", AppOrder::MostUsed, &usage, 100);
        assert_eq!(ranked[0].id, "right.desktop");
        // Name order ignores usage entirely.
        let ranked = ranked_apps([&right, &left], "editor", AppOrder::Name, &usage, 100);
        assert_eq!(ranked[0].id, "left.desktop");
    }

    #[test]
    fn recently_used_prefers_the_newest_timestamp() {
        let old = app("old.desktop", "Editor", "", &[]);
        let new = app("new.desktop", "Editor", "", &[]);
        let usage = history(&[("old.desktop", 99, 10), ("new.desktop", 1, 20)]);
        let ranked = ranked_apps([&old, &new], "editor", AppOrder::RecentlyUsed, &usage, 30);
        assert_eq!(ranked[0].id, "new.desktop");
        let ranked = ranked_apps([&old, &new], "editor", AppOrder::MostUsed, &usage, 30);
        assert_eq!(ranked[0].id, "old.desktop");
    }

    #[test]
    fn unmatched_applications_disappear() {
        let apps = [app("a.desktop", "Alpha", "", &[]), app("b.desktop", "Beta", "", &[])];
        let ranked = ranked_apps(&apps, "beta", AppOrder::Name, &HashMap::new(), 0);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].id, "b.desktop");
    }

    #[test]
    fn order_values_round_trip() {
        for order in [
            AppOrder::Smart,
            AppOrder::MostUsed,
            AppOrder::RecentlyUsed,
            AppOrder::Name,
        ] {
            assert_eq!(AppOrder::from_value(order.value()), order);
        }
        assert_eq!(AppOrder::from_value("unsupported"), AppOrder::Name);
    }
}
