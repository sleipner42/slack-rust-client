//! Slack stores drafts as rich-text blocks; the composer edits Slack markup.
//! Convert between the two for the constructs the composer can produce.

use serde_json::{Map, Value, json};

use crate::domain::markup::markup_styles;

/// Composer text from a draft's blocks.
pub fn draft_text(blocks: &Value) -> String {
    let text = super::block_text::rich_text_markup(blocks);
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// A draft's blocks from composer text.
pub fn draft_blocks(text: &str) -> Value {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut elements = Vec::new();
    let mut plain: Vec<&str> = Vec::new();
    let mut index = 0;
    let flush_plain = |plain: &mut Vec<&str>, elements: &mut Vec<Value>| {
        if !plain.is_empty() {
            elements.push(section(&plain.join("\n")));
            plain.clear();
        }
    };
    while index < lines.len() {
        let line = lines[index];
        if let Some(rest) = line.strip_prefix(FENCE) {
            flush_plain(&mut plain, &mut elements);
            // ```code``` on one line, or a fenced block over several.
            let mut code: Vec<&str> = Vec::new();
            if let Some(inner) = rest.strip_suffix(FENCE) {
                code.push(inner);
            } else {
                if !rest.is_empty() {
                    code.push(rest);
                }
                index += 1;
                while index < lines.len() {
                    match lines[index].strip_suffix(FENCE) {
                        Some(last) => {
                            if !last.is_empty() {
                                code.push(last);
                            }
                            break;
                        }
                        None => code.push(lines[index]),
                    }
                    index += 1;
                }
            }
            elements.push(json!({
                "type": "rich_text_preformatted",
                "elements": [{"type": "text", "text": code.join("\n")}],
            }));
            index += 1;
            continue;
        }
        if quote_body(line).is_some() {
            flush_plain(&mut plain, &mut elements);
            let mut quoted = Vec::new();
            while let Some(body) = lines.get(index).and_then(|line| quote_body(line)) {
                quoted.push(body);
                index += 1;
            }
            elements.push(json!({
                "type": "rich_text_quote",
                "elements": inlines(&quoted.join("\n")),
            }));
            continue;
        }
        if let Some((ordered, _)) = list_item(line) {
            flush_plain(&mut plain, &mut elements);
            let mut items = Vec::new();
            while let Some((same, body)) = lines.get(index).and_then(|line| list_item(line))
                && same == ordered
            {
                items.push(section(body));
                index += 1;
            }
            elements.push(json!({
                "type": "rich_text_list",
                "style": if ordered { "ordered" } else { "bullet" },
                "indent": 0,
                "elements": items,
            }));
            continue;
        }
        plain.push(line);
        index += 1;
    }
    flush_plain(&mut plain, &mut elements);
    json!([{"type": "rich_text", "elements": elements}])
}

const FENCE: &str = "```";

fn quote_body(line: &str) -> Option<&str> {
    line.strip_prefix("> ").or_else(|| line.strip_prefix('>'))
}

/// Whether a line is a list item (ordered or not), and its text.
fn list_item(line: &str) -> Option<(bool, &str)> {
    if let Some(body) = line.strip_prefix("• ").or_else(|| line.strip_prefix("- ")) {
        return Some((false, body));
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    (digits > 0)
        .then(|| line[digits..].strip_prefix(". "))
        .flatten()
        .map(|body| (true, body))
}

fn section(text: &str) -> Value {
    json!({"type": "rich_text_section", "elements": inlines(text)})
}

/// Styled text runs, with mentions, links and emoji as their own elements.
fn inlines(text: &str) -> Vec<Value> {
    let mut elements: Vec<Value> = Vec::new();
    for (range, style) in markup_styles(text) {
        if style.marker {
            continue;
        }
        let mut flags = Map::new();
        for (name, on) in [
            ("bold", style.bold),
            ("italic", style.italic),
            ("strike", style.strike),
            ("code", style.code),
        ] {
            if on {
                flags.insert(name.into(), Value::Bool(true));
            }
        }
        for piece in tokens(&text[range], style.code) {
            match piece {
                Token::Text(text) => {
                    let mut element = json!({"type": "text", "text": text});
                    if !flags.is_empty() {
                        element["style"] = Value::Object(flags.clone());
                    }
                    // Merge with the previous run when nothing differs.
                    if let Some(last) = elements.last_mut()
                        && last["type"] == "text"
                        && last.get("style") == element.get("style")
                    {
                        let joined = format!("{}{text}", last["text"].as_str().unwrap_or(""));
                        last["text"] = Value::String(joined);
                    } else {
                        elements.push(element);
                    }
                }
                Token::Element(element) => elements.push(element),
            }
        }
    }
    elements
}

enum Token {
    Text(String),
    Element(Value),
}

/// Split `<@U1>`, `<#C1>`, `<url|label>` and `:emoji:` out of plain text.
fn tokens(text: &str, code: bool) -> Vec<Token> {
    if code {
        return vec![Token::Text(text.to_owned())];
    }
    let mut tokens = Vec::new();
    let mut rest = text;
    let mut plain = String::new();
    while !rest.is_empty() {
        if let Some((element, length)) = special(rest) {
            if !plain.is_empty() {
                tokens.push(Token::Text(std::mem::take(&mut plain)));
            }
            tokens.push(Token::Element(element));
            rest = &rest[length..];
            continue;
        }
        let ch = rest.chars().next().unwrap();
        plain.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    if !plain.is_empty() {
        tokens.push(Token::Text(plain));
    }
    tokens
}

fn special(text: &str) -> Option<(Value, usize)> {
    if let Some(inner) = text.strip_prefix('<') {
        let end = inner.find('>')?;
        let body = &inner[..end];
        let length = end + 2;
        let element = if let Some(user) = body.strip_prefix('@') {
            json!({"type": "user", "user_id": user.split('|').next()?})
        } else if let Some(channel) = body.strip_prefix('#') {
            json!({"type": "channel", "channel_id": channel.split('|').next()?})
        } else if let Some(range) = body.strip_prefix('!') {
            if let Some(group) = range.strip_prefix("subteam^") {
                json!({"type": "usergroup", "usergroup_id": group.split('|').next()?})
            } else {
                json!({"type": "broadcast", "range": range.split('|').next()?})
            }
        } else if body.starts_with("http") || body.starts_with("mailto:") {
            match body.split_once('|') {
                Some((url, label)) => json!({"type": "link", "url": url, "text": label}),
                None => json!({"type": "link", "url": body}),
            }
        } else {
            return None;
        };
        return Some((element, length));
    }
    let inner = text.strip_prefix(':')?;
    let end = inner.find(':')?;
    let name = &inner[..end];
    (!name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '+' | '-')))
    .then(|| (json!({"type": "emoji", "name": name}), end + 2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composer_markup_survives_a_round_trip_through_blocks() {
        for text in [
            "plain text\nsecond line",
            "*bold* and _italic_ and ~gone~ and `code`",
            "• one\n• two\nafter",
            "1. first\n2. second",
            "> quoted\n> twice",
            "```\nfn main() {}\n```",
            "hi <@U123> see <https://x.test|the docs> :tada: <#C9>",
        ] {
            assert_eq!(draft_text(&draft_blocks(text)), text, "{text}");
        }
    }

    #[test]
    fn styled_runs_become_text_elements_with_flags() {
        let blocks = draft_blocks("a *b* c");
        assert_eq!(
            blocks[0]["elements"][0]["elements"],
            json!([
                {"type": "text", "text": "a "},
                {"type": "text", "text": "b", "style": {"bold": true}},
                {"type": "text", "text": " c"},
            ])
        );
    }
}
