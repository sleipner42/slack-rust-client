use crate::domain::model::Conversation;

pub fn rank_conversations(
    query: &str,
    conversations: &[Conversation],
    limit: usize,
) -> Vec<Conversation> {
    let needle = query.trim().to_lowercase();
    let mut scored: Vec<(MatchRank, &Conversation)> = conversations
        .iter()
        .filter_map(|conversation| {
            match_rank(&needle, &conversation.name.to_lowercase()).map(|rank| (rank, conversation))
        })
        .collect();
    scored.sort_by(|(a_rank, a), (b_rank, b)| {
        a_rank
            .cmp(b_rank)
            .then_with(|| a.name.len().cmp(&b.name.len()))
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(_, conversation)| conversation.clone())
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum MatchRank {
    Exact,
    Prefix,
    WordPrefix,
    Contains,
    Subsequence(usize),
}

fn match_rank(needle: &str, haystack: &str) -> Option<MatchRank> {
    if needle.is_empty() {
        return Some(MatchRank::Contains);
    }
    if haystack == needle {
        return Some(MatchRank::Exact);
    }
    if haystack.starts_with(needle) {
        return Some(MatchRank::Prefix);
    }
    if haystack
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word.starts_with(needle))
    {
        return Some(MatchRank::WordPrefix);
    }
    if haystack.contains(needle) {
        return Some(MatchRank::Contains);
    }
    subsequence_gaps(needle, haystack).map(MatchRank::Subsequence)
}

fn subsequence_gaps(needle: &str, haystack: &str) -> Option<usize> {
    let mut gaps = 0;
    let mut remaining = haystack.chars();
    for wanted in needle.chars() {
        let mut skipped = 0;
        loop {
            match remaining.next() {
                Some(c) if c == wanted => break,
                Some(_) => skipped += 1,
                None => return None,
            }
        }
        gaps += skipped;
    }
    Some(gaps)
}
