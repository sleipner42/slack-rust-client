//! Searching messages the way Slack's search bar does: Slack runs the query
//! (with `from:`, `in:`, `before:` and the rest) and marks the matched words.

use serde::{Deserialize, Serialize};

use super::model::Message;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SearchSort {
    #[default]
    Newest,
    Relevant,
}

impl SearchSort {
    pub const ALL: [SearchSort; 2] = [SearchSort::Newest, SearchSort::Relevant];

    pub fn label(self) -> &'static str {
        match self {
            SearchSort::Newest => "Newest",
            SearchSort::Relevant => "Most relevant",
        }
    }
}

/// A message search found.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchMatch {
    pub conversation_id: String,
    /// Slack's name for the conversation; a user ID for direct messages.
    pub conversation_name: String,
    pub is_channel: bool,
    /// The thread the message is in, if any.
    pub thread_ts: Option<String>,
    pub message: Message,
    /// Slack markup with the matched words between `HIGHLIGHT_START` and
    /// `HIGHLIGHT_END`.
    pub highlighted: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchPage {
    pub matches: Vec<SearchMatch>,
    /// One-based.
    pub page: u32,
    pub pages: u32,
    pub total: u32,
}

pub const HIGHLIGHT_START: char = '\u{e000}';
pub const HIGHLIGHT_END: char = '\u{e001}';

pub fn strip_highlights(text: &str) -> String {
    text.chars()
        .filter(|ch| *ch != HIGHLIGHT_START && *ch != HIGHLIGHT_END)
        .collect()
}

/// The text in runs, each marked whether it matched.
pub fn highlight_runs(text: &str) -> Vec<(String, bool)> {
    let mut runs: Vec<(String, bool)> = Vec::new();
    let mut highlighted = false;
    for ch in text.chars() {
        match ch {
            HIGHLIGHT_START => highlighted = true,
            HIGHLIGHT_END => highlighted = false,
            _ => match runs.last_mut() {
                Some((run, flag)) if *flag == highlighted => run.push(ch),
                _ => runs.push((ch.to_string(), highlighted)),
            },
        }
    }
    runs
}

/// The text on one line, starting a little before the first match so the
/// match stays in view when the line is cut short.
pub fn snippet(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = line.chars().collect();
    let first = chars
        .iter()
        .position(|ch| *ch == HIGHLIGHT_START)
        .unwrap_or(0);
    if first <= SNIPPET_LEAD {
        return line;
    }
    let mut start = first - SNIPPET_LEAD;
    while start < first && !chars[start - 1].is_whitespace() {
        start += 1;
    }
    format!("…{}", chars[start..].iter().collect::<String>())
}

/// The thread a message permalink points into.
pub fn permalink_thread(permalink: &str) -> Option<String> {
    let (_, query) = permalink.split_once('?')?;
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix("thread_ts="))
        .filter(|ts| !ts.is_empty())
        .map(str::to_owned)
}

/// Characters of context kept before the first match.
const SNIPPET_LEAD: usize = 40;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_split_into_runs_and_strip_cleanly() {
        let text = "ship the \u{e000}deploy\u{e001} at \u{e000}noon\u{e001}";
        assert_eq!(
            highlight_runs(text),
            vec![
                ("ship the ".into(), false),
                ("deploy".into(), true),
                (" at ".into(), false),
                ("noon".into(), true),
            ]
        );
        assert_eq!(strip_highlights(text), "ship the deploy at noon");
    }

    #[test]
    fn snippets_keep_the_first_match_in_view() {
        assert_eq!(
            snippet("short\n\n\u{e000}hit\u{e001} here"),
            "short \u{e000}hit\u{e001} here"
        );
        let long = format!("{} \u{e000}hit\u{e001} end", "word ".repeat(30));
        let cut = snippet(&long);
        assert!(cut.starts_with("…word"), "{cut}");
        assert!(cut.chars().count() < 60, "{cut}");
        assert!(cut.ends_with("\u{e000}hit\u{e001} end"));
    }

    #[test]
    fn thread_comes_from_the_permalink() {
        assert_eq!(
            permalink_thread(
                "https://x.slack.com/archives/C1/p1700000000000200?thread_ts=1700000000.000100&cid=C1"
            ),
            Some("1700000000.000100".into())
        );
        assert_eq!(
            permalink_thread("https://x.slack.com/archives/C1/p1700000000000200"),
            None
        );
    }
}
