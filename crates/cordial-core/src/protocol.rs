//! Parsing the model's own output.
//!
//! LFM3 answers inside ChatML and, when it wants a tool, emits the `qwen3_coder`
//! style block that its chat template documents:
//!
//! ```text
//! <tool_call>
//! <function=web_search>
//! <parameter=query>
//! Sydney weather today
//! </parameter>
//! </function>
//! </tool_call>
//! ```
//!
//! Reasoning arrives inside `<think>...</think>` because the prompt already
//! opened that block. Parsing is deliberately forgiving: a 2B model occasionally
//! drops a closing tag, and a dropped tag must not stall the conversation.

use crate::session::{ToolArgument, ToolCall};

pub const THINK_OPEN: &str = "<think>";
pub const THINK_CLOSE: &str = "</think>";
pub const TOOL_CALL_OPEN: &str = "<tool_call>";
pub const TOOL_CALL_CLOSE: &str = "</tool_call>";

/// A tool call as written by the model, before it is given an id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCall {
    pub name: String,
    pub arguments: Vec<ToolArgument>,
}

impl ParsedCall {
    pub fn into_call(self, id: impl Into<String>) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: self.name,
            arguments: self.arguments,
        }
    }
}

/// Everything the harness needs out of one model reply.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedReply {
    pub thinking: Option<String>,
    /// Answer text with thinking and tool-call blocks removed.
    pub content: String,
    pub calls: Vec<ParsedCall>,
    /// Set when a `<tool_call>` block could not be understood.
    pub malformed: Option<String>,
    /// True when the reply ended while still inside a `<think>` block.
    pub truncated_thinking: bool,
}

impl ParsedReply {
    pub fn has_tool_calls(&self) -> bool {
        !self.calls.is_empty()
    }
}

/// Parses a model reply.
///
/// `thinking_enabled` mirrors how the prompt was rendered: when thinking is on,
/// generation starts inside `<think>`, so text before `</think>` is reasoning by
/// construction. When it is off, the whole reply is answer text.
pub fn parse_reply(raw: &str, thinking_enabled: bool) -> ParsedReply {
    // Nothing generated at all is not "reasoning ran long"; it is an empty
    // answer, and the caller should say so rather than retry blindly.
    if raw.trim().is_empty() {
        return ParsedReply::default();
    }

    let mut parsed = ParsedReply::default();

    let body = if thinking_enabled {
        match split_thinking(raw) {
            (thinking, rest, truncated) => {
                parsed.thinking = thinking;
                parsed.truncated_thinking = truncated;
                rest
            }
        }
    } else {
        raw.to_string()
    };

    let (content, calls, malformed) = extract_calls(&body);
    parsed.content = content;
    parsed.calls = calls;
    parsed.malformed = malformed;
    parsed
}

fn split_thinking(raw: &str) -> (Option<String>, String, bool) {
    let without_open = raw.strip_prefix(THINK_OPEN).unwrap_or(raw);
    if let Some(index) = without_open.find(THINK_CLOSE) {
        let thinking = without_open[..index].trim();
        let rest = &without_open[index + THINK_CLOSE.len()..];
        let thinking = if thinking.is_empty() {
            None
        } else {
            Some(thinking.to_string())
        };
        (
            thinking,
            rest.trim_start_matches(['\n', ' ']).to_string(),
            false,
        )
    } else {
        // Generation was cut off before the model closed its reasoning, which
        // means no answer was produced yet.
        let thinking = without_open.trim();
        (
            if thinking.is_empty() {
                None
            } else {
                Some(thinking.to_string())
            },
            String::new(),
            true,
        )
    }
}

