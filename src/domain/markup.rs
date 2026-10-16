//! Slack markup in the composer: how to draw it while typing, and the
//! formatting actions behind the toolbar and shortcuts. Positions are in chars.

use std::ops::Range;

use super::message_text::{STYLE_MARKERS, find_close, opens_at};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MarkupStyle {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub code_block: bool,
    pub quote: bool,
    pub list: bool,
    /// Markup characters such as `*` or `> `, drawn dimmed.
    pub marker: bool,
}

/// Byte ranges covering all of `text`, each with how to draw it. The rules
/// follow the message parser so the draft looks like the sent message.
pub fn markup_styles(text: &str) -> Vec<(Range<usize>, MarkupStyle)> {
    let chars: Vec<char> = text.chars().collect();
    let mut styles = vec![MarkupStyle::default(); chars.len()];
    let fence = MarkupStyle {
        code_block: true,
        marker: true,
        ..MarkupStyle::default()
    };
    let (mut index, mut in_code, mut prose_start) = (0, false, 0);
    while index < chars.len() {
        if chars[index..].starts_with(&FENCE) {
            if !in_code {
                prose(&chars[prose_start..index], &mut styles[prose_start..index]);
            }
            styles[index..index + FENCE.len()].fill(fence);
            in_code = !in_code;
            index += FENCE.len();
            prose_start = index;
            continue;
        }
        if in_code {
            styles[index].code_block = true;
        }
        index += 1;
    }
    if !in_code {
        prose(&chars[prose_start..], &mut styles[prose_start..]);
    }

    let mut runs: Vec<(Range<usize>, MarkupStyle)> = Vec::new();
    for ((byte, c), style) in text.char_indices().zip(styles) {
        match runs.last_mut() {
            Some((range, last)) if *last == style => range.end = byte + c.len_utf8(),
            _ => runs.push((byte..byte + c.len_utf8(), style)),
        }
    }
    runs
}

fn prose(chars: &[char], styles: &mut [MarkupStyle]) {
    let mut start = 0;
    for end in (0..=chars.len()).filter(|&i| i == chars.len() || chars[i] == '\n') {
        line(&chars[start..end], &mut styles[start..end]);
        start = end + 1;
    }
}

fn line(chars: &[char], styles: &mut [MarkupStyle]) {
    let mut base = MarkupStyle::default();
    let mut content = 0;
    if chars.first() == Some(&'>') {
        base.quote = true;
        content = if chars.get(1) == Some(&' ') { 2 } else { 1 };
        styles[..content].fill(MarkupStyle {
            marker: true,
            ..base
        });
    } else if let Some(prefix) = list_prefix(chars) {
        content = prefix;
        styles[..content].fill(MarkupStyle { list: true, ..base });
    }
    inline(&chars[content..], &mut styles[content..], base);
}

fn inline(chars: &[char], styles: &mut [MarkupStyle], base: MarkupStyle) {
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '`'
            && let Some(length) = chars[index + 1..].iter().position(|c| *c == '`')
            && length > 0
        {
            let close = index + 1 + length;
            let code = MarkupStyle { code: true, ..base };
            styles[index + 1..close].fill(code);
            styles[index] = MarkupStyle {
                marker: true,
                ..code
            };
            styles[close] = styles[index];
            index = close + 1;
            continue;
        }
        if STYLE_MARKERS.contains(&chars[index])
            && opens_at(chars, index)
            && let Some(close) = find_close(chars, index)
        {
            let style = match chars[index] {
                '*' => MarkupStyle { bold: true, ..base },
                '_' => MarkupStyle {
                    italic: true,
                    ..base
                },
                _ => MarkupStyle {
                    strike: true,
                    ..base
                },
            };
            styles[index] = MarkupStyle {
                marker: true,
                ..style
            };
            styles[close] = styles[index];
            inline(
                &chars[index + 1..close],
                &mut styles[index + 1..close],
                style,
            );
            index = close + 1;
            continue;
        }
        styles[index] = base;
        index += 1;
    }
}

/// Length of a list prefix such as "• ", "- " or "12. ", after any indent.
fn list_prefix(chars: &[char]) -> Option<usize> {
    let indent = chars.iter().take_while(|c| **c == ' ').count();
    let rest = &chars[indent..];
    let marker = match rest {
        ['•' | '-', ' ', ..] => 2,
        _ => {
            let digits = rest.iter().take_while(|c| c.is_ascii_digit()).count();
            if digits == 0 || !rest[digits..].starts_with(&['.', ' ']) {
                return None;
            }
            digits + 2
        }
    };
    Some(indent + marker)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Bold,
    Italic,
    Strike,
    Code,
    CodeBlock,
    Quote,
    Bullets,
    Numbered,
}

