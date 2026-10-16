#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Block {
    Paragraph(Vec<Inline>),
    Quote(Vec<Inline>),
    Code(String),
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Inline {
    Text {
        text: String,
        style: TextStyle,
    },
    Mention {
        label: String,
        is_self: bool,
    },
    Link {
        label: String,
        url: String,
        #[serde(default)]
        style: TextStyle,
    },
    Emoji(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TextStyle {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
}

pub trait NameLookup {
    fn user_name(&self, user_id: &str) -> String;
    fn conversation_name(&self, conversation_id: &str) -> String;
    fn is_self(&self, user_id: &str) -> bool;
}

pub fn parse_message_text(raw: &str, names: &dyn NameLookup) -> Vec<Block> {
    let mut blocks = Vec::new();
    for (index, chunk) in raw.split(CODE_FENCE).enumerate() {
        if index % 2 == 1 {
            let code = unescape(chunk.trim_matches('\n'));
            blocks.push(Block::Code(code));
        } else {
            blocks.extend(parse_prose(chunk, names));
        }
    }
    blocks
}

pub fn plain_text(blocks: &[Block]) -> String {
    blocks
        .iter()
        .map(|block| match block {
            Block::Paragraph(inlines) | Block::Quote(inlines) => {
                inlines.iter().map(inline_text).collect::<String>()
            }
            Block::Code(code) => code.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Readable clipboard text, retaining destinations hidden behind link labels.
pub fn clipboard_text(blocks: &[Block]) -> String {
    blocks
        .iter()
        .map(|block| match block {
            Block::Paragraph(inlines) | Block::Quote(inlines) => inlines
                .iter()
                .map(|inline| match inline {
                    Inline::Link { label, url, .. } if label != url => format!("{label} ({url})"),
                    Inline::Emoji(name) => emojis::get_by_shortcode(name)
                        .map(|emoji| emoji.as_str().to_owned())
                        .unwrap_or_else(|| format!(":{name}:")),
                    _ => inline_text(inline),
                })
                .collect::<String>(),
            Block::Code(code) => code.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn mentions_user(raw: &str, user_id: &str) -> bool {
    raw.contains(&format!("<@{user_id}>"))
        || raw.contains(&format!("<@{user_id}|"))
        || BROADCAST_MENTIONS
            .iter()
            .any(|mention| raw.contains(mention))
}

fn inline_text(inline: &Inline) -> String {
    match inline {
        Inline::Text { text, .. } => text.clone(),
        Inline::Mention { label, .. } => label.clone(),
        Inline::Link { label, .. } => label.clone(),
        Inline::Emoji(name) => format!(":{name}:"),
    }
}

fn parse_prose(chunk: &str, names: &dyn NameLookup) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut quote_lines: Vec<&str> = Vec::new();
    let mut paragraph_lines: Vec<&str> = Vec::new();
    for line in chunk.split('\n') {
        match quote_body(line) {
            Some(body) => {
                flush(&mut blocks, &mut paragraph_lines, false, names);
                quote_lines.push(body);
            }
            None => {
                flush(&mut blocks, &mut quote_lines, true, names);
                paragraph_lines.push(line);
            }
        }
    }
    flush(&mut blocks, &mut paragraph_lines, false, names);
    flush(&mut blocks, &mut quote_lines, true, names);
    blocks
}

fn quote_body(line: &str) -> Option<&str> {
    line.strip_prefix(ESCAPED_QUOTE)
        .or_else(|| line.strip_prefix('>'))
        .map(|body| body.strip_prefix(' ').unwrap_or(body))
}

fn flush(blocks: &mut Vec<Block>, lines: &mut Vec<&str>, is_quote: bool, names: &dyn NameLookup) {
    if lines.is_empty() {
        return;
    }
    let text = lines.join("\n");
    lines.clear();
    let trimmed = text.trim_matches('\n');
    if trimmed.trim().is_empty() {
        return;
    }
    let inlines = parse_inlines(trimmed, names);
    blocks.push(if is_quote {
        Block::Quote(inlines)
    } else {
        Block::Paragraph(inlines)
    });
}

/// Links, mentions and code are cut out first and stand in as placeholder
/// characters, so bold and italic markers can wrap them: `*<url|:fire: title>*`.
fn parse_inlines(text: &str, names: &dyn NameLookup) -> Vec<Inline> {
    let mut tokens = Vec::new();
    let mut marked = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        let next_token = rest.find('<');
        let next_code = rest.find('`');
        let (position, is_token) = match (next_token, next_code) {
            (Some(token), Some(code)) if token < code => (token, true),
            (Some(_), Some(code)) => (code, false),
            (Some(token), None) => (token, true),
            (None, Some(code)) => (code, false),
            (None, None) => break,
        };
        let closing = if is_token { '>' } else { '`' };
        let Some(length) = rest[position + 1..].find(closing) else {
            break;
        };
        let inner = &rest[position + 1..position + 1 + length];
        if !is_token && inner.is_empty() {
            marked.push_str(&rest[..position + 2]);
            rest = &rest[position + 2..];
            continue;
        }
        let Some(placeholder) = char::from_u32(PLACEHOLDER_BASE + tokens.len() as u32) else {
            break;
        };
        marked.push_str(&rest[..position]);
        marked.push(placeholder);
        tokens.push(if is_token {
            parse_token(inner, names)
        } else {
            Inline::Text {
                text: unescape(inner),
                style: TextStyle {
                    code: true,
                    ..TextStyle::default()
                },
            }
        });
        rest = &rest[position + length + 2..];
    }
    marked.push_str(rest);
    let mut styled_inlines = Vec::new();
    styled(&mut styled_inlines, &marked, TextStyle::default());
    let mut inlines = Vec::new();
    for inline in styled_inlines {
        let Inline::Text { text, style } = inline else {
            inlines.push(inline);
            continue;
        };
        let mut plain = String::new();
        for c in text.chars() {
            let token = (c as u32)
                .checked_sub(PLACEHOLDER_BASE)
                .and_then(|index| tokens.get(index as usize));
            match token {
                Some(token) => {
                    push_text(&mut inlines, &std::mem::take(&mut plain), style);
                    push_token(&mut inlines, token.clone(), style);
                }
                None => plain.push(c),
            }
        }
        // Already unescaped; escape again so push_text does not unescape twice.
        push_text(&mut inlines, &escape(&plain), style);
    }
    inlines
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Place a cut-out token inside the surrounding style; link labels get their emoji.
fn push_token(inlines: &mut Vec<Inline>, token: Inline, outer: TextStyle) {
    match token {
        Inline::Text { text, style } => inlines.push(Inline::Text {
            text,
            style: TextStyle {
                bold: style.bold || outer.bold,
                italic: style.italic || outer.italic,
                strike: style.strike || outer.strike,
                code: style.code || outer.code,
            },
        }),
        Inline::Link { label, url, .. } => {
            let mut pieces = Vec::new();
            emoji_split(&mut pieces, &escape(&label), TextStyle::default());
            for piece in pieces {
                inlines.push(match piece {
                    Inline::Text { text, .. } => Inline::Link {
                        label: text,
                        url: url.clone(),
                        style: outer,
                    },
                    other => other,
                });
            }
        }
        other => inlines.push(other),
    }
}

fn styled(inlines: &mut Vec<Inline>, text: &str, style: TextStyle) {
    let chars: Vec<char> = text.chars().collect();
    let mut plain_start = 0;
    let mut index = 0;
    while index < chars.len() {
        let marker = chars[index];
        if STYLE_MARKERS.contains(&marker)
            && opens_at(&chars, index)
            && let Some(close) = find_close(&chars, index)
        {
            emoji_split(
                inlines,
                &chars[plain_start..index].iter().collect::<String>(),
                style,
            );
            let inner: String = chars[index + 1..close].iter().collect();
            styled(inlines, &inner, apply_marker(style, marker));
            index = close + 1;
            plain_start = index;
            continue;
        }
        index += 1;
    }
    emoji_split(
        inlines,
        &chars[plain_start..].iter().collect::<String>(),
        style,
    );
}

pub(crate) fn opens_at(chars: &[char], index: usize) -> bool {
    let before_ok = index == 0 || !chars[index - 1].is_alphanumeric();
    let after_ok = chars
        .get(index + 1)
        .is_some_and(|c| !c.is_whitespace() && *c != chars[index]);
    before_ok && after_ok
}

pub(crate) fn find_close(chars: &[char], open: usize) -> Option<usize> {
    let marker = chars[open];
    (open + 2..chars.len())
        .take_while(|&i| chars[i] != '\n')
        .find(|&close| {
            chars[close] == marker
                && !chars[close - 1].is_whitespace()
                && chars.get(close + 1).is_none_or(|c| !c.is_alphanumeric())
        })
}

fn apply_marker(style: TextStyle, marker: char) -> TextStyle {
    match marker {
        '*' => TextStyle {
            bold: true,
            ..style
        },
        '_' => TextStyle {
            italic: true,
            ..style
        },
        _ => TextStyle {
            strike: true,
            ..style
        },
    }
}

fn emoji_split(inlines: &mut Vec<Inline>, text: &str, style: TextStyle) {
    let mut rest = text;
    while let Some(start) = rest.find(':') {
        let after = &rest[start + 1..];
        let Some(length) = after.find(':') else {
            break;
        };
        let name = &after[..length];
        let is_time = rest[..start].ends_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_digit())
            && after[length + 1..].starts_with(|c: char| c.is_ascii_digit());
        if is_emoji_name(name) && !is_time {
            push_text(inlines, &rest[..start], style);
            if name.starts_with(SKIN_TONE_PREFIX) {
                if let Some(Inline::Emoji(previous)) = inlines.last_mut() {
                    previous.push_str("::");
                    previous.push_str(name);
                } else {
                    push_text(inlines, &rest[start..start + length + 2], style);
                }
            } else {
                inlines.push(Inline::Emoji(name.to_string()));
            }
            rest = &after[length + 1..];
        } else {
            push_text(inlines, &rest[..start + 1], style);
            rest = after;
        }
    }
    push_text(inlines, rest, style);
}

fn is_emoji_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_EMOJI_NAME_LENGTH
        && name.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '+' | '\'')
        })
}

fn push_text(inlines: &mut Vec<Inline>, text: &str, style: TextStyle) {
    if text.is_empty() {
        return;
    }
    let text = unescape(text);
    if let Some(Inline::Text {
        text: previous,
        style: previous_style,
    }) = inlines.last_mut()
        && *previous_style == style
    {
        previous.push_str(&text);
        return;
    }
    inlines.push(Inline::Text { text, style });
}

fn parse_token(token: &str, names: &dyn NameLookup) -> Inline {
    let (target, label) = match token.split_once('|') {
        Some((target, label)) => (target, Some(label)),
        None => (token, None),
    };
    if let Some(user_id) = target.strip_prefix('@') {
        return Inline::Mention {
            label: format!(
                "@{}",
                label
                    .map(str::to_string)
                    .unwrap_or_else(|| names.user_name(user_id))
            ),
            is_self: names.is_self(user_id),
        };
    }
    if let Some(conversation_id) = target.strip_prefix('#') {
        return Inline::Mention {
            label: format!(
                "#{}",
                label
                    .map(str::to_string)
                    .unwrap_or_else(|| names.conversation_name(conversation_id))
            ),
            is_self: false,
        };
    }
    if let Some(special) = target.strip_prefix('!') {
        return Inline::Mention {
            label: special_mention(special, label),
            is_self: BROADCAST_NAMES.contains(&special),
        };
    }
    if !["https://", "http://", "mailto:", "slack://", "tel:"]
        .iter()
        .any(|scheme| target.starts_with(scheme))
    {
        return Inline::Text {
            text: format!("<{}>", unescape(token)),
            style: TextStyle::default(),
        };
    }
    Inline::Link {
        label: unescape(label.unwrap_or(target.strip_prefix(MAILTO).unwrap_or(target))),
        url: unescape(target),
        style: TextStyle::default(),
    }
}

/// Slack markup made readable for editing: mentions show as names and entities
/// are unescaped. `restore` turns the edited text back into Slack markup.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditableText {
    pub text: String,
    /// Visible label and the markup it stands for, longest label first.
    tokens: Vec<(String, String)>,
}

impl EditableText {
    pub fn new(raw: &str, names: &dyn NameLookup) -> Self {
        let mut text = String::new();
        let mut tokens: Vec<(String, String)> = Vec::new();
        let mut rest = raw;
        while let Some(start) = rest.find('<') {
            let Some(length) = rest[start..].find('>') else {
                break;
            };
            text.push_str(&unescape(&rest[..start]));
            let markup = &rest[start..start + length + 1];
            let token = &markup[1..markup.len() - 1];
            match editable_label(token, names) {
                // Bare links are linked again by Slack; nothing to restore.
                Some(EditLabel::Plain(url)) => text.push_str(&url),
                Some(EditLabel::Token(label)) => {
                    if !tokens.iter().any(|(existing, _)| *existing == label) {
                        tokens.push((label.clone(), markup.into()));
                    }
                    text.push_str(&label);
                }
                None => {
                    if !tokens.iter().any(|(existing, _)| existing == markup) {
                        tokens.push((markup.into(), markup.into()));
                    }
                    text.push_str(markup);
                }
            }
            rest = &rest[start + length + 1..];
        }
        text.push_str(&unescape(rest));
        tokens.sort_by_key(|(label, _)| std::cmp::Reverse(label.len()));
        Self { text, tokens }
    }

    pub fn restore(&self, edited: &str) -> String {
        let mut markup = String::with_capacity(edited.len());
        let mut rest = edited;
        'outer: while let Some(next) = rest.chars().next() {
            for (label, token) in &self.tokens {
                if let Some(after) = rest.strip_prefix(label.as_str())
                    && !after
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_alphanumeric() || c == '_')
                {
                    markup.push_str(token);
                    rest = after;
                    continue 'outer;
                }
            }
            match next {
                '&' => markup.push_str("&amp;"),
                '<' => markup.push_str("&lt;"),
                '>' => markup.push_str("&gt;"),
                c => markup.push(c),
            }
            rest = &rest[next.len_utf8()..];
        }
        markup
    }
}