fn extract_calls(body: &str) -> (String, Vec<ParsedCall>, Option<String>) {
    let mut content = String::new();
    let mut calls = Vec::new();
    let mut malformed: Option<String> = None;
    let mut rest = body;

    while let Some(start) = rest.find(TOOL_CALL_OPEN) {
        content.push_str(&rest[..start]);
        let after = &rest[start + TOOL_CALL_OPEN.len()..];
        let (block, remainder) = match after.find(TOOL_CALL_CLOSE) {
            Some(end) => (&after[..end], &after[end + TOOL_CALL_CLOSE.len()..]),
            // Unterminated block: parse what is there and stop scanning.
            None => (after, ""),
        };
        match parse_block(block) {
            Ok(call) => calls.push(call),
            Err(reason) => {
                malformed.get_or_insert(reason);
            }
        }
        if remainder.is_empty() {
            rest = "";
            break;
        }
        rest = remainder;
    }

    content.push_str(rest);
    (content.trim().to_string(), calls, malformed)
}

fn parse_block(block: &str) -> Result<ParsedCall, String> {
    let trimmed = block.trim();
    if trimmed.starts_with('{') {
        let value: serde_json::Value = serde_json::from_str(trimmed)
            .map_err(|_| "invalid JSON tool call".to_string())?;
        let name = value.get("name").and_then(|v| v.as_str())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| "JSON tool call has no function name".to_string())?;
        let arguments = value.get("arguments")
            .and_then(|v| v.as_object())
            .ok_or_else(|| "JSON tool call has no arguments object".to_string())?;
        return Ok(ParsedCall {
            name: name.to_string(),
            arguments: arguments.iter().map(|(key, value)| ToolArgument {
                name: key.clone(),
                value: value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string()),
            }).collect(),
        });
    }
    const FN: &str = "<function=";
    let start = block
        .find(FN)
        .ok_or_else(|| "tool call without a <function=...> tag".to_string())?;
    let after = &block[start + FN.len()..];
    let name_end = after
        .find('>')
        .ok_or_else(|| "tool call with an unterminated <function=> tag".to_string())?;
    let name = unquote(&after[..name_end]);
    if name.is_empty() {
        return Err("tool call with an empty function name".to_string());
    }

    let mut rest = &after[name_end + 1..];
    let mut arguments = Vec::new();
    const PARAM: &str = "<parameter=";
    const PARAM_END: &str = "</parameter>";

    while let Some(position) = rest.find(PARAM) {
        let after_param = &rest[position + PARAM.len()..];
        let Some(key_end) = after_param.find('>') else {
            break;
        };
        let key = unquote(&after_param[..key_end]);
        let value_part = &after_param[key_end + 1..];
        let (raw_value, remainder) = match value_part.find(PARAM_END) {
            Some(end) => (&value_part[..end], &value_part[end + PARAM_END.len()..]),
            None => (value_part, ""),
        };
        let value = raw_value
            .strip_prefix('\n')
            .unwrap_or(raw_value)
            .trim_end()
            .to_string();
        if !key.is_empty() {
            arguments.push(ToolArgument { name: key, value });
        }
        if remainder.is_empty() {
            break;
        }
        rest = remainder;
    }

    Ok(ParsedCall { name, arguments })
}

