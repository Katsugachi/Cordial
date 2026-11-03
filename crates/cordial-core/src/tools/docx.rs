//! DOCX reading and editing without a full office suite.
//!
//! The tools are intentionally small: `docx_read` extracts numbered paragraphs,
//! `docx_edit` replaces the text of selected paragraphs while preserving the
//! rest of the package, and `docx_create` writes a simple new document. Archive
//! size, entry count and XML size are bounded before parsing.

use std::io::{Cursor, Read, Write};
use std::path::Path;

use quick_xml::escape::{escape, unescape};
use quick_xml::events::{BytesText, Event};
use quick_xml::reader::Reader;
use quick_xml::writer::Writer;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::session::ToolCall;
use crate::tools::{files, ToolOutcome, ToolRuntime};
use crate::util::truncate_chars;

const MAX_DOCX_BYTES: u64 = 25_000_000;
const MAX_XML_BYTES: u64 = 10_000_000;
const MAX_ENTRIES: usize = 2_000;
const MAX_PARAGRAPHS: usize = 20_000;
const MAX_TEXT_CHARS: usize = 200_000;
const MAX_TOTAL_BYTES: u64 = 200_000_000;

pub fn read(runtime: &ToolRuntime, call: &ToolCall) -> ToolOutcome {
    let Some(raw) = call.arg("path") else {
        return ToolOutcome::fail("missing path", "Error: `path` is required.");
    };
    let path = match files::resolve(runtime.workspace(), raw) {
        Ok(path) => path,
        Err(err) => return ToolOutcome::fail("path refused", format!("Error: {err}")),
    };
    match read_docx(&path) {
        Ok(paragraphs) => {
            let start = call.int("start").unwrap_or(0);
            if start < 0 || start as usize > paragraphs.len() {
                return ToolOutcome::fail(
                    "invalid start",
                    "Error: start must be a valid paragraph index.",
                );
            }
            let count = call.int("count").unwrap_or(100).clamp(1, 500) as usize;
            let start = start as usize;
            let budget = runtime.effort().fetch_chars().saturating_sub(300).max(1000);
            let mut text = String::new();
            let mut next = start;
            for (index, paragraph) in paragraphs.iter().enumerate().skip(start).take(count) {
                let line = format!("[p{index}] {paragraph}\n");
                if text.chars().count() + line.chars().count() > budget {
                    if next == start {
                        return ToolOutcome::fail("paragraph too long", format!("Error: paragraph {index} exceeds the read limit; it was not silently truncated."));
                    }
                    break;
                }
                text.push_str(&line);
                next = index + 1;
            }
            text.push_str(&format!(
                "\nParagraphs {start}..{next} of {}.",
                paragraphs.len()
            ));
            if next < paragraphs.len() {
                text.push_str(&format!(" Continue with docx_read start={next}."));
            }
            ToolOutcome::ok(
                format!("read {} of {} paragraphs", next - start, paragraphs.len()),
                text,
            )
        }
        Err(err) => ToolOutcome::fail("docx read failed", format!("Error: {err}")),
    }
}

