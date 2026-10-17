use std::collections::HashMap;
use std::sync::OnceLock;

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
        match self.custom.get(name) {
            Some(target) => match target.strip_prefix("alias:") {
                // A workspace name for a standard emoji.
                Some(standard) => match Self::standard(standard) {
                    EmojiGlyph::Missing(_) => EmojiGlyph::Missing(name.to_string()),
                    glyph => glyph,
                },
                None => EmojiGlyph::Image(target.clone()),
            },
            None => Self::standard(name),
        }
    }

    fn standard(name: &str) -> EmojiGlyph {
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
                Some(format!("{TWEMOJI_BASE}{codepoints}.png"))
            }
            EmojiGlyph::Missing(_) => None,
        }
    }
}

/// Standard emoji ship inside the binary; the CDN URL stays the cache key and the
/// fallback for anything the bundled Twemoji release lacks.
pub fn bundled_image(url: &str) -> Option<&'static [u8]> {
    let codepoints = url.strip_prefix(TWEMOJI_BASE)?.strip_suffix(".png")?;
    let (index, data) = PACK.get_or_init(|| parse_pack(TWEMOJI_PACK).unwrap_or_default());
    let &(offset, length) = index.get(codepoints)?;
    data.get(offset..offset + length)
}

type Pack = (HashMap<&'static str, (usize, usize)>, &'static [u8]);

static PACK: OnceLock<Pack> = OnceLock::new();

/// Reads the index written by `scripts/build-emoji-pack.py`.
fn parse_pack(pack: &'static [u8]) -> Option<Pack> {
    let mut rest = pack.strip_prefix(b"TWEMOJI1")?;
    let mut take = |length: usize| {
        let (head, tail) = rest.split_at_checked(length)?;
        rest = tail;
        Some(head)
    };
    let u32_at = |bytes: &[u8]| bytes.try_into().ok().map(u32::from_le_bytes);
    let count = u32_at(take(4)?)?;
    let mut index = HashMap::with_capacity(count as usize);
    for _ in 0..count {
        let length = take(1)?[0] as usize;
        let name = std::str::from_utf8(take(length)?).ok()?;
        let offset = u32_at(take(4)?)? as usize;
        let size = u32_at(take(4)?)? as usize;
        index.insert(name, (offset, size));
    }
    Some((index, rest))
}

static TWEMOJI_PACK: &[u8] = include_bytes!("../../assets/emoji/twemoji-72.pack");
const TWEMOJI_BASE: &str = "https://cdn.jsdelivr.net/gh/jdecked/twemoji@17.0.3/assets/72x72/";

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
    fn bundled_pack_serves_standard_emoji_without_downloads() {
        let catalog = EmojiCatalog::default();
        for name in [
            "+1",
            "heart",
            "tada",
            "eyes",
            "white_check_mark",
            "joy",
            "pray",
        ] {
            let url = catalog.image_url(name).unwrap();
            let png = bundled_image(&url).unwrap_or_else(|| panic!("{name} missing"));
            assert!(png.starts_with(b"\x89PNG"), "{name}");
        }
        assert!(bundled_image(&catalog.image_url("+1::skin-tone-3").unwrap()).is_some());
        assert!(bundled_image("https://emoji.slack-edge.com/T1/party/1.gif").is_none());
        let shortcodes: Vec<_> = emojis::iter().filter_map(|e| e.shortcode()).collect();
        let missing: Vec<_> = shortcodes
            .iter()
            .filter(|name| bundled_image(&catalog.image_url(name).unwrap()).is_none())
            .collect();
        // Anything missing still loads from the CDN; keep the gap small.
        assert!(
            missing.len() * 100 <= shortcodes.len(),
            "{} of {} missing: {missing:?}",
            missing.len(),
            shortcodes.len()
        );
    }

    #[test]
    fn rejects_malformed_packs() {
        assert!(parse_pack(b"").is_none());
        assert!(parse_pack(b"TWEMOJI1\x01\x00\x00\x00\x05ab").is_none());
        let (index, data) =
            parse_pack(b"TWEMOJI1\x01\x00\x00\x00\x02ab\x00\x00\x00\x00\x01\x00\x00\x00Z").unwrap();
        assert_eq!(index["ab"], (0, 1));
        assert_eq!(data, b"Z");
    }

    #[test]
    fn resolves_standard_custom_and_missing() {
        let catalog = EmojiCatalog::new(HashMap::from([
            ("party".to_string(), "https://e/p.gif".to_string()),
            ("yay".to_string(), "alias:tada".to_string()),
            ("broken".to_string(), "alias:no_such_emoji".to_string()),
        ]));
        assert_eq!(catalog.resolve("yay"), EmojiGlyph::Unicode("🎉".into()));
        assert_eq!(
            catalog.resolve("broken"),
            EmojiGlyph::Missing("broken".into())
        );
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
