//! Prompt assembly for LFM3-VL-2B.
//!
//! LFM3 uses ChatML with a few extra rules, taken from the checkpoint's own
//! `chat_template.jinja`:
//!
//! * the prompt opens with `<|startoftext|>`,
//! * tool definitions live inside the system turn as `<tools>[…]</tools>`,
//! * an assistant turn keeps its reasoning when it belongs to the current user
//!   turn, and loses it (but keeps the tags) once a newer user message arrives,
//! * tool output is wrapped in `<tool_response>…</tool_response>`,
//! * generation starts with `<|im_start|>assistant\n<think>`.
//!
//! Building these strings here, rather than through the C API's template
//! helper, means the harness can be tested without loading a model.

use std::path::PathBuf;

use crate::session::{AttachmentKind, Message, Role};
use crate::tools::ToolSpec;
use crate::util::truncate_chars;

pub const BOS: &str = "<|startoftext|>";
pub const IM_START: &str = "<|im_start|>";
pub const IM_END: &str = "<|im_end|>";
pub const THINK_OPEN: &str = "<think>";
pub const THINK_CLOSE: &str = "</think>";
pub const TOOL_RESPONSE_OPEN: &str = "<tool_response>";
pub const TOOL_RESPONSE_CLOSE: &str = "</tool_response>";

/// What `mtmd` replaces with real image tokens.
pub const DEFAULT_MEDIA_MARKER: &str = "<__media__>";

/// A rendered prompt plus the images it references, in marker order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderedPrompt {
    pub text: String,
    pub media: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct PromptOptions<'a> {
    pub system: &'a str,
    pub tools: &'a [ToolSpec],
    pub messages: &'a [Message],
    pub media_marker: &'a str,
    pub thinking: bool,
    /// Tool output longer than this is clipped before it reaches the model.
    pub max_tool_result_chars: usize,
    /// Images are kept for this many most-recent user turns. Older screenshots
    /// stop being re-encoded, which is what keeps a long session inside the
    /// context window.
    pub keep_image_turns: usize,
}

/// Renders a full prompt, ending with the assistant generation prefix.
pub fn render(options: &PromptOptions<'_>) -> RenderedPrompt {
    let mut out = String::with_capacity(4_096);
    let mut media = Vec::new();

    out.push_str(BOS);

    let system = system_block(options.system, options.tools);
    if !system.is_empty() {
        out.push_str(IM_START);
        out.push_str("system\n");
        out.push_str(&system);
        out.push_str(IM_END);
        out.push('\n');
    }

    let last_user = options
        .messages
        .iter()
        .rposition(|message| message.role == Role::User);
    let image_cutoff = image_cutoff(options.messages, options.keep_image_turns);

    for (index, message) in options.messages.iter().enumerate() {
        let with_images = index >= image_cutoff;
        match message.role {
            Role::User => push_user(
                &mut out,
                message,
                options.media_marker,
                &mut media,
                with_images,
            ),
            Role::Assistant => {
                let keep_thinking = last_user.map(|last| index > last).unwrap_or(false);
                push_assistant(&mut out, message, keep_thinking);
            }
            Role::Tool => push_tool(
                &mut out,
                message,
                options.media_marker,
                &mut media,
                options.max_tool_result_chars,
                with_images,
            ),
        }
    }

    out.push_str(IM_START);
    out.push_str("assistant\n");
    if options.thinking {
        out.push_str(THINK_OPEN);
    } else {
        out.push_str(THINK_OPEN);
        out.push_str(THINK_CLOSE);
    }

    RenderedPrompt { text: out, media }
}

fn system_block(system: &str, tools: &[ToolSpec]) -> String {
    let mut block = system.trim().to_string();
    if tools.is_empty() {
        return block;
    }
    let mut tools_block = String::from("<tools>\n");
    for (index, tool) in tools.iter().enumerate() {
        if index > 0 {
            tools_block.push('\n');
        }
        tools_block.push_str(&tool.to_prompt_json());
    }
    tools_block.push_str("\n</tools>");
    if !block.is_empty() {
        block.push_str("\n\n");
    }
    block.push_str(&tools_block);
    block
}

