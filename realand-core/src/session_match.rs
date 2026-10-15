//! Session restoration matches stable identities before transport or placement.
//! Exact app/title pairs are consumed in live insertion order, then only one
//! unmatched saved and one unmatched live identity may share an app fallback.
//! Strings are borrowed; this module performs no file or protocol operations.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;

/// Results retain saved positions, including anonymous and unmatched entries.
/// The platform validates session size (4096 saved windows in Realand) before
/// this call. Work/storage are linear in input count, apart from string hashing.
/// Repeated live IDs use their first identity record and cannot match twice.
pub fn match_session_windows<I: Clone + Eq + Hash>(
    available: &[(I, (String, String))],
    saved: &[(&str, &str)],
) -> Vec<Option<I>> {
    let mut exact: HashMap<(&str, &str), VecDeque<usize>> = HashMap::new();
    let mut unique = HashSet::with_capacity(available.len());
    let mut eligible = vec![false; available.len()];
    for (index, (id, (app, title))) in available.iter().enumerate() {
        if !unique.insert(id) || app.is_empty() {
            continue;
        }
        eligible[index] = true;
        exact
            .entry((app.as_str(), title.as_str()))
            .or_default()
            .push_back(index);
    }
    let mut matched = vec![None; saved.len()];
    let mut used = vec![false; available.len()];
    for (index, &(app, title)) in saved.iter().enumerate() {
        if app.is_empty() {
            continue;
        }
        if let Some(live) = exact.get_mut(&(app, title)).and_then(VecDeque::pop_front) {
            matched[index] = Some(available[live].0.clone());
            used[live] = true;
        }
    }
    let mut missing: HashMap<&str, (usize, usize)> = HashMap::new();
    for (index, &(app, _)) in saved.iter().enumerate() {
        if !app.is_empty() && matched[index].is_none() {
            let entry = missing.entry(app).or_insert((0, index));
            entry.0 += 1;
        }
    }
    let mut remaining: HashMap<&str, (usize, usize)> = HashMap::new();
    for (index, (_, (app, _))) in available.iter().enumerate() {
        if eligible[index] && !used[index] {
            let entry = remaining.entry(app.as_str()).or_insert((0, index));
            entry.0 += 1;
        }
    }
    for (app, (count, saved_index)) in missing {
        if let (1, Some(&(1, live_index))) = (count, remaining.get(app)) {
            matched[saved_index] = Some(available[live_index].0.clone());
        }
    }
    matched
}

#[cfg(test)]
mod tests {
    use super::*;
    fn live(values: &[(u64, &str, &str)]) -> Vec<(u64, (String, String))> {
        values
            .iter()
            .map(|(id, app, title)| (*id, ((*app).into(), (*title).into())))
            .collect()
    }
    #[test]
    fn repeated_exact_identities_consume_live_insertion_order_without_shifting_saved_slots() {
        let available = live(&[
            (17, "editor", "same"),
            (3, "editor", "same"),
            (9, "browser", "page"),
        ]);
        assert_eq!(
            match_session_windows(
                &available,
                &[
                    ("editor", "same"),
                    ("missing", "x"),
                    ("editor", "same"),
                    ("browser", "page")
                ]
            ),
            [Some(17), None, Some(3), Some(9)]
        );
    }
    #[test]
    fn exact_pass_finishes_before_unique_app_fallback_and_never_matches_anonymous_clients() {
        let available = live(&[
            (1, "editor", "new"),
            (2, "editor", "exact"),
            (3, "", "anonymous"),
        ]);
        assert_eq!(
            match_session_windows(
                &available,
                &[("editor", "old"), ("editor", "exact"), ("", "anonymous")]
            ),
            [Some(1), Some(2), None]
        );
    }
    #[test]
    fn ambiguous_saved_or_live_app_fallback_is_rejected() {
        let available = live(&[
            (1, "editor", "new"),
            (2, "browser", "a"),
            (3, "browser", "b"),
        ]);
        assert_eq!(
            match_session_windows(
                &available,
                &[("editor", "old-a"), ("editor", "old-b"), ("browser", "old")]
            ),
            [None, None, None]
        );
    }
    #[test]
    fn title_matching_is_exact_and_duplicate_live_ids_cannot_supply_two_windows() {
        let available = live(&[
            (1, "editor", "Title"),
            (1, "other", "same"),
            (2, "editor", "title"),
        ]);
        assert_eq!(
            match_session_windows(
                &available,
                &[("editor", "title"), ("editor", "Title"), ("other", "same")]
            ),
            [Some(2), Some(1), None]
        );
    }
    #[test]
    fn empty_live_and_saved_sets_preserve_result_shape() {
        assert_eq!(
            match_session_windows::<u64>(&[], &[("editor", "x"), ("", "y")]),
            [None, None]
        );
        assert!(match_session_windows(&live(&[(1, "editor", "x")]), &[]).is_empty());
    }
    #[test]
    fn a_full_4096_window_exact_restore_does_not_scan_previously_used_ids_quadratically() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COMPARISONS: AtomicUsize = AtomicUsize::new(0);
        #[derive(Clone)]
        struct Id(usize);
        impl Hash for Id {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
                self.0.hash(state);
            }
        }
        impl PartialEq for Id {
            fn eq(&self, other: &Self) -> bool {
                COMPARISONS.fetch_add(1, Ordering::Relaxed);
                self.0 == other.0
            }
        }
        impl Eq for Id {}
        let count = 4096;
        let available: Vec<_> = (0..count)
            .map(|id| (Id(id), ("editor".into(), "same".into())))
            .collect();
        let saved = vec![("editor", "same"); count];
        COMPARISONS.store(0, Ordering::Relaxed);
        let result = match_session_windows(&available, &saved);
        assert!(result
            .iter()
            .enumerate()
            .all(|(index, id)| id.as_ref().is_some_and(|id| id.0 == index)));
        // The old used-ID Vec scan made over eight million equality calls.
        // This broad bound tolerates hash-table collisions without depending
        // on a particular map implementation or timing-sensitive benchmark.
        assert!(COMPARISONS.load(Ordering::Relaxed) < count * 32);
    }
}