pub fn edit(runtime: &ToolRuntime, call: &ToolCall) -> ToolOutcome {
    let (Some(raw), Some(edits)) = (call.arg("path"), call.arg("edits")) else {
        return ToolOutcome::fail(
            "missing arguments",
            "Error: `path` and `edits` are required. `edits` is a JSON array like [{\"paragraph\":0,\"text\":\"Answer\"}].",
        );
    };
    let path = match files::resolve(runtime.workspace(), raw) {
        Ok(path) => path,
        Err(err) => return ToolOutcome::fail("path refused", format!("Error: {err}")),
    };
    let edits: Vec<Edit> = match serde_json::from_str(edits) {
        Ok(edits) => edits,
        Err(err) => return ToolOutcome::fail("invalid edits", format!("Error: {err}")),
    };
    if edits.is_empty() || edits.len() > 500 {
        return ToolOutcome::fail(
            "invalid edits",
            "Error: provide between 1 and 500 paragraph edits.",
        );
    }
    let output = match call.arg("output") {
        Some(raw) => match files::resolve(runtime.workspace(), raw) {
            Ok(path) => path,
            Err(err) => return ToolOutcome::fail("path refused", format!("Error: {err}")),
        },
        None => {
            let name = default_output(&path);
            let relative = match name.strip_prefix(
                runtime
                    .workspace()
                    .canonicalize()
                    .unwrap_or_else(|_| runtime.workspace().to_path_buf()),
            ) {
                Ok(relative) => relative.to_string_lossy().into_owned(),
                Err(_) => {
                    return ToolOutcome::fail(
                        "path refused",
                        "Error: output is outside the workspace.",
                    )
                }
            };
            match files::resolve(runtime.workspace(), &relative) {
                Ok(path) => path,
                Err(err) => return ToolOutcome::fail("path refused", format!("Error: {err}")),
            }
        }
    };
    if output
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| !ext.eq_ignore_ascii_case("docx"))
        .unwrap_or(true)
    {
        return ToolOutcome::fail(
            "invalid output",
            "Error: the output path must end in .docx.",
        );
    }
    match edit_docx(&path, &output, &edits) {
        Ok(count) => ToolOutcome::ok(
            format!("edited {count} paragraphs"),
            format!("Wrote {}.", output.display()),
        ),
        Err(err) => ToolOutcome::fail("docx edit failed", format!("Error: {err}")),
    }
}

pub fn create(runtime: &ToolRuntime, call: &ToolCall) -> ToolOutcome {
    let (Some(raw), Some(content)) = (call.arg("path"), call.arg("content")) else {
        return ToolOutcome::fail(
            "missing arguments",
            "Error: `path` and `content` are required.",
        );
    };
    if content.len() > 1_000_000 {
        return ToolOutcome::fail("content too large", "Error: content exceeds 1 MB.");
    }
    let path = match files::resolve(runtime.workspace(), raw) {
        Ok(path) => path,
        Err(err) => return ToolOutcome::fail("path refused", format!("Error: {err}")),
    };
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| !ext.eq_ignore_ascii_case("docx"))
        .unwrap_or(true)
    {
        return ToolOutcome::fail("invalid path", "Error: the output path must end in .docx.");
    }
    match create_docx(&path, content) {
        Ok(()) => ToolOutcome::ok("created docx", format!("Wrote {}.", path.display())),
        Err(err) => ToolOutcome::fail("docx create failed", format!("Error: {err}")),
    }
}

#[derive(Debug, serde::Deserialize)]
struct Edit {
    paragraph: usize,
    text: String,
}

fn default_output(path: &Path) -> std::path::PathBuf {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document");
    path.with_file_name(format!("{stem}.edited.docx"))
}

fn open_archive(path: &Path) -> Result<ZipArchive<Cursor<Vec<u8>>>, String> {
    let meta = std::fs::metadata(path).map_err(|err| err.to_string())?;
    if meta.len() > MAX_DOCX_BYTES {
        return Err("DOCX is larger than the 25 MB limit".to_string());
    }
    let bytes = std::fs::read(path).map_err(|err| err.to_string())?;
    let archive = ZipArchive::new(Cursor::new(bytes)).map_err(|err| err.to_string())?;
    if archive.len() > MAX_ENTRIES {
        return Err("DOCX has too many archive entries".to_string());
    }
    Ok(archive)
}

fn read_entry(archive: &mut ZipArchive<Cursor<Vec<u8>>>, name: &str) -> Result<Vec<u8>, String> {
    let mut file = archive.by_name(name).map_err(|err| err.to_string())?;
    if file.size() > MAX_XML_BYTES {
        return Err(format!("{name} is larger than the 10 MB XML limit"));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_XML_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| err.to_string())?;
    if bytes.len() as u64 > MAX_XML_BYTES {
        return Err(format!("{name} exceeded the XML read limit"));
    }
    Ok(bytes)
}

fn read_docx(path: &Path) -> Result<Vec<String>, String> {
    let mut archive = open_archive(path)?;
    let xml = read_entry(&mut archive, "word/document.xml")?;
    let xml = std::str::from_utf8(&xml).map_err(|err| err.to_string())?;
    parse_paragraphs(xml)
}

