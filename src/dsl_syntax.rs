// =========================================
// =========================================
// crates/motionloom/src/dsl_syntax.rs

//! Source scanners shared by parsing and lossless formatting.

use serde::{Deserialize, Serialize};
use std::ops::Range;
use thiserror::Error;

const MAX_DEPTH: usize = 512;

/// Structural source error; formatting never returns partially rewritten input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
#[error("line {line}, column {column}: {message}")]
pub struct FormatError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

#[derive(Debug)]
pub(crate) struct Tag<'a> {
    pub(crate) name: &'a str,
    pub(crate) prefix_end: usize,
    pub(crate) attributes: Vec<Range<usize>>,
    pub(crate) tail_start: usize,
    pub(crate) closing: bool,
    pub(crate) self_closing: bool,
    pub(crate) preserve: bool,
}

#[derive(Debug)]
pub(crate) enum Kind<'a> {
    Tag(Tag<'a>),
    Comment,
    Text,
    Cdata,
    Declaration,
}

#[derive(Debug)]
pub(crate) struct Token<'a> {
    pub(crate) span: Range<usize>,
    pub(crate) kind: Kind<'a>,
    pub(crate) depth: usize,
    // A mixed-content element is emitted as one untouched source range.
    pub(crate) protected_end: Option<usize>,
}

fn error(source: &str, position: usize, message: impl Into<String>) -> FormatError {
    let prefix = &source[..position];
    FormatError {
        line: prefix.bytes().filter(|&b| b == b'\n').count() + 1,
        column: prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1,
        message: message.into(),
    }
}

fn skip_space(source: &str, mut at: usize, end: usize) -> usize {
    while at < end {
        let ch = source[at..end].chars().next().unwrap();
        if !ch.is_whitespace() {
            break;
        }
        at += ch.len_utf8();
    }
    at
}

fn name_end(source: &str, at: usize, end: usize) -> usize {
    let mut cursor = at;
    for ch in source[at..end].chars() {
        if ch.is_alphanumeric() || matches!(ch, '_' | ':' | '-' | '.') {
            cursor += ch.len_utf8();
        } else {
            break;
        }
    }
    cursor
}

fn parse_tag(source: &str, start: usize, end: usize) -> Result<Tag<'_>, FormatError> {
    let closing = source[start..].starts_with("</");
    let name_start = start + if closing { 2 } else { 1 };
    let prefix_end = name_end(source, name_start, end - 1);
    if prefix_end == name_start {
        return Err(error(source, start, "Expected a tag name."));
    }
    let name = &source[name_start..prefix_end];
    let self_closing = !closing && source[..end - 1].ends_with('/');
    let tail_start = end - if self_closing { 2 } else { 1 };
    let mut cursor = prefix_end;
    let mut attributes = Vec::new();
    let mut preserve = false;
    let mut has_text_value = false;
    while cursor < tail_start {
        let next = skip_space(source, cursor, tail_start);
        if next == tail_start {
            break;
        }
        if closing || next == cursor {
            return Err(error(
                source,
                cursor,
                "Expected whitespace between attributes.",
            ));
        }
        cursor = next;
        let attr_start = cursor;
        let attr_name_end = name_end(source, cursor, tail_start);
        if attr_name_end == cursor {
            return Err(error(source, cursor, "Expected an attribute name."));
        }
        cursor = skip_space(source, attr_name_end, tail_start);
        let assignment = source[cursor..tail_start].chars().next();
        if !matches!(assignment, Some('=' | '＝')) {
            return Err(error(
                source,
                cursor,
                "Expected '=' after the attribute name.",
            ));
        }
        cursor += assignment.unwrap().len_utf8();
        cursor = skip_space(source, cursor, tail_start);
        let value_start = cursor;
        let mut state = TagScanState::default();
        let delimited = source[value_start..tail_start].starts_with(['\'', '"', '{']);
        for (offset, ch) in source[value_start..tail_start].char_indices() {
            if ch.is_whitespace() && state.at_boundary() {
                break;
            }
            state.consume(ch).ok_or_else(|| {
                error(
                    source,
                    value_start + offset,
                    "Unbalanced attribute expression.",
                )
            })?;
            cursor = value_start + offset + ch.len_utf8();
            if delimited && state.at_boundary() {
                break;
            }
        }
        if cursor == value_start || !state.at_boundary() {
            return Err(error(source, value_start, "Incomplete attribute value."));
        }
        if &source[attr_start..attr_name_end] == "xml:space" {
            preserve = matches!(&source[value_start..cursor], "\"preserve\"" | "'preserve'");
        }
        has_text_value |= &source[attr_start..attr_name_end] == "value";
        attributes.push(attr_start..cursor);
    }
    Ok(Tag {
        name,
        prefix_end,
        attributes,
        tail_start,
        closing,
        self_closing,
        preserve: preserve || (name == "Text" && !has_text_value),
    })
}

