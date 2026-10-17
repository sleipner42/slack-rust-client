//! Which emoji the user reaches for: how often and how recently, scored the
//! way Slack's "frequently used" list is ("frecency").

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EmojiUse {
    pub count: u32,
    /// Recent uses, Unix milliseconds, oldest first.
    pub visits: Vec<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EmojiUsage {
    pub emoji: HashMap<String, EmojiUse>,
}

impl EmojiUsage {
    pub fn record(&mut self, name: &str, now_ms: i64) {
        let entry = self.emoji.entry(name.to_owned()).or_default();
        entry.count += 1;
        entry.visits.push(now_ms);
        if entry.visits.len() > MAX_VISITS {
            entry.visits.remove(0);
        }
    }

    /// Both histories together: counts add up, visits interleave.
    pub fn merged(&self, other: &EmojiUsage) -> EmojiUsage {
        let mut merged = self.clone();
        for (name, theirs) in &other.emoji {
            let entry = merged.emoji.entry(name.clone()).or_default();
            entry.count += theirs.count;
            entry.visits.extend(&theirs.visits);
            entry.visits.sort_unstable();
            let excess = entry.visits.len().saturating_sub(MAX_VISITS);
            entry.visits.drain(..excess);
        }
        merged
    }

    /// Emoji names, most frecent first.
    pub fn ranked(&self, now_ms: i64) -> Vec<String> {
        let mut scored: Vec<(f64, &String)> = self
            .emoji
            .iter()
            .map(|(name, entry)| (score(entry, now_ms), name))
            .filter(|(score, _)| *score > 0.0)
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        scored.into_iter().map(|(_, name)| name.clone()).collect()
    }
}

/// Recent visits weigh most; the total count scales the sample up.
fn score(entry: &EmojiUse, now_ms: i64) -> f64 {
    if entry.visits.is_empty() {
        return 0.0;
    }
    let weights: f64 = entry
        .visits
        .iter()
        .map(|visit| {
            let hours = (now_ms - visit).max(0) as f64 / 3_600_000.0;
            match hours {
                h if h < 4.0 => 100.0,
                h if h < 24.0 => 80.0,
                h if h < 72.0 => 60.0,
                h if h < 168.0 => 40.0,
                h if h < 720.0 => 20.0,
                h if h < 2160.0 => 10.0,
                _ => 1.0,
            }
        })
        .sum();
    weights * f64::from(entry.count.max(1)) / entry.visits.len() as f64
}

const MAX_VISITS: usize = 10;

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 3_600_000;

    #[test]
    fn recent_and_frequent_emoji_rank_first() {
        let now = 1_000 * 24 * HOUR;
        let mut usage = EmojiUsage::default();
        usage.emoji.insert(
            "white_check_mark".into(),
            EmojiUse {
                count: 211,
                visits: vec![now - 200 * 24 * HOUR],
            },
        );
        usage.emoji.insert(
            "eyes".into(),
            EmojiUse {
                count: 17,
                visits: vec![now - 2 * HOUR, now - HOUR],
            },
        );
        usage.emoji.insert(
            "tada".into(),
            EmojiUse {
                count: 3,
                visits: vec![now - 60 * 24 * HOUR],
            },
        );
        assert_eq!(usage.ranked(now), ["eyes", "white_check_mark", "tada"]);
        let mut local = EmojiUsage::default();
        for _ in 0..30 {
            local.record("tada", now);
        }
        assert_eq!(usage.merged(&local).ranked(now)[0], "tada");
        assert_eq!(local.emoji["tada"].visits.len(), MAX_VISITS);
    }
}