/// First message index whose images stay in the prompt.
fn image_cutoff(messages: &[Message], keep_turns: usize) -> usize {
    if keep_turns == 0 {
        return messages.len();
    }
    let user_positions: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == Role::User)
        .map(|(index, _)| index)
        .collect();
    if user_positions.len() <= keep_turns {
        return 0;
    }
    user_positions[user_positions.len() - keep_turns]
}

fn push_user(
    out: &mut String,
    message: &Message,
    marker: &str,
    media: &mut Vec<PathBuf>,
    with_images: bool,
) {
    out.push_str(IM_START);
    out.push_str("user\n");
    for attachment in &message.attachments {
        if with_images && attachment.kind == AttachmentKind::Image {
            out.push_str(marker);
            out.push('\n');
            media.push(PathBuf::from(&attachment.path));
        }
    }
    out.push_str(&message.content);
    out.push_str(IM_END);
    out.push('\n');
}

fn push_assistant(out: &mut String, message: &Message, keep_thinking: bool) {
    out.push_str(IM_START);
    out.push_str("assistant\n");
    out.push_str(&assistant_body(message, keep_thinking));
    out.push_str(IM_END);
    out.push('\n');
}

/// Mirrors the template's assistant branch, including its thinking rules.
pub fn assistant_body(message: &Message, keep_thinking: bool) -> String {
    let mut out = String::new();
    let content = message.content.trim();

    if keep_thinking {
        match message.thinking.as_deref().map(str::trim) {
            Some(thinking) if !thinking.is_empty() => {
                out.push_str(THINK_OPEN);
                out.push_str(thinking);
                out.push_str(THINK_CLOSE);
                out.push_str(content);
            }
            _ if !content.contains(THINK_CLOSE) => {
                // Nothing to keep and no tags present: mirror the template and
                // emit an empty reasoning block.
                out.push_str(THINK_OPEN);
                out.push_str(THINK_CLOSE);
                out.push_str(content);
            }
            _ => out.push_str(content),
        }
    } else {
        let answer = match content.rfind(THINK_CLOSE) {
            Some(index) => content[index + THINK_CLOSE.len()..].trim(),
            None => content,
        };
        out.push_str(THINK_OPEN);
        out.push_str(THINK_CLOSE);
        out.push_str(answer);
    }

    if !message.tool_calls.is_empty() {
        out.push_str(&render_tool_calls(&message.tool_calls));
    }
    out
}

/// Renders tool calls exactly as LFM3 writes them.
pub fn render_tool_calls(calls: &[crate::session::ToolCall]) -> String {
    let mut out = String::new();
    for (index, call) in calls.iter().enumerate() {
        if index > 0 {
            out.push_str("\n\n");
        }
        out.push_str("<tool_call>\n");
        out.push_str("<function=");
        out.push_str(&call.name);
        out.push_str(">\n");
        for argument in &call.arguments {
            out.push_str("<parameter=");
            out.push_str(&argument.name);
            out.push_str(">\n");
            out.push_str(&argument.value);
            out.push_str("\n</parameter>\n");
        }
        out.push_str("</function>\n</tool_call>");
    }
    out
}

fn push_tool(
    out: &mut String,
    message: &Message,
    marker: &str,
    media: &mut Vec<PathBuf>,
    max_chars: usize,
    with_images: bool,
) {
    out.push_str(IM_START);
    out.push_str("tool\n");
    out.push_str(TOOL_RESPONSE_OPEN);
    out.push('\n');
    for attachment in &message.attachments {
        if with_images && attachment.kind == AttachmentKind::Image {
            out.push_str(marker);
            out.push('\n');
            media.push(PathBuf::from(&attachment.path));
        }
    }
    out.push_str(&truncate_chars(&message.content, max_chars));
    out.push('\n');
    out.push_str(TOOL_RESPONSE_CLOSE);
    out.push_str(IM_END);
    out.push('\n');
}