fn parse_paragraphs(xml: &str) -> Result<Vec<String>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut paragraphs = Vec::new();
    let mut current: Option<String> = None;
    let mut depth = 0usize;
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(start)) => match start.local_name().as_ref() {
                "p" => {
                    if current.is_some() {
                        return Err("nested paragraphs are not supported".to_string());
                    }
                    current = Some(String::new());
                    depth += 1;
                }
                "tab" => {
                    if let Some(text) = current.as_mut() {
                        text.push('\t');
                    }
                }
                "br" | "cr" => {
                    if let Some(text) = current.as_mut() {
                        text.push('\n');
                    }
                }
                _ => {}
            },
            Ok(Event::Empty(start)) => match start.local_name().as_ref() {
                "tab" => {
                    if let Some(text) = current.as_mut() {
                        text.push('\t');
                    }
                }
                "br" | "cr" => {
                    if let Some(text) = current.as_mut() {
                        text.push('\n');
                    }
                }
                _ => {}
            },
            Ok(Event::Text(text)) => {
                if let Some(current) = current.as_mut() {
                    let decoded = unescape(text.as_ref()).map_err(|err| err.to_string())?;
                    current.push_str(&decoded);
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                if let Some(current) = current.as_mut() {
                    let entity = format!("&{};", reference.as_ref());
                    let decoded = unescape(&entity).map_err(|err| err.to_string())?;
                    current.push_str(&decoded);
                }
            }
            Ok(Event::End(end)) => {
                if end.local_name().as_ref() == "p" {
                    if let Some(text) = current.take() {
                        paragraphs.push(text);
                        if paragraphs.len() > MAX_PARAGRAPHS {
                            return Err("DOCX has too many paragraphs".to_string());
                        }
                    }
                    depth = depth.saturating_sub(1);
                }
            }
            Ok(_) => {}
            Err(err) => return Err(format!("invalid DOCX XML: {err}")),
        }
    }
    if depth != 0 || current.is_some() {
        return Err("DOCX XML ended inside a paragraph".to_string());
    }
    Ok(paragraphs)
}

fn edit_docx(path: &Path, output: &Path, edits: &[Edit]) -> Result<usize, String> {
    let mut archive = open_archive(path)?;
    let xml = read_entry(&mut archive, "word/document.xml")?;
    let xml = std::str::from_utf8(&xml).map_err(|err| err.to_string())?;
    let (updated, count) = replace_paragraphs(xml, edits)?;
    if count != edits.len() {
        return Err("one or more paragraph indexes were not found".to_string());
    }
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let names: Vec<String> = archive
        .file_names()
        .map(|name| {
            name.map(|value| value.into_owned())
                .map_err(|err| err.to_string())
        })
        .collect::<Result<_, _>>()?;
    let mut total = updated.len() as u64;
    for name in &names {
        if name == "word/document.xml" {
            continue;
        }
        let entry = archive.by_name(name).map_err(|err| err.to_string())?;
        total = total.saturating_add(entry.size());
        if total > MAX_TOTAL_BYTES {
            return Err("DOCX expands beyond the 200 MB limit".to_string());
        }
    }
    // Build the whole archive before touching the output. Never overwrite an
    // existing document, including the source, even if approval was granted.
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for name in names {
        let mut entry = archive.by_name(&name).map_err(|err| err.to_string())?;
        if entry.is_dir() {
            writer
                .add_directory(name, options)
                .map_err(|err| err.to_string())?;
            continue;
        }
        writer
            .start_file(&name, options)
            .map_err(|err| err.to_string())?;
        if name == "word/document.xml" {
            writer
                .write_all(updated.as_bytes())
                .map_err(|err| err.to_string())?;
        } else {
            let mut bytes = Vec::new();
            entry
                .by_ref()
                .take(MAX_XML_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|err| err.to_string())?;
            if bytes.len() as u64 > MAX_XML_BYTES {
                return Err("DOCX entry exceeded the read limit".to_string());
            }
            writer.write_all(&bytes).map_err(|err| err.to_string())?;
        }
    }
    let bytes = writer.finish().map_err(|err| err.to_string())?.into_inner();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|err| err.to_string())?;
    file.write_all(&bytes).map_err(|err| err.to_string())?;
    Ok(count)
}