/// Apply or remove a format around the selection; returns the new text and selection.
pub fn apply_format(text: &str, selection: Range<usize>, format: Format) -> (String, Range<usize>) {
    let mut chars: Vec<char> = text.chars().collect();
    let selection = selection.start.min(chars.len())..selection.end.min(chars.len());
    let range = match format {
        Format::Bold => wrap(&mut chars, selection, '*'),
        Format::Italic => wrap(&mut chars, selection, '_'),
        Format::Strike => wrap(&mut chars, selection, '~'),
        Format::Code => wrap(&mut chars, selection, '`'),
        Format::CodeBlock => code_block(&mut chars, selection),
        Format::Quote | Format::Bullets | Format::Numbered => {
            prefix_lines(&mut chars, selection, format)
        }
    };
    (chars.into_iter().collect(), range)
}

fn wrap(chars: &mut Vec<char>, selection: Range<usize>, marker: char) -> Range<usize> {
    let (mut start, mut end) = (selection.start, selection.end);
    // Slack ignores markers next to spaces, so keep them on the words.
    while start < end && chars[start].is_whitespace() {
        start += 1;
    }
    while end > start && chars[end - 1].is_whitespace() {
        end -= 1;
    }
    if start == end {
        let at = selection.end;
        chars.splice(at..at, [marker, marker]);
        return at + 1..at + 1;
    }
    if start > 0 && chars.get(end) == Some(&marker) && chars[start - 1] == marker {
        chars.remove(end);
        chars.remove(start - 1);
        return start - 1..end - 1;
    }
    if end - start >= 2 && chars[start] == marker && chars[end - 1] == marker {
        chars.remove(end - 1);
        chars.remove(start);
        return start..end - 2;
    }
    chars.insert(end, marker);
    chars.insert(start, marker);
    start + 1..end + 1
}

fn code_block(chars: &mut Vec<char>, selection: Range<usize>) -> Range<usize> {
    let selected: String = chars[selection.clone()].iter().collect();
    if let Some(inner) = selected
        .strip_prefix("```")
        .and_then(|rest| rest.strip_suffix("```"))
    {
        let inner: Vec<char> = inner.trim_matches('\n').chars().collect();
        let length = inner.len();
        chars.splice(selection.clone(), inner);
        return selection.start..selection.start + length;
    }
    let mut block: Vec<char> = Vec::new();
    if selection.start > 0 && chars[selection.start - 1] != '\n' {
        block.push('\n');
    }
    block.extend("```\n".chars());
    let inner_start = selection.start + block.len();
    block.extend(selected.chars());
    let inner_end = selection.start + block.len();
    block.extend("\n```".chars());
    if chars.get(selection.end).is_some_and(|c| *c != '\n') {
        block.push('\n');
    }
    chars.splice(selection, block);
    inner_start..inner_end
}

fn prefix_lines(chars: &mut Vec<char>, selection: Range<usize>, format: Format) -> Range<usize> {
    let first = chars[..selection.start]
        .iter()
        .rposition(|c| *c == '\n')
        .map_or(0, |i| i + 1);
    // A selection ending at the start of a line does not include that line.
    let last_char = if selection.end > selection.start && chars[selection.end - 1] == '\n' {
        selection.end - 1
    } else {
        selection.end
    };
    let end = chars[last_char..]
        .iter()
        .position(|c| *c == '\n')
        .map_or(chars.len(), |i| last_char + i);
    let block: String = chars[first..end].iter().collect();
    let lines: Vec<&str> = block.split('\n').collect();
    let existing = |line: &str| -> Option<usize> {
        let chars: Vec<char> = line.chars().collect();
        match format {
            Format::Quote => line
                .starts_with('>')
                .then(|| if line.starts_with("> ") { 2 } else { 1 }),
            Format::Bullets => matches!(chars.as_slice(), ['•' | '-', ' ', ..]).then_some(2),
            _ => list_prefix(&chars).filter(|_| chars[0].is_ascii_digit()),
        }
    };
    let remove = lines.iter().all(|line| existing(line).is_some());
    let mut shift = 0isize;
    let changed: Vec<String> = lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let (prefix, kept) = if remove {
                let length = existing(line).unwrap_or(0);
                (String::new(), line.chars().skip(length).collect::<String>())
            } else {
                let prefix = match format {
                    Format::Quote => "> ".to_owned(),
                    Format::Bullets => "• ".to_owned(),
                    _ => format!("{}. ", index + 1),
                };
                (prefix, line.to_string())
            };
            if index == 0 {
                shift = prefix.chars().count() as isize
                    - (line.chars().count() - kept.chars().count()) as isize;
            }
            prefix + &kept
        })
        .collect();
    let changed: Vec<char> = changed.join("\n").chars().collect();
    let length = changed.len();
    chars.splice(first..end, changed);
    if selection.is_empty() {
        let cursor = (selection.start as isize + shift).max(first as isize) as usize;
        cursor..cursor
    } else {
        first..first + length
    }
}