enum EditLabel {
    Plain(String),
    Token(String),
}

fn editable_label(token: &str, names: &dyn NameLookup) -> Option<EditLabel> {
    let (target, label) = match token.split_once('|') {
        Some((target, label)) => (target, Some(label)),
        None => (token, None),
    };
    if target.starts_with('@') || target.starts_with('#') || target.starts_with('!') {
        return match parse_token(token, names) {
            Inline::Mention { label, .. } => Some(EditLabel::Token(label)),
            _ => None,
        };
    }
    let url = unescape(target);
    (["https://", "http://"]
        .iter()
        .any(|scheme| url.starts_with(scheme))
        && label.is_none_or(|label| unescape(label) == url))
    .then_some(EditLabel::Plain(url))
}

fn special_mention(special: &str, label: Option<&str>) -> String {
    if let Some(label) = label {
        return label.to_string();
    }
    match special.split_once('^') {
        Some((name, _)) => format!("@{name}"),
        None => format!("@{special}"),
    }
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Supplementary Private Use Area: never sent in real messages.
const PLACEHOLDER_BASE: u32 = 0xF0000;
const CODE_FENCE: &str = "```";
const ESCAPED_QUOTE: &str = "&gt;";
const MAILTO: &str = "mailto:";
pub(crate) const STYLE_MARKERS: [char; 3] = ['*', '_', '~'];
const SKIN_TONE_PREFIX: &str = "skin-tone-";
const MAX_EMOJI_NAME_LENGTH: usize = 64;
const BROADCAST_NAMES: [&str; 3] = ["here", "channel", "everyone"];
const BROADCAST_MENTIONS: [&str; 3] = ["<!here", "<!channel", "<!everyone"];

#[cfg(test)]
mod tests {
    use super::*;

    struct Names;

    impl NameLookup for Names {
        fn user_name(&self, user_id: &str) -> String {
            format!("name-{user_id}")
        }
        fn conversation_name(&self, conversation_id: &str) -> String {
            format!("chan-{conversation_id}")
        }
        fn is_self(&self, user_id: &str) -> bool {
            user_id == "UME"
        }
    }

    fn text(value: &str) -> Inline {
        Inline::Text {
            text: value.to_string(),
            style: TextStyle::default(),
        }
    }

    fn with_style(value: &str, style: TextStyle) -> Inline {
        Inline::Text {
            text: value.to_string(),
            style,
        }
    }

    #[test]
    fn parses_mentions_and_links() {
        let blocks = parse_message_text(
            "hi <@U1> and <@UME> see <https://x.io/a_b|docs> in <#C1>",
            &Names,
        );
        assert_eq!(
            blocks,
            vec![Block::Paragraph(vec![
                text("hi "),
                Inline::Mention {
                    label: "@name-U1".into(),
                    is_self: false
                },
                text(" and "),
                Inline::Mention {
                    label: "@name-UME".into(),
                    is_self: true
                },
                text(" see "),
                Inline::Link {
                    label: "docs".into(),
                    url: "https://x.io/a_b".into(),
                    style: TextStyle::default(),
                },
                text(" in "),
                Inline::Mention {
                    label: "#chan-C1".into(),
                    is_self: false
                },
            ])]
        );
    }

    #[test]
    fn clipboard_keeps_the_full_post_with_line_breaks_code_and_link_targets() {
        let long_body = "A line well below the preview.\n".repeat(100);
        let blocks = parse_message_text(
            &format!(
                "*Report* :rocket: <@U1>\n{long_body}\n<https://example.com/report|View report>\n<https://example.com>\n```first();\n  second();```\n&gt; quoted text\nLast line :workspace_custom:"
            ),
            &Names,
        );
        assert_eq!(
            clipboard_text(&blocks),
            format!(
                "Report 🚀 @name-U1\n{long_body}\nView report (https://example.com/report)\nhttps://example.com\nfirst();\n  second();\nquoted text\nLast line :workspace_custom:"
            )
        );
    }

    #[test]
    fn parses_styles() {
        let blocks = parse_message_text("a *bold* _it_ ~gone~ `co*de*` snake_case_name", &Names);
        assert_eq!(
            blocks,
            vec![Block::Paragraph(vec![
                text("a "),
                with_style(
                    "bold",
                    TextStyle {
                        bold: true,
                        ..TextStyle::default()
                    }
                ),
                text(" "),
                with_style(
                    "it",
                    TextStyle {
                        italic: true,
                        ..TextStyle::default()
                    }
                ),
                text(" "),
                with_style(
                    "gone",
                    TextStyle {
                        strike: true,
                        ..TextStyle::default()
                    }
                ),
                text(" "),
                with_style(
                    "co*de*",
                    TextStyle {
                        code: true,
                        ..TextStyle::default()
                    }
                ),
                text(" snake_case_name"),
            ])]
        );
    }

    #[test]
    fn parses_code_blocks_and_quotes() {
        let blocks = parse_message_text("intro\n```let x = 1;```\n&gt; quoted\nafter", &Names);
        assert_eq!(
            blocks,
            vec![
                Block::Paragraph(vec![text("intro")]),
                Block::Code("let x = 1;".into()),
                Block::Quote(vec![text("quoted")]),
                Block::Paragraph(vec![text("after")]),
            ]
        );
    }

    #[test]
    fn traceback_omission_markers_are_literal_text_not_links() {
        let blocks = parse_message_text("Traceback ...<4 lines>... end", &Names);
        assert_eq!(plain_text(&blocks), "Traceback ...<4 lines>... end");
        assert!(!blocks.iter().any(|block| matches!(block, Block::Paragraph(inlines) if inlines.iter().any(|inline| matches!(inline, Inline::Link { .. })))));
    }

    #[test]
    fn styles_wrap_links_and_link_labels_show_emoji() {
        let bold = TextStyle {
            bold: true,
            ..TextStyle::default()
        };
        let link = |label: &str| Inline::Link {
            label: label.into(),
            url: "https://fly.io/apps/a".into(),
            style: bold,
        };
        assert_eq!(
            parse_message_text(
                "*<https://fly.io/apps/a|:rotating_light: metrics (sjc)>* &amp; _`x`_",
                &Names
            ),
            vec![Block::Paragraph(vec![
                Inline::Emoji("rotating_light".into()),
                link(" metrics (sjc)"),
                text(" & "),
                with_style(
                    "x",
                    TextStyle {
                        italic: true,
                        code: true,
                        ..TextStyle::default()
                    }
                ),
            ])]
        );
    }

    #[test]
    fn parses_emoji_and_ignores_times() {
        let blocks = parse_message_text("ok :thumbsup::skin-tone-2: at 10:30:00", &Names);
        assert_eq!(
            blocks,
            vec![Block::Paragraph(vec![
                text("ok "),
                Inline::Emoji("thumbsup::skin-tone-2".into()),
                text(" at 10:30:00")
            ])]
        );
    }

    #[test]
    fn editable_text_shows_names_and_restores_markup() {
        let raw = "<@U1> see <#C2> &amp; <https://a.test/x?a=1&amp;b=2> or <https://b.test|docs>, a &lt; b <!here>";
        let editable = EditableText::new(raw, &Names);
        assert_eq!(
            editable.text,
            "@name-U1 see #chan-C2 & https://a.test/x?a=1&b=2 or <https://b.test|docs>, a < b @here"
        );
        // Bare links go back as plain URLs, which Slack links again.
        assert_eq!(
            editable.restore(&editable.text),
            raw.replace(
                "<https://a.test/x?a=1&amp;b=2>",
                "https://a.test/x?a=1&amp;b=2"
            )
        );
        assert_eq!(
            editable.restore("@name-U1x and @name-U1, 1 > 0"),
            "@name-U1x and <@U1>, 1 &gt; 0"
        );
    }

    #[test]
    fn detects_mentions() {
        assert!(mentions_user("hey <@UME>", "UME"));
        assert!(mentions_user("<!here> deploy", "UME"));
        assert!(!mentions_user("hey <@U2>", "UME"));
    }
}