fn replace_paragraphs(xml: &str, edits: &[Edit]) -> Result<(String, usize), String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Cursor::new(Vec::new()));
    let mut paragraph: Option<usize> = None;
    let mut in_target = false;
    let mut in_text = false;
    let mut wrote_replacement = false;
    let mut current_edit: Option<&Edit> = None;
    let mut count = 0usize;
    loop {
        let event = reader.read_event().map_err(|err| err.to_string())?;
        match event {
            Event::Eof => break,
            Event::Start(start) => {
                let local = start.local_name();
                if local.as_ref() == "p" {
                    let index = paragraph.map(|value| value + 1).unwrap_or(0);
                    paragraph = Some(index);
                    current_edit = edits.iter().find(|edit| edit.paragraph == index);
                    in_target = current_edit.is_some();
                    wrote_replacement = false;
                } else if local.as_ref() == "t" && in_target {
                    in_text = true;
                }
                writer
                    .write_event(Event::Start(start.into_owned()))
                    .map_err(|err| err.to_string())?;
            }
            Event::End(end) => {
                let local = end.local_name();
                if local.as_ref() == "t" {
                    in_text = false;
                } else if local.as_ref() == "p" {
                    if in_target && !wrote_replacement {
                        return Err(format!(
                            "paragraph {} has no editable text run",
                            paragraph.unwrap_or(0)
                        ));
                    }
                    in_target = false;
                    current_edit = None;
                }
                writer
                    .write_event(Event::End(end.into_owned()))
                    .map_err(|err| err.to_string())?;
            }
            Event::Text(text) => {
                if in_text && in_target {
                    if let Some(edit) = current_edit {
                        if !wrote_replacement {
                            let escaped = escape(&edit.text);
                            writer
                                .write_event(Event::Text(BytesText::from_escaped(
                                    escaped.into_owned(),
                                )))
                                .map_err(|err| err.to_string())?;
                            wrote_replacement = true;
                            count += 1;
                        } else {
                            writer
                                .write_event(Event::Text(BytesText::from_escaped("")))
                                .map_err(|err| err.to_string())?;
                        }
                    }
                } else {
                    writer
                        .write_event(Event::Text(text.into_owned()))
                        .map_err(|err| err.to_string())?;
                }
            }
            Event::GeneralRef(_) if in_text && in_target => {}
            other => {
                writer
                    .write_event(other.into_owned())
                    .map_err(|err| err.to_string())?;
            }
        }
    }
    let bytes = writer.into_inner().into_inner();
    String::from_utf8(bytes)
        .map(|text| (text, count))
        .map_err(|err| err.to_string())
}

fn create_docx(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut document = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>"#,
    );
    for line in content.lines() {
        document.push_str("<w:p><w:r><w:t xml:space=\"preserve\">");
        document.push_str(&escape(line));
        document.push_str("</w:t></w:r></w:p>");
    }
    document.push_str(r#"<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"/></w:sectPr></w:body></w:document>"#);
    let entries: &[(&str, &str)] = &[
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#,
        ),
        (
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
        ),
        ("word/document.xml", &document),
    ];
    for (name, body) in entries {
        writer
            .start_file(*name, options)
            .map_err(|err| err.to_string())?;
        writer
            .write_all(body.as_bytes())
            .map_err(|err| err.to_string())?;
    }
    let bytes = writer.finish().map_err(|err| err.to_string())?.into_inner();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|err| err.to_string())?;
    file.write_all(&bytes).map_err(|err| err.to_string())?;
    Ok(())
}

