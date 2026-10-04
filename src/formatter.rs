// =========================================
// =========================================
// src/formatter.rs

//! Deterministic formatting that retains the original DSL values and content.

use serde::{Deserialize, Serialize};

pub use crate::dsl_syntax::FormatError;
use crate::dsl_syntax::{Kind, Tag, Token, scan_source};

const INDENT: usize = 2;
const LINE_WIDTH: usize = 120;

/// One source edit. Byte offsets serve Rust; UTF-16 offsets serve browser editors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatEdit {
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_utf16: usize,
    pub end_utf16: usize,
    pub replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatResult {
    pub source: String,
    pub changed: bool,
    /// Nonoverlapping edits in original-source order; apply from last to first.
    pub edits: Vec<FormatEdit>,
}

struct Writer<'a> {
    input: &'a str,
    output: String,
    cursor: usize,
    edits: Vec<FormatEdit>,
}

impl Writer<'_> {
    fn replace_until(&mut self, end: usize, replacement: &str) {
        if &self.input[self.cursor..end] != replacement {
            self.edits.push(FormatEdit {
                start_byte: self.cursor,
                end_byte: end,
                start_utf16: 0,
                end_utf16: 0,
                replacement: replacement.into(),
            });
        }
        self.output.push_str(replacement);
        self.cursor = end;
    }

    fn retain_until(&mut self, end: usize) {
        self.output.push_str(&self.input[self.cursor..end]);
        self.cursor = end;
    }

    fn tag(&mut self, token: &Token<'_>, tag: &Tag<'_>) {
        self.retain_until(tag.prefix_end);
        let width = token.depth * INDENT
            + tag.name.chars().count()
            + 1
            + tag
                .attributes
                .iter()
                .map(|r| self.input[r.clone()].chars().count() + 1)
                .sum::<usize>()
            + if tag.self_closing { 3 } else { 1 };
        let multiline = !matches!(tag.name, "Vertex" | "Face") && width > LINE_WIDTH;
        for attribute in &tag.attributes {
            let separator = if multiline {
                format!("\n{}", " ".repeat((token.depth + 1) * INDENT))
            } else {
                " ".into()
            };
            self.replace_until(attribute.start, &separator);
            self.retain_until(attribute.end);
        }
        let separator = if multiline && !tag.attributes.is_empty() {
            format!("\n{}", " ".repeat(token.depth * INDENT))
        } else if tag.self_closing {
            " ".into()
        } else {
            String::new()
        };
        self.replace_until(tag.tail_start, &separator);
        self.retain_until(token.span.end);
    }
}

