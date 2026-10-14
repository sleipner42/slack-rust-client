use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub enum EmojiGlyph {
    Unicode(String),
    Image(String),
    Missing(String),
}

#[derive(Clone, Debug, Default)]
pub struct EmojiCatalog {
    custom: HashMap<String, String>,
}

impl EmojiCatalog {
    pub fn new(custom: HashMap<String, String>) -> Self {
        Self { custom }
    }

    pub fn resolve(&self, name: &str) -> EmojiGlyph {
        if let Some(url) = self.custom.get(name) {
            return EmojiGlyph::Image(url.clone());
        }
        let (base, tone) = name
            .split_once("::skin-tone-")
            .map_or((name, None), |(base, tone)| {
                (base, tone.parse::<usize>().ok())
            });
        let lookup = SLACK_ALIASES
            .iter()
            .find(|(alias, _)| *alias == base)
            .map(|(_, standard)| *standard)
            .unwrap_or(base);
        match emojis::get_by_shortcode(lookup) {
            Some(emoji) => {
                let toned = tone
                    .and_then(|tone| tone.checked_sub(1))
                    .and_then(|index| emoji.skin_tones().and_then(|mut tones| tones.nth(index)))
                    .unwrap_or(emoji);
                EmojiGlyph::Unicode(toned.as_str().to_string())
            }
            None => EmojiGlyph::Missing(name.to_string()),
        }
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<String> {
        let needle = query.trim().to_lowercase();
        let standard = emojis::iter().filter_map(|emoji| emoji.shortcode().map(str::to_string));
        let custom = self.custom.keys().cloned();
        let mut names: Vec<String> = standard
            .chain(custom)
            .filter(|name| needle.is_empty() || name.to_lowercase().contains(&needle))
            .collect();
        if !needle.is_empty() {
            names.sort_by_key(|name| {
                (
                    !name.to_lowercase().starts_with(&needle),
                    name.len(),
                    name.clone(),
                )
            });
        }
        let mut seen = std::collections::HashSet::new();
        names.retain(|name| seen.insert(name.clone()));
        names.truncate(limit);
        names
    }

    pub fn image_url(&self, name: &str) -> Option<String> {
        match self.resolve(name) {
            EmojiGlyph::Image(url) => Some(url),
            EmojiGlyph::Unicode(glyph) => {
                let joined = glyph.contains('\u{200d}');
                let codepoints = glyph
                    .chars()
                    .filter(|c| joined || *c != '\u{fe0f}')
                    .map(|c| format!("{:x}", c as u32))
                    .collect::<Vec<_>>()
                    .join("-");
                Some(format!(
                    "https://cdn.jsdelivr.net/gh/jdecked/twemoji@17.0.3/assets/72x72/{codepoints}.png"
                ))
            }
            EmojiGlyph::Missing(_) => None,
        }
    }
}

const SLACK_ALIASES: [(&str, &str); 9] = [
    ("thumbsup", "+1"),
    ("thumbsdown", "-1"),
    ("simple_smile", "slightly_smiling_face"),
    ("white_check_mark", "white_check_mark"),
    ("heavy_check_mark", "heavy_check_mark"),
    ("slightly_smiling_face", "slightly_smiling_face"),
    ("face_with_rolling_eyes", "roll_eyes"),
    ("face_with_raised_eyebrow", "raised_eyebrow"),
    ("hugging_face", "hugs"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_standard_custom_and_missing() {
        let catalog = EmojiCatalog::new(HashMap::from([(
            "party".to_string(),
            "https://e/p.gif".to_string(),
        )]));
        assert_eq!(
            catalog.resolve("thumbsup"),
            EmojiGlyph::Unicode("👍".into())
        );
        assert_eq!(catalog.resolve("tada"), EmojiGlyph::Unicode("🎉".into()));
        assert_eq!(
            catalog.resolve("thumbsup::skin-tone-2"),
            EmojiGlyph::Unicode("👍🏻".into())
        );
        assert_eq!(
            catalog.resolve("party"),
            EmojiGlyph::Image("https://e/p.gif".into())
        );
        assert_eq!(
            catalog.resolve("nope_nope"),
            EmojiGlyph::Missing("nope_nope".into())
        );
    }
}