/// Extracts a user-selected DOCX attachment for the prompt. Unlike workspace
/// tools, the path comes from the native file picker, not from the model.
pub(crate) fn attachment_text(path: &Path, max_chars: usize) -> Result<String, String> {
    let paragraphs = read_docx(path)?;
    let mut text = String::new();
    for (index, paragraph) in paragraphs.iter().enumerate() {
        text.push_str(&format!("[p{index}] {paragraph}\n"));
    }
    Ok(truncate_chars(&text, max_chars.min(MAX_TEXT_CHARS)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numbered_paragraphs() {
        let xml = r#"<w:document xmlns:w="x"><w:body><w:p><w:r><w:t>Hello</w:t></w:r></w:p><w:p><w:r><w:t>World &amp; more</w:t></w:r></w:p></w:body></w:document>"#;
        let paragraphs = parse_paragraphs(xml).unwrap();
        assert_eq!(paragraphs, vec!["Hello", "World & more"]);
    }

    #[test]
    fn replaces_only_target_paragraph() {
        let xml = r#"<w:document xmlns:w="x"><w:body><w:p><w:r><w:t>Question</w:t></w:r></w:p><w:p><w:r><w:t>Blank</w:t></w:r></w:p></w:body></w:document>"#;
        let edits = vec![Edit {
            paragraph: 1,
            text: "Answer <safe>".to_string(),
        }];
        let (updated, count) = replace_paragraphs(xml, &edits).unwrap();
        assert_eq!(count, 1);
        let paragraphs = parse_paragraphs(&updated).unwrap();
        assert_eq!(paragraphs, vec!["Question", "Answer <safe>"]);
    }

    #[test]
    fn replacing_text_removes_old_entity_references() {
        let xml = r#"<w:document xmlns:w="x"><w:body><w:p><w:r><w:t>Old &amp; text</w:t></w:r></w:p></w:body></w:document>"#;
        let edits = vec![Edit {
            paragraph: 0,
            text: "New".into(),
        }];
        let (updated, _) = replace_paragraphs(xml, &edits).unwrap();
        assert_eq!(parse_paragraphs(&updated).unwrap(), vec!["New"]);
    }

    #[test]
    fn create_does_not_overwrite_existing_document() {
        let dir =
            std::env::temp_dir().join(format!("cordial-docx-preserve-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.docx");
        std::fs::write(&path, b"original").unwrap();
        assert!(create_docx(&path, "replacement").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn edit_never_overwrites_source_or_existing_output() {
        let dir = std::env::temp_dir().join(format!("cordial-docx-edit-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.docx");
        create_docx(&source, "Original").unwrap();
        let original = std::fs::read(&source).unwrap();
        let edits = vec![Edit {
            paragraph: 0,
            text: "New".into(),
        }];
        assert!(edit_docx(&source, &source, &edits).is_err());
        assert_eq!(std::fs::read(&source).unwrap(), original);
        let output = dir.join("output.docx");
        std::fs::write(&output, b"preserve").unwrap();
        assert!(edit_docx(&source, &output, &edits).is_err());
        assert_eq!(std::fs::read(&output).unwrap(), b"preserve");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn read_paginates_without_losing_paragraphs() {
        use crate::session::ToolArgument;
        let dir = std::env::temp_dir().join(format!("cordial-docx-pages-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        create_docx(&dir.join("input.docx"), "one\ntwo\nthree").unwrap();
        let runtime = ToolRuntime::new(
            dir.clone(),
            dir.join("scratch"),
            crate::settings::Effort::Medium,
        )
        .unwrap();
        let call = ToolCall {
            id: "test".into(),
            name: "docx_read".into(),
            arguments: vec![
                ToolArgument {
                    name: "path".into(),
                    value: "input.docx".into(),
                },
                ToolArgument {
                    name: "start".into(),
                    value: "1".into(),
                },
                ToolArgument {
                    name: "count".into(),
                    value: "1".into(),
                },
            ],
        };
        let result = read(&runtime, &call);
        assert!(result.ok);
        assert!(result.text.contains("[p1] two"));
        assert!(result.text.contains("start=2"));
        assert!(!result.text.contains("[p2]"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn round_trips_a_created_docx() {
        let dir = std::env::temp_dir().join(format!("cordial-docx-{}", crate::util::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.docx");
        create_docx(&path, "First line\nSecond line").unwrap();
        let paragraphs = read_docx(&path).unwrap();
        assert_eq!(paragraphs, vec!["First line", "Second line"]);
        std::fs::remove_dir_all(dir).ok();
    }
}
