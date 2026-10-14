//! Preserve Slack's visible message structure instead of its notification fallback.
use serde_json::Value;

pub fn message_text(message: &Value) -> String {
    let blocks = block_text(&message["blocks"]);
    let primary = if blocks.is_empty() {
        message["text"].as_str().unwrap_or("").to_owned()
    } else {
        blocks
    };
    let mut parts = vec![primary];
    for attachment in message["attachments"].as_array().into_iter().flatten() {
        // URL unfurls already have their own preview; don't repeat the linked page.
        if attachment["is_msg_unfurl"].as_bool() == Some(true)
            || attachment["is_app_unfurl"].as_bool() == Some(true)
        {
            continue;
        }
        let mut detail = Vec::new();
        if let Some(pretext) = attachment["pretext"].as_str() {
            detail.push(pretext.to_owned());
        }
        if let Some(title) = attachment["title"].as_str() {
            detail.push(match attachment["title_link"].as_str() {
                Some(url) => format!("*<{url}|{title}>*"),
                None => format!("*{title}*"),
            });
        }
        let blocks = block_text(&attachment["blocks"]);
        if !blocks.is_empty() {
            detail.push(blocks);
        } else if let Some(text) = attachment["text"].as_str() {
            detail.push(text.to_owned());
        }
        for field in attachment["fields"].as_array().into_iter().flatten() {
            let title = field["title"].as_str().unwrap_or("");
            let value = field["value"].as_str().unwrap_or("");
            detail.push(if title.is_empty() {
                value.into()
            } else {
                format!("*{title}*\n{value}")
            });
        }
        if detail.is_empty()
            && parts.iter().all(String::is_empty)
            && let Some(fallback) = attachment["fallback"].as_str()
        {
            detail.push(fallback.to_owned());
        }
        let detail = detail.join("\n\n");
        if !detail.is_empty() && !parts.iter().any(|part| part == &detail) {
            parts.push(detail);
        }
    }
    parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn block_text(blocks: &Value) -> String {
    blocks
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| {
            let text = match block["type"].as_str()? {
                "header" => format!("*{}*", text_object(&block["text"])),
                "section" => {
                    let mut parts = vec![text_object(&block["text"])];
                    parts.extend(
                        block["fields"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(text_object),
                    );
                    parts
                        .into_iter()
                        .filter(|part| !part.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n")
                }
                "markdown" => block["text"].as_str().unwrap_or("").into(),
                "rich_text" => rich_blocks(&block["elements"], 0),
                "context" => block["elements"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(text_object)
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · "),
                _ => return None,
            };
            (!text.is_empty()).then_some(text)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn text_object(value: &Value) -> String {
    let text = value["text"].as_str().unwrap_or("");
    if value["type"].as_str() == Some("plain_text") {
        escape(text)
    } else {
        text.into()
    }
}

fn rich_blocks(elements: &Value, depth: usize) -> String {
    if depth > 16 {
        return String::new();
    }
    elements
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|element| {
            let text = match element["type"].as_str()? {
                "rich_text_section" => rich_inlines(&element["elements"], false),
                "rich_text_preformatted" => {
                    format!("```\n{}\n```", rich_inlines(&element["elements"], true))
                }
                "rich_text_quote" => rich_inlines(&element["elements"], false)
                    .lines()
                    .map(|line| format!("> {line}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                "rich_text_list" => {
                    let ordered = element["style"].as_str() == Some("ordered");
                    let offset = element["offset"].as_u64().unwrap_or(0);
                    let indent =
                        "  ".repeat(element["indent"].as_u64().unwrap_or(0).min(8) as usize);
                    element["elements"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .enumerate()
                        .map(|(i, entry)| {
                            let prefix = if ordered {
                                format!("{}.", offset + i as u64 + 1)
                            } else {
                                "•".into()
                            };
                            let text = rich_blocks(&Value::Array(vec![entry.clone()]), depth + 1);
                            format!("{indent}{prefix} {text}")
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                }
                _ => return None,
            };
            Some(text)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn rich_inlines(elements: &Value, code: bool) -> String {
    elements
        .as_array()
        .into_iter()
        .flatten()
        .map(|element| {
            if code {
                return escape(
                    element["text"]
                        .as_str()
                        .or(element["url"].as_str())
                        .unwrap_or(""),
                );
            }
            match element["type"].as_str().unwrap_or("") {
                "text" => {
                    let mut text = escape(element["text"].as_str().unwrap_or(""));
                    for (style, marker) in [
                        ("code", "`"),
                        ("bold", "*"),
                        ("italic", "_"),
                        ("strike", "~"),
                    ] {
                        if element["style"][style].as_bool() == Some(true) {
                            text = format!("{marker}{text}{marker}");
                            if style == "code" {
                                break;
                            }
                        }
                    }
                    text
                }
                "link" => {
                    let url = element["url"].as_str().unwrap_or("");
                    let label = element["text"].as_str().unwrap_or(url);
                    format!("<{}|{}>", escape(url), escape(label))
                }
                "user" => format!("<@{}>", element["user_id"].as_str().unwrap_or("")),
                "channel" => format!("<#{}>", element["channel_id"].as_str().unwrap_or("")),
                "usergroup" => format!(
                    "<!subteam^{}>",
                    element["usergroup_id"].as_str().unwrap_or("")
                ),
                "broadcast" => format!("<!{}>", element["range"].as_str().unwrap_or("here")),
                "emoji" => format!(":{}:", element["name"].as_str().unwrap_or("")),
                _ => String::new(),
            }
        })
        .collect()
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::chat_service::ChatService;
    use crate::domain::message_text::Block;
    use crate::infrastructure::{demo::DemoGateway, slack_parsing::parse_message};
    use std::sync::Arc;

    #[test]
    fn bot_blocks_override_flat_notification_text_and_keep_traceback_whitespace() {
        let code = "Traceback (most recent call last):\n  File \"/app/example.py\", line 12\n    send_message()\n    ...<4 lines>...\nRuntimeError: example";
        let raw = serde_json::json!({
            "ts": "10.0", "bot_id": "B1", "text": "flattened notification fallback",
            "bot_profile": {"name": "Example Bot", "icons": {"image_72": "https://example.test/bot.png"}},
            "blocks": [
                {"type":"header", "text":{"type":"plain_text", "text":"🔴 Example service"}},
                {"type":"section", "text":{"type":"mrkdwn", "text":"Could not update the message."}},
                {"type":"section", "text":{"type":"mrkdwn", "text":format!("```{code}```")}}
            ]
        });
        let message = parse_message(&raw).unwrap();
        let service = ChatService::new(Arc::new(DemoGateway::new()));
        let display = service.display(&message);
        assert_eq!(display.author, "Example Bot");
        assert_eq!(
            display.avatar_url.as_deref(),
            Some("https://example.test/bot.png")
        );
        assert!(!message.text.contains("flattened"));
        assert!(message.text.contains("🔴 Example service"));
        assert_eq!(display.blocks.last(), Some(&Block::Code(code.into())));
    }

    #[test]
    fn rich_preformatted_and_legacy_attachment_content_are_preserved() {
        let raw = serde_json::json!({"blocks":[{"type":"rich_text", "elements":[
            {"type":"rich_text_section", "elements":[{"type":"text", "text":"Heading", "style":{"bold":true}}]},
            {"type":"rich_text_preformatted", "elements":[{"type":"text", "text":"  a < b\n    ...<9 lines>..."}]}
        ]}], "attachments":[{"title":"Details", "text":"```\n  legacy_code()\n```"}]});
        let text = message_text(&raw);
        assert!(text.contains("*Heading*"));
        assert!(text.contains("```\n  a &lt; b\n    ...&lt;9 lines&gt;...\n```"));
        assert!(text.contains("*Details*\n\n```\n  legacy_code()\n```"));
    }
}
