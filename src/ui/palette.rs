use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as MatcherConfig, Matcher};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    AddWorktree,
    RemoveWorktree,
    SwitchRepo,
    Refresh,
    Quit,
}

impl Command {
    pub const ALL: [Command; 5] = [
        Command::AddWorktree,
        Command::RemoveWorktree,
        Command::SwitchRepo,
        Command::Refresh,
        Command::Quit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Command::AddWorktree => "Add worktree",
            Command::RemoveWorktree => "Remove worktree",
            Command::SwitchRepo => "Switch repo",
            Command::Refresh => "Refresh",
            Command::Quit => "Quit",
        }
    }
}

/// Ranks the commands against a fuzzy `query`. An empty query returns every
/// command in declaration order.
pub fn filter(query: &str) -> Vec<Command> {
    if query.is_empty() {
        return Command::ALL.to_vec();
    }

    let mut matcher = Matcher::new(MatcherConfig::DEFAULT);
    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);

    let mut scored: Vec<(u32, Command)> = Command::ALL
        .iter()
        .filter_map(|&cmd| {
            let mut buf = Vec::new();
            let haystack = nucleo_matcher::Utf32Str::new(cmd.label(), &mut buf);
            pattern.score(haystack, &mut matcher).map(|s| (s, cmd))
        })
        .collect();

    scored.sort_by_key(|&(score, _)| std::cmp::Reverse(score));
    scored.into_iter().map(|(_, cmd)| cmd).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_returns_all_in_order() {
        assert_eq!(filter(""), Command::ALL.to_vec());
    }

    #[test]
    fn fuzzy_query_ranks_best_match_first() {
        let result = filter("add");
        assert_eq!(result.first(), Some(&Command::AddWorktree));
    }

    #[test]
    fn nonsense_query_filters_everything_out() {
        assert!(filter("zzzzqqqq").is_empty());
    }
}