/// Shift+Enter in a list or quote line continues it; on an empty item it ends the list.
pub fn continue_line(text: &str, cursor: usize) -> Option<(String, usize)> {
    let mut chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    let start = chars[..cursor]
        .iter()
        .rposition(|c| *c == '\n')
        .map_or(0, |i| i + 1);
    let line = &chars[start..cursor];
    let (length, next): (usize, Vec<char>) = if line.first() == Some(&'>') {
        let length = if line.get(1) == Some(&' ') { 2 } else { 1 };
        (length, "> ".chars().collect())
    } else {
        let length = list_prefix(line)?;
        let indent = line.iter().take_while(|c| **c == ' ').count();
        let marker: String = line[indent..length].iter().collect();
        let next = match marker.trim_end().strip_suffix('.') {
            Some(number) => format!("{}. ", number.parse::<u64>().ok()? + 1),
            None => marker,
        };
        (
            length,
            " ".repeat(indent).chars().chain(next.chars()).collect(),
        )
    };
    if line.len() == length {
        chars.drain(start..cursor);
        return Some((chars.into_iter().collect(), start));
    }
    let inserted = 1 + next.len();
    chars.splice(cursor..cursor, std::iter::once('\n').chain(next));
    Some((chars.into_iter().collect(), cursor + inserted))
}

const FENCE: [char; 3] = ['`', '`', '`'];

#[cfg(test)]
mod tests {
    use super::*;

    fn styled(text: &str, part: &str) -> MarkupStyle {
        let at = text.find(part).unwrap();
        markup_styles(text)
            .into_iter()
            .find(|(range, _)| range.contains(&at))
            .unwrap()
            .1
    }

    #[test]
    fn draft_markup_is_styled_like_the_sent_message() {
        let text = "say *bold _both_* and `co*de*`\n> quoted\n• item\n```\nlet *x*\n```";
        assert!(styled(text, "bold").bold);
        let both = styled(text, "both");
        assert!(both.bold && both.italic);
        assert!(styled(text, "*bold").marker);
        assert!(styled(text, "co*de").code && !styled(text, "co*de").bold);
        assert!(styled(text, "> ").marker && styled(text, "quoted").quote);
        assert!(styled(text, "• ").list);
        assert!(styled(text, "let").code_block && !styled(text, "x*").bold);
        assert_eq!(styled(text, "say"), MarkupStyle::default());
        let covered: usize = markup_styles(text).iter().map(|(r, _)| r.len()).sum();
        assert_eq!(covered, text.len());
    }

    #[test]
    fn wrapping_toggles_and_keeps_markers_on_the_words() {
        assert_eq!(
            apply_format("make this bold", 5..10, Format::Bold),
            ("make *this* bold".into(), 6..10)
        );
        assert_eq!(
            apply_format("make *this* bold", 6..10, Format::Bold),
            ("make this bold".into(), 5..9)
        );
        assert_eq!(
            apply_format("make *this* bold", 5..11, Format::Bold),
            ("make this bold".into(), 5..9)
        );
        assert_eq!(
            apply_format("hi ", 3..3, Format::Italic),
            ("hi __".into(), 4..4)
        );
        assert_eq!(
            apply_format("a ", 0..2, Format::Strike),
            ("~a~ ".into(), 1..2)
        );
    }

    #[test]
    fn code_blocks_and_line_prefixes_toggle() {
        assert_eq!(
            apply_format("see x()", 4..7, Format::CodeBlock),
            ("see \n```\nx()\n```".into(), 9..12)
        );
        let (text, range) = apply_format("one\ntwo", 1..5, Format::Numbered);
        assert_eq!((text.as_str(), range.clone()), ("1. one\n2. two", 0..13));
        assert_eq!(apply_format(&text, range, Format::Numbered).0, "one\ntwo");
        assert_eq!(
            apply_format("item", 4..4, Format::Bullets),
            ("• item".into(), 6..6)
        );
        assert_eq!(
            apply_format("> said", 2..2, Format::Quote),
            ("said".into(), 0..0)
        );
    }

    #[test]
    fn shift_enter_continues_lists_and_ends_them_on_empty_items() {
        assert_eq!(continue_line("• one", 5), Some(("• one\n• ".into(), 8)));
        assert_eq!(
            continue_line("9. nine", 7),
            Some(("9. nine\n10. ".into(), 12))
        );
        assert_eq!(continue_line("x\n• ", 4), Some(("x\n".into(), 2)));
        assert_eq!(continue_line("plain", 5), None);
    }
}