fn unquote(text: &str) -> String {
    text.trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_answer_has_no_calls() {
        let parsed = parse_reply(
            "Reasoning about it...\n</think>\nCanberra is the capital of Australia.",
            true,
        );
        assert_eq!(parsed.thinking.as_deref(), Some("Reasoning about it..."));
        assert_eq!(parsed.content, "Canberra is the capital of Australia.");
        assert!(parsed.calls.is_empty());
        assert!(!parsed.truncated_thinking);
    }

    #[test]
    fn parses_single_tool_call() {
        let raw = "I should search.\n</think>\n<tool_call>\n<function=web_search>\n<parameter=query>\nSydney weather today\n</parameter>\n</function>\n</tool_call>";
        let parsed = parse_reply(raw, true);
        assert_eq!(parsed.calls.len(), 1);
        assert_eq!(parsed.calls[0].name, "web_search");
        assert_eq!(parsed.calls[0].arguments[0].name, "query");
        assert_eq!(parsed.calls[0].arguments[0].value, "Sydney weather today");
        assert_eq!(parsed.content, "");
    }

    #[test]
    fn parses_multiple_calls_and_keeps_text() {
        let raw = "checking\n</think>\nLet me look.\n<tool_call>\n<function=read_file>\n<parameter=path>\na.txt\n</parameter>\n</function>\n</tool_call>\n<tool_call>\n<function=list_files>\n<parameter=path>\n.\n</parameter>\n</function>\n</tool_call>";
        let parsed = parse_reply(raw, true);
        assert_eq!(parsed.calls.len(), 2);
        assert_eq!(parsed.calls[1].name, "list_files");
        assert_eq!(parsed.content, "Let me look.");
    }

    #[test]
    fn tolerates_missing_closing_tag() {
        let raw = "sure\n</think>\n<tool_call>\n<function=browser_open>\n<parameter=url>\nhttps://example.com\n</parameter>\n</function>";
        let parsed = parse_reply(raw, true);
        assert_eq!(parsed.calls.len(), 1);
        assert_eq!(parsed.calls[0].arguments[0].value, "https://example.com");
    }

    #[test]
    fn tolerates_quoted_names() {
        let raw = "go\n</think>\n<tool_call>\n<function=\"web_fetch\">\n<parameter=\"url\">\nhttps://a.example\n</parameter>\n</function>\n</tool_call>";
        let parsed = parse_reply(raw, true);
        assert_eq!(parsed.calls[0].name, "web_fetch");
        assert_eq!(parsed.calls[0].arguments[0].name, "url");
    }

    #[test]
    fn parses_explicit_json_tool_call() {
        let parsed = parse_reply(r#"<tool_call>{"name":"web_search","arguments":{"query":"llama.cpp release notes"}}</tool_call>"#, false);
        assert_eq!(parsed.calls.len(), 1);
        assert_eq!(parsed.calls[0].name, "web_search");
        assert_eq!(parsed.calls[0].arguments[0].value, "llama.cpp release notes");
    }

    #[test]
    fn json_arguments_without_a_tool_name_are_not_executed() {
        let parsed = parse_reply(r#"<tool_call>{"query":"llama.cpp latest release notes","top_n":10}</tool_call>"#, false);
        assert!(parsed.calls.is_empty());
        assert!(parsed.malformed.is_some());
    }

    #[test]
    fn reports_malformed_block() {
        let raw =
            "go\n</think>\n<tool_call>\n<parameter=query>\nbroken\n</parameter>\n</tool_call>";
        let parsed = parse_reply(raw, true);
        assert!(parsed.calls.is_empty());
        assert!(parsed.malformed.is_some());
    }

    #[test]
    fn unterminated_thinking_is_flagged() {
        let parsed = parse_reply("still thinking about the question", true);
        assert!(parsed.truncated_thinking);
        assert_eq!(parsed.content, "");
        assert_eq!(
            parsed.thinking.as_deref(),
            Some("still thinking about the question")
        );
    }

    #[test]
    fn thinking_disabled_keeps_everything_as_answer() {
        let raw = "Paris is the capital of France.";
        let parsed = parse_reply(raw, false);
        assert_eq!(parsed.content, raw);
        assert!(parsed.thinking.is_none());
    }

    #[test]
    fn empty_reply_is_harmless() {
        let parsed = parse_reply("", true);
        assert_eq!(parsed, ParsedReply::default());
    }

    #[test]
    fn multiline_values_are_preserved() {
        let raw = "ok\n</think>\n<tool_call>\n<function=write_file>\n<parameter=content>\nline one\nline two\n</parameter>\n<parameter=path>\nnotes.txt\n</parameter>\n</function>\n</tool_call>";
        let parsed = parse_reply(raw, true);
        assert_eq!(parsed.calls[0].arguments[0].value, "line one\nline two");
        assert_eq!(parsed.calls[0].arguments[1].value, "notes.txt");
    }
}
