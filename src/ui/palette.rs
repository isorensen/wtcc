use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as MatcherConfig, Matcher};

use crate::keymap::Action;

/// The palette command list, derived from the keymap: every action the keymap
/// declares as palette-visible, in [`Action::ALL`] order.
fn palette_actions() -> Vec<Action> {
    Action::ALL
        .iter()
        .copied()
        .filter(|a| a.in_palette())
        .collect()
}

/// Ranks the palette actions against a fuzzy `query`. An empty query returns
/// every palette action in declaration order.
pub fn filter(query: &str) -> Vec<Action> {
    let actions = palette_actions();
    if query.is_empty() {
        return actions;
    }

    let mut matcher = Matcher::new(MatcherConfig::DEFAULT);
    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);

    let mut scored: Vec<(u32, Action)> = actions
        .iter()
        .filter_map(|&action| {
            let mut buf = Vec::new();
            let haystack = nucleo_matcher::Utf32Str::new(action.label(), &mut buf);
            pattern.score(haystack, &mut matcher).map(|s| (s, action))
        })
        .collect();

    scored.sort_by_key(|&(score, _)| std::cmp::Reverse(score));
    scored.into_iter().map(|(_, action)| action).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_returns_all_palette_actions_in_order() {
        assert_eq!(filter(""), palette_actions());
    }

    #[test]
    fn fuzzy_query_ranks_best_match_first() {
        assert_eq!(filter("worktree").first(), Some(&Action::AddWorktree));
    }

    #[test]
    fn fuzzy_query_matches_add_repo() {
        assert_eq!(filter("repo").first(), Some(&Action::AddRepo));
    }

    #[test]
    fn nonsense_query_filters_everything_out() {
        assert!(filter("zzzzqqqq").is_empty());
    }

    #[test]
    fn palette_excludes_nav_and_modal_actions() {
        let actions = palette_actions();
        assert!(!actions.contains(&Action::Next));
        assert!(!actions.contains(&Action::OpenPalette));
        assert!(!actions.contains(&Action::Help));
    }
}