/// The behavioural core of Cordial. Written for a 2B model: short sentences,
/// explicit ordering, and a hard bias towards checking facts before answering.
pub fn system_prompt(
    effort: crate::settings::Effort,
    permission_mode: crate::settings::PermissionMode,
    web_enabled: bool,
) -> String {
    let today = crate::util::iso_date_now();
    let mut prompt = String::new();

    prompt.push_str(
        "You are Cordial, a local assistant running on this device with LFM3-VL-2B. \
         You help with questions, research, browsing and the user's files.\n\n",
    );

    prompt.push_str("Answering\n");
    if effort == crate::settings::Effort::Task {
        prompt.push_str("- Address every part of the task. Show the requested reasoning and verification; use as much space as needed. Avoid filler.\n");
    } else {
        prompt.push_str("- Lead with the answer, then at most three short paragraphs of support. No preamble, no restating the question, no filler.\n");
    }
    prompt.push_str("- Use plain words and short sentences. Do not use em dashes. No emoji, no bold headers, no bullet lists unless you are listing things.\n");
    prompt.push_str(
        "- If something is uncertain, say so in one sentence and say how you would check it.\n\n",
    );

    prompt.push_str("Facts\n");
    prompt.push_str(&format!("- Today is {today}. Anything that changes (news, prices, sport, releases, people, places) needs a web check first.\n"));
    if web_enabled {
        prompt.push_str("- Search, open the best result, then answer. Put the source URL in brackets after the sentence it supports, at most three links.\n");
    } else {
        prompt.push_str("- The web is disabled. Do not pretend you checked online; identify facts you could not verify.\n");
    }
    prompt.push_str("- Never invent URLs, quotes, dates, prices or numbers. If a tool fails, say the web is unavailable and mark what you say as unchecked.\n\n");

    prompt.push_str("Tools\n");
    if web_enabled {
        prompt.push_str("- Call one tool at a time in a <tool_call> block, then wait for the result before writing further.\n");
        prompt.push_str("- Prefer web_fetch when you already have a link. Use browser_open when a page needs JavaScript or when seeing the screen helps, then browser_click by label, browser_type and browser_view.\n");
    } else {
        prompt.push_str("- Web tools are switched off. Say so if a question needs the internet.\n");
    }
    prompt.push_str("- Attached text and images are already included in the message. Use them directly; do not search the workspace for an attachment. PDF page text can omit diagram labels: inspect the page image and do not invent unreadable measurements. For other workspace files, use list_files or read_file before asking for a path.\n");
    prompt.push_str("- run_command executes one read-only workspace command: grep, find, ls, cat, head, tail, wc, stat or pwd. It is not a shell; destructive commands, pipes and redirects are refused.\n");
    prompt.push_str("- For Word worksheets, use docx_read to get numbered paragraphs, research each part, then docx_edit to write answers into a copy. Verify the edited file with docx_read.\n");
    if effort == crate::settings::Effort::Task {
        prompt.push_str("- Use update_plan to maintain the task checklist. As work progresses, include every step and mark each done only after verification. Report blocked work honestly.\n");
    }
    prompt.push_str("- When you have enough information, answer in plain text with no tool call. Stop once the question is answered.\n\n");

    prompt.push_str("Style\n");
    prompt.push_str(
        "- Be warm and direct, like a capable colleague. Answer the question, not the person.\n",
    );
    prompt.push_str("- Ask a question back only when the task cannot proceed without it.\n");
    prompt.push_str(
        "- Do not close with offers of further help (\"Would you like me to help with anything \
         else?\"). Finish on the last useful sentence.\n",
    );
    prompt.push_str("- If the user attaches an image, say what you actually see in it before answering the question.\n\n");

    prompt.push_str(effort.guidance());
    prompt.push('\n');
    match permission_mode {
        crate::settings::PermissionMode::Auto => {
            prompt.push_str("Writes to files may ask the user for approval before they run.\n")
        }
        crate::settings::PermissionMode::Manual => {
            prompt.push_str("Every tool call is shown to the user for approval before it runs.\n")
        }
    }
    prompt.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::session::{Attachment, Message, ToolArgument, ToolCall};
    use crate::tools;

    fn options<'a>(
        system: &'a str,
        tools: &'a [ToolSpec],
        messages: &'a [Message],
    ) -> PromptOptions<'a> {
        PromptOptions {
            system,
            tools,
            messages,
            media_marker: DEFAULT_MEDIA_MARKER,
            thinking: true,
            max_tool_result_chars: 4_000,
            keep_image_turns: 2,
        }
    }

    #[test]
    fn minimal_prompt_matches_the_template() {
        let messages = vec![Message::user("Hello", vec![])];
        let rendered = render(&options("Sys.", &[], &messages));
        assert_eq!(
            rendered.text,
            format!(
                "{BOS}{IM_START}system\nSys.{IM_END}\n{IM_START}user\nHello{IM_END}\n{IM_START}assistant\n{THINK_OPEN}"
            )
        );
        assert!(rendered.media.is_empty());
    }

    #[test]
    fn tools_are_embedded_in_the_system_turn() {
        let specs = tools::specs(false);
        let messages = vec![Message::user("hi", vec![])];
        let rendered = render(&options("Sys.", &specs, &messages));
        assert!(rendered.text.contains("<tools>\n"));
        assert!(rendered.text.contains("</tools>"));
        assert!(rendered.text.contains("\"name\": \"read_file\""));
        assert!(rendered.text.contains("Sys.\n\n<tools>"));
    }

    #[test]
    fn images_become_markers_in_order() {
        let attachments = vec![
            Attachment {
                kind: AttachmentKind::Image,
                name: "a.png".into(),
                path: "/tmp/a.png".into(),
            },
            Attachment {
                kind: AttachmentKind::Text,
                name: "notes.txt".into(),
                path: "/tmp/notes.txt".into(),
            },
            Attachment {
                kind: AttachmentKind::Image,
                name: "b.png".into(),
                path: "/tmp/b.png".into(),
            },
        ];
        let messages = vec![Message::user("compare", attachments)];
        let rendered = render(&options("Sys.", &[], &messages));
        assert_eq!(rendered.media.len(), 2);
        assert_eq!(rendered.media[0], PathBuf::from("/tmp/a.png"));
        assert_eq!(rendered.media[1], PathBuf::from("/tmp/b.png"));
        let first = rendered.text.find(DEFAULT_MEDIA_MARKER).unwrap();
        let second = rendered.text[first + 1..]
            .find(DEFAULT_MEDIA_MARKER)
            .map(|offset| offset + first + 1)
            .unwrap();
        assert!(first < second);
        assert!(rendered
            .text
            .contains("user\n<__media__>\n<__media__>\ncompare"));
    }

    #[test]
    fn old_screenshots_drop_out_of_the_prompt() {
        let image = |path: &str| Attachment {
            kind: AttachmentKind::Image,
            name: "shot.png".into(),
            path: path.into(),
        };
        let messages = vec![
            Message::user("first", vec![image("/tmp/one.png")]),
            Message::assistant("done", None, vec![]),
            Message::user("second", vec![image("/tmp/two.png")]),
            Message::assistant("done again", None, vec![]),
            Message::user("third", vec![image("/tmp/three.png")]),
        ];
        let rendered = render(&options("Sys.", &[], &messages));
        // keep_image_turns = 2 keeps the newest two user turns.
        assert_eq!(rendered.media.len(), 2);
        assert!(rendered
            .media
            .iter()
            .all(|p| p != Path::new("/tmp/one.png")));
    }

    #[test]
    fn current_turn_keeps_its_thinking() {
        let mut assistant =
            Message::assistant("The answer is 42.", Some("I counted.".into()), vec![]);
        assistant.id = "a".into();
        let messages = vec![Message::user("q", vec![]), assistant];
        let rendered = render(&options("Sys.", &[], &messages));
        assert!(rendered
            .text
            .contains("assistant\n<think>I counted.</think>The answer is 42."));
    }

    #[test]
    fn older_turns_drop_their_thinking_but_keep_the_tags() {
        let mut assistant = Message::assistant("<think>secret plan</think>\nParis.", None, vec![]);
        assistant.id = "a".into();
        let messages = vec![
            Message::user("first", vec![]),
            assistant,
            Message::user("second", vec![]),
        ];
        let rendered = render(&options("Sys.", &[], &messages));
        assert!(rendered.text.contains("assistant\n<think></think>Paris."));
        assert!(!rendered.text.contains("secret plan"));
    }

    #[test]
    fn tool_calls_and_results_round_trip() {
        let call = ToolCall {
            id: "c1".into(),
            name: "web_search".into(),
            arguments: vec![ToolArgument {
                name: "query".into(),
                value: "capital of australia".into(),
            }],
        };
        let assistant = Message::assistant("", Some("Need a check.".into()), vec![call]);
        let tool = Message::tool_result("c1", "1. Canberra — https://example.com/a");
        let messages = vec![Message::user("capital?", vec![]), assistant, tool];
        let rendered = render(&options("Sys.", &[], &messages));

        let expected_call = concat!(
            "<tool_call>\n",
            "<function=web_search>\n",
            "<parameter=query>\n",
            "capital of australia\n",
            "</parameter>\n",
            "</function>\n",
            "</tool_call>"
        );
        assert!(rendered.text.contains(expected_call));
        assert!(rendered.text.contains(
            "tool\n<tool_response>\n1. Canberra — https://example.com/a\n</tool_response><|im_end|>"
        ));
    }

    #[test]
    fn tool_output_is_clipped() {
        let tool = Message::tool_result("c1", "x".repeat(500));
        let messages = vec![Message::user("read", vec![]), tool];
        let mut opts = options("Sys.", &[], &messages);
        opts.max_tool_result_chars = 100;
        let rendered = render(&opts);
        assert!(rendered.text.len() < 600);
        assert!(rendered.text.contains("[cut: content was longer]"));
    }

    #[test]
    fn thinking_can_be_disabled() {
        let messages = vec![Message::user("hi", vec![])];
        let mut opts = options("Sys.", &[], &messages);
        opts.thinking = false;
        let rendered = render(&opts);
        assert!(rendered
            .text
            .ends_with("<|im_start|>assistant\n<think></think>"));
    }

    #[test]
    fn system_prompt_mentions_date_and_effort() {
        let prompt = system_prompt(
            crate::settings::Effort::Medium,
            crate::settings::PermissionMode::Auto,
            true,
        );
        assert!(prompt.contains("Do not use em dashes"));
        assert!(prompt.contains("Today is"));
        assert!(prompt.contains("Effort: medium"));
        assert!(prompt.contains("web_search") || prompt.contains("Search"));
        assert!(!prompt.contains("None"));
    }

    #[test]
    fn task_prompt_allows_full_work_and_uses_attachments_directly() {
        let prompt = system_prompt(
            crate::settings::Effort::Task,
            crate::settings::PermissionMode::Auto,
            false,
        );
        assert!(prompt.contains("Address every part"));
        assert!(!prompt.contains("at most three short paragraphs"));
        assert!(prompt.contains("do not search the workspace for an attachment"));
        assert!(prompt.contains("do not invent unreadable measurements"));
        assert!(prompt.contains("Do not pretend you checked online"));
        assert!(!prompt.contains("Search, open the best result"));
    }

    #[test]
    fn system_prompt_respects_web_switch() {
        let prompt = system_prompt(
            crate::settings::Effort::Low,
            crate::settings::PermissionMode::Manual,
            false,
        );
        assert!(prompt.contains("Web tools are switched off"));
        assert!(prompt.contains("Every tool call is shown"));
    }
}