/// Format structure without loading assets or interpreting numeric/string values.
/// Attribute values, comments and mixed-content element bodies retain their bytes.
pub fn format_dsl(source: &str) -> Result<FormatResult, FormatError> {
    let tokens = scan_source(source)?;
    let mut writer = Writer {
        input: source,
        output: String::with_capacity(source.len()),
        cursor: 0,
        edits: Vec::new(),
    };
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        if matches!(token.kind, Kind::Text) {
            index += 1;
            continue;
        }
        let gap = &source[writer.cursor..token.span.start];
        let mut separator = String::new();
        if !writer.output.is_empty() {
            separator.push('\n');
            if gap.bytes().filter(|&b| b == b'\n').count() >= 2 {
                separator.push('\n');
            }
        }
        separator.push_str(&" ".repeat(token.depth * INDENT));
        writer.replace_until(token.span.start, &separator);
        if let Some(end) = token.protected_end {
            writer.retain_until(tokens[end].span.end);
            index = end + 1;
            continue;
        }
        match &token.kind {
            Kind::Tag(tag) => writer.tag(token, tag),
            _ => writer.retain_until(token.span.end),
        }
        index += 1;
    }
    let tail = if writer.output.is_empty() { "" } else { "\n" };
    writer.replace_until(source.len(), tail);
    // Convert offsets once in source order; never rescan large mesh prefixes per edit.
    let mut byte_cursor = 0;
    let mut utf16_cursor = 0;
    for edit in &mut writer.edits {
        utf16_cursor += source[byte_cursor..edit.start_byte].encode_utf16().count();
        edit.start_utf16 = utf16_cursor;
        utf16_cursor += source[edit.start_byte..edit.end_byte]
            .encode_utf16()
            .count();
        edit.end_utf16 = utf16_cursor;
        byte_cursor = edit.end_byte;
    }
    Ok(FormatResult {
        changed: !writer.edits.is_empty(),
        source: writer.output,
        edits: writer.edits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(source: &str) -> FormatResult {
        let result = format_dsl(source).unwrap();
        assert!(!format_dsl(&result.source).unwrap().changed);
        let mut patched = source.to_string();
        for edit in result.edits.iter().rev() {
            patched.replace_range(edit.start_byte..edit.end_byte, &edit.replacement);
        }
        assert_eq!(patched, result.source);
        for edit in &result.edits {
            assert!(
                source[edit.start_byte..edit.end_byte]
                    .chars()
                    .all(char::is_whitespace)
            );
            assert!(edit.replacement.chars().all(char::is_whitespace));
        }
        result
    }

    #[test]
    fn compact_fragments_and_blank_lines_are_deterministic() {
        assert_eq!(
            check("<A><B x={[1,2,3]}/><C/></A><D/>").source,
            "<A>\n  <B x={[1,2,3]} />\n  <C />\n</A>\n<D />\n"
        );
        assert_eq!(
            check("\r\n<A>\r\n\r\n\r\n<B />\r\n</A>\r\n").source,
            "<A>\n\n  <B />\n</A>\n"
        );
    }

    #[test]
    fn values_comments_and_meaningful_text_keep_their_bytes() {
        let body = r#"<Text>  蘋果 <B /> \n </Text>"#;
        let comment = "<!-- first\r\n  <Fake /> 最後 -->";
        let input =
            format!("<A>{comment}<B x={{if($time > 1, \"}}\", 1.00)}} v='a > b' />{body}</A>");
        let result = check(&input);
        assert!(result.source.contains(comment));
        assert!(result.source.contains(body));
        assert!(result.source.contains("x={if($time > 1, \"}\", 1.00)}"));
        assert!(
            check("<A><![CDATA[a < b\r\n ]]></A>")
                .source
                .starts_with("<A><![CDATA[a < b\r\n ]]></A>")
        );
        assert!(
            check("<A xml:space=\"preserve\">   <B/>   </A>")
                .source
                .starts_with("<A xml:space=\"preserve\">   <B/>   </A>")
        );
    }

    #[test]
    fn attribute_based_text_formats_structural_style_children() {
        assert_eq!(
            check("<Text value=\"apple\"><TextLayout/><TextGlow/></Text>").source,
            "<Text value=\"apple\">\n  <TextLayout />\n  <TextGlow />\n</Text>\n"
        );
        assert_eq!(check("<Text>  </Text>").source, "<Text>  </Text>\n");
    }

    #[test]
    fn long_tags_wrap_but_vertex_face_and_values_do_not() {
        let value = "x".repeat(150);
        let result = check(&format!(
            "<A id=\"a\" value=\"{value}\"/><Vertex data=\"{value}\"/><Face data=\"{value}\"/>"
        ));
        assert!(result.source.starts_with("<A\n  id=\"a\"\n  value=\""));
        assert!(
            result
                .source
                .contains(&format!("\n<Vertex data=\"{value}\" />\n"))
        );
        assert!(
            result
                .source
                .contains(&format!("\n<Face data=\"{value}\" />\n"))
        );
    }

    #[test]
    fn invalid_input_has_locations_and_never_returns_partial_output() {
        for input in [
            "<A>",
            "<A></B>",
            "<A v=\"x>",
            "<A v={1>",
            "<!--",
            "<A id />",
            "<A/><",
            "</A>",
            "<A x=\"1\"y=\"2\"/>",
        ] {
            assert!(format_dsl(input).is_err(), "{input}");
        }
        let err = format_dsl("<A>\n  </B>").unwrap_err();
        assert_eq!((err.line, err.column), (2, 3));
    }

    #[test]
    fn edit_offsets_support_unicode_browser_selections() {
        let input = "<A value=\"🍎蘋果\"><B/></A>";
        let result = check(input);
        for edit in &result.edits {
            assert_eq!(
                input[..edit.start_byte].encode_utf16().count(),
                edit.start_utf16
            );
            assert_eq!(
                input[..edit.end_byte].encode_utf16().count(),
                edit.end_utf16
            );
        }
    }

    #[test]
    fn semantic_parse_is_equal_except_source_and_diagnostic_locations() {
        let input = r##"<Graph fps={24} duration="1s" size={[1080,1920]}>
<Background color="#000000"/>
<Scene id="s">
<Timeline>
<Track id="t">
<Sequence from="0s" duration="1s">
<Layer>
<Rect id="r" x={1.00} y="2" width="30" height="40" color="#ff0000"/>
</Layer>
</Sequence>
</Track>
</Timeline>
</Scene>
<AnimationTarget node="r" property="x">
<Key time="0s" value="1"/><Key time="1s" value="2"/>
</AnimationTarget>
<Present from="s"/>
</Graph>"##;
        let output = check(input).source;
        let mut before = crate::parse_graph_script(input).unwrap();
        let mut after = crate::parse_graph_script(&output).unwrap();
        assert_eq!(before.animation_targets[0].keys.len(), 2);
        assert_eq!(after.animation_targets[0].keys.len(), 2);
        before.raw_script = None;
        after.raw_script = None;
        assert_eq!(
            serde_json::to_value(before).unwrap(),
            serde_json::to_value(after).unwrap()
        );
        let compact = input.lines().map(str::trim).collect::<String>();
        let mut compact_graph = crate::parse_graph_script(&compact).unwrap();
        let mut formatted_graph = crate::parse_graph_script(&output).unwrap();
        compact_graph.raw_script = None;
        formatted_graph.raw_script = None;
        assert_eq!(
            serde_json::to_value(compact_graph).unwrap(),
            serde_json::to_value(formatted_graph).unwrap()
        );
    }

    #[test]
    fn line_comments_and_processing_instructions_keep_contents() {
        let output =
            check("<?xml version=\"1.0\"?>\r\n// comment <Fake/>\n<A>\n // child\n<B/>\n</A>")
                .source;
        assert!(output.contains("// comment <Fake/>"));
        assert!(output.contains("\n  // child\n  <B />"));
    }

    #[test]
    fn parser_diagnostics_map_to_original_compact_source_lines() {
        let source = "<Graph fps={24} duration=\"1s\" size={[64,64]}>\n<AnimationTarget node=\"r\" property=\"notAProperty\"><Key time=\"0s\" value=\"1\"/></AnimationTarget><Present from=\"s\"/></Graph>";
        let error = crate::parse_graph_script(source).unwrap_err();
        assert_eq!(error.line, 2);
    }
}
