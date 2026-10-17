//! Which words of a draft to spell check: prose only, never code, links,
//! mentions, emoji names or identifiers.

use std::ops::Range;

use super::markup::markup_styles;

/// Byte ranges of the words in `text` worth checking.
pub fn checkable_words(text: &str) -> Vec<Range<usize>> {
    let mut skip = vec![false; text.len()];
    for (range, style) in markup_styles(text) {
        if style.code || style.code_block {
            skip[range].fill(true);
        }
    }
    // Whole whitespace-separated chunks that are links, mentions or Slack tokens.
    let mut start = None;
    for (index, ch) in text.char_indices().chain([(text.len(), ' ')]) {
        match (ch.is_whitespace(), start) {
            (false, None) => start = Some(index),
            (true, Some(from)) => {
                let chunk = &text[from..index];
                if chunk.contains("://")
                    || chunk.starts_with("www.")
                    || chunk.contains(['@', '<', '>', '/', '\\', '#', '='])
                {
                    skip[from..index].fill(true);
                }
                start = None;
            }
            _ => {}
        }
    }
    for range in emoji_names(text) {
        skip[range].fill(true);
    }

    let mut words = Vec::new();
    let mut token_start = None;
    for (index, ch) in text.char_indices().chain([(text.len(), ' ')]) {
        let part = ch.is_alphanumeric() || matches!(ch, '_' | '\'' | '’');
        match (part, token_start) {
            (true, None) => token_start = Some(index),
            (false, Some(from)) => {
                token_start = None;
                if let Some(word) = word_in(text, from..index)
                    && !skip[word.clone()].iter().any(|skipped| *skipped)
                {
                    words.push(word);
                }
            }
            _ => {}
        }
    }
    words
}

/// The checkable word in a token, without the italic underscores or quotes
/// around it; none for identifiers, numbers, acronyms and mixed case.
fn word_in(text: &str, token: Range<usize>) -> Option<Range<usize>> {
    let raw = &text[token.clone()];
    let trimmed = raw.trim_start_matches(['_', '\'', '’']);
    let start = token.start + raw.len() - trimmed.len();
    let word = trimmed.trim_end_matches(['_', '\'', '’']);
    let end = start + word.len();
    let mut chars = word.chars();
    let first = chars.next()?;
    let rest: Vec<char> = chars.collect();
    let acceptable = !rest.is_empty()
        && !word.contains('_')
        && !word.chars().any(|ch| ch.is_numeric())
        // Acronyms (API) and camelCase are names and code, not prose.
        && !rest.iter().any(|ch| ch.is_uppercase())
        && first.is_alphabetic();
    acceptable.then_some(start..end)
}

/// `:emoji_name:` shortcodes.
fn emoji_names(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b':' {
            let name_end = bytes[index + 1..]
                .iter()
                .position(|byte| {
                    !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'+' | b'-'))
                })
                .map(|offset| index + 1 + offset);
            if let Some(end) = name_end
                && end > index + 1
                && bytes[end] == b':'
            {
                ranges.push(index..end + 1);
                index = end + 1;
                continue;
            }
        }
        index += 1;
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<&str> {
        checkable_words(text)
            .into_iter()
            .map(|range| &text[range])
            .collect()
    }

    #[test]
    fn checks_prose_but_not_code_links_mentions_or_names() {
        assert_eq!(
            words("*Helo* _wrld_ it's `fn mian` :tada: <@U1> @anna"),
            ["Helo", "wrld", "it's"]
        );
        assert_eq!(
            words("see https://exmple.com/pth and ```\nlet x = vecc\n``` ok"),
            ["see", "and", "ok"]
        );
        assert_eq!(
            words("API v2 snake_case camelCase 2024 Göteborg är"),
            ["Göteborg", "är"]
        );
        assert_eq!(words("well-knwn ‘quoted’ a"), ["well", "knwn", "quoted"]);
    }
}