// Scan complete source before emitting anything, including fragments with multiple roots.
pub(crate) fn scan_source(source: &str) -> Result<Vec<Token<'_>>, FormatError> {
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < source.len() {
        let (end, kind) = if source[at..].starts_with("<!--") {
            let end = source[at + 4..]
                .find("-->")
                .map(|n| at + 4 + n + 3)
                .ok_or_else(|| error(source, at, "Unclosed comment."))?;
            (end, Kind::Comment)
        } else if source[at..].starts_with("<![CDATA[") {
            let end = source[at + 9..]
                .find("]]>")
                .map(|n| at + 9 + n + 3)
                .ok_or_else(|| error(source, at, "Unclosed CDATA section."))?;
            (end, Kind::Cdata)
        } else if source[at..].starts_with("<?") {
            let end = find_tag_end_byte(source, at)
                .ok_or_else(|| error(source, at, "Unclosed processing instruction."))?
                + 1;
            if !source[..end].ends_with("?>") {
                return Err(error(
                    source,
                    at,
                    "Processing instruction must end with '?>'.",
                ));
            }
            (end, Kind::Declaration)
        } else if source[at..].starts_with('<') {
            let end = find_tag_end_byte(source, at)
                .ok_or_else(|| error(source, at, "Unclosed tag, quote or expression."))?
                + 1;
            (end, Kind::Tag(parse_tag(source, at, end)?))
        } else if source[at..].starts_with("//")
            && source[..at]
                .rsplit('\n')
                .next()
                .unwrap_or("")
                .trim()
                .is_empty()
        {
            let end = source[at..].find('\n').map_or(source.len(), |n| at + n);
            (end, Kind::Comment)
        } else {
            let mut end = at;
            for (offset, ch) in source[at..].char_indices() {
                let position = at + offset;
                if ch == '<'
                    || (source[position..].starts_with("//")
                        && source[..position]
                            .rsplit('\n')
                            .next()
                            .unwrap_or("")
                            .trim()
                            .is_empty())
                {
                    break;
                }
                end = position + ch.len_utf8();
            }
            (end, Kind::Text)
        };
        tokens.push(Token {
            span: at..end,
            kind,
            depth: 0,
            protected_end: None,
        });
        at = end;
    }
    let mut stack: Vec<(usize, bool)> = Vec::new();
    for index in 0..tokens.len() {
        tokens[index].depth = stack.len();
        match &tokens[index].kind {
            Kind::Tag(tag) if tag.closing => {
                let Some((open, preserve)) = stack.pop() else {
                    return Err(error(
                        source,
                        tokens[index].span.start,
                        "Unexpected closing tag.",
                    ));
                };
                let Kind::Tag(open_tag) = &tokens[open].kind else {
                    unreachable!()
                };
                if open_tag.name != tag.name {
                    return Err(error(
                        source,
                        tokens[index].span.start,
                        format!("Expected </{}>, found </{}>.", open_tag.name, tag.name),
                    ));
                }
                tokens[index].depth = stack.len();
                if preserve {
                    tokens[open].protected_end = Some(index);
                }
            }
            Kind::Tag(tag) if !tag.self_closing => {
                if stack.len() >= MAX_DEPTH {
                    return Err(error(
                        source,
                        tokens[index].span.start,
                        "Maximum formatting depth exceeded.",
                    ));
                }
                stack.push((index, tag.preserve));
            }
            Kind::Cdata | Kind::Text => {
                let meaningful = matches!(tokens[index].kind, Kind::Cdata)
                    || !source[tokens[index].span.clone()].trim().is_empty();
                if meaningful {
                    if let Some((_, preserve)) = stack.last_mut() {
                        *preserve = true;
                    } else {
                        return Err(error(
                            source,
                            tokens[index].span.start,
                            "Text content must belong to an element.",
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    if let Some((open, _)) = stack.last() {
        let Kind::Tag(tag) = &tokens[*open].kind else {
            unreachable!()
        };
        return Err(error(
            source,
            tokens[*open].span.start,
            format!("Missing </{}>.", tag.name),
        ));
    }
    Ok(tokens)
}

pub(crate) struct LogicalSource {
    pub(crate) source: String,
    original_lines: Vec<usize>,
}

impl LogicalSource {
    pub(crate) fn original_line(&self, line: usize) -> usize {
        if line == 0 {
            return 0;
        }
        self.original_lines.get(line - 1).copied().unwrap_or(line)
    }
}

/// Give line-oriented parsers one structural tag per logical line without changing payloads.
/// Diagnostic lines map back to the caller's source rather than the inserted line breaks.
pub(crate) fn logical_tag_lines(source: &str) -> Result<LogicalSource, FormatError> {
    let tokens = scan_source(source)?;
    let mut output = String::with_capacity(source.len());
    let mut original_lines = vec![1];
    let mut original_line = 1;
    let mut cursor = 0;
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        if matches!(token.kind, Kind::Text) {
            index += 1;
            continue;
        }
        append_logical(
            &source[cursor..token.span.start],
            &mut output,
            &mut original_lines,
            &mut original_line,
        );
        if !output.rsplit('\n').next().unwrap_or("").trim().is_empty() {
            output.push('\n');
            original_lines.push(original_line);
        }
        let end = token.protected_end.unwrap_or(index);
        append_logical(
            &source[token.span.start..tokens[end].span.end],
            &mut output,
            &mut original_lines,
            &mut original_line,
        );
        cursor = tokens[end].span.end;
        index = end + 1;
    }
    append_logical(
        &source[cursor..],
        &mut output,
        &mut original_lines,
        &mut original_line,
    );
    Ok(LogicalSource {
        source: output,
        original_lines,
    })
}

fn append_logical(
    part: &str,
    output: &mut String,
    lines: &mut Vec<usize>,
    original_line: &mut usize,
) {
    output.push_str(part);
    for byte in part.bytes() {
        if byte == b'\n' {
            *original_line += 1;
            lines.push(*original_line);
        }
    }
}

#[derive(Default)]
pub(crate) struct TagScanState {
    quote: Option<char>,
    escaped: bool,
    braces: usize,
}

impl TagScanState {
    pub(crate) fn at_boundary(&self) -> bool {
        self.quote.is_none() && self.braces == 0
    }

    pub(crate) fn consume(&mut self, ch: char) -> Option<()> {
        if let Some(expected) = self.quote {
            if self.escaped {
                self.escaped = false;
            } else if ch == '\\' {
                self.escaped = true;
            } else if ch == expected {
                self.quote = None;
            }
        } else {
            match ch {
                '"' | '\'' => self.quote = Some(ch),
                '{' => self.braces += 1,
                '}' => self.braces = self.braces.checked_sub(1)?,
                _ => {}
            }
        }
        Some(())
    }
}

/// Find a tag boundary without treating quoted or expression operators as markup.
pub(crate) fn find_tag_end_byte(input: &str, start: usize) -> Option<usize> {
    let mut state = TagScanState::default();
    for (offset, ch) in input.get(start..)?.char_indices() {
        if ch == '>' && state.at_boundary() {
            return Some(start + offset);
        }
        state.consume(ch)?;
    }
    None
}

pub(crate) fn opening_tag_name(tag: &str) -> Option<&str> {
    let rest = tag.strip_prefix('<')?.trim_start();
    if rest.starts_with('/') || rest.starts_with('!') || rest.starts_with('?') {
        return None;
    }
    let end = rest
        .find(|ch: char| ch.is_whitespace() || ch == '>' || ch == '/')
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

pub(crate) fn closing_tag_name(tag: &str) -> Option<&str> {
    let rest = tag.strip_prefix("</")?.trim_start();
    let end = rest
        .find(|ch: char| ch.is_whitespace() || ch == '>')
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

pub(crate) fn is_raw_self_closing_tag(tag: &str) -> bool {
    tag.trim_end()
        .strip_suffix('>')
        .is_some_and(|body| body.trim_end().ends_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_end_respects_nested_expression_strings_and_escapes() {
        let tag = r#"<Rect x={if($time > 1, "}", 2)} value="a\" > b" />"#;
        assert_eq!(find_tag_end_byte(tag, 0), Some(tag.len() - 1));
        assert_eq!(find_tag_end_byte("<Rect x={1 />", 0), None);
    }
}
