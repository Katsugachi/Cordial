//! Persist locally rendered PDF pages as bounded scratch attachments.
//! The UI cannot choose output paths: every inline attachment is written under
//! a freshly generated scratch directory, and is never executed.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use base64::Engine;
use cordial_core::session::{Attachment, AttachmentKind};

use crate::commands::AttachmentInput;

const MAX_INLINE_ITEMS: usize = 6;
const MAX_INLINE_IMAGE_BYTES: usize = 9_000_000;
const MAX_INLINE_TEXT_BYTES: usize = 400_000;
const MAX_SCRATCH_BYTES: u64 = 100_000_000;
const MAX_PAGE_PIXELS: u64 = 2_000_000;
const PNG_PREFIX: &str = "data:image/png;base64,";

struct Prepared {
    attachment: Attachment,
    bytes: Option<Vec<u8>>,
    extension: &'static str,
}

/// Validate all inputs before writing anything. The scratch files are retained
/// because saved sessions can refer to them in later turns.
pub fn prepare(inputs: Vec<AttachmentInput>, scratch: &Path) -> Result<Vec<Attachment>, String> {
    let mut prepared = Vec::with_capacity(inputs.len());
    let mut inline_count = 0;
    for input in inputs {
        let kind = if input.kind == "image" {
            AttachmentKind::Image
        } else {
            AttachmentKind::Text
        };
        let name = input.name.as_deref().unwrap_or_else(|| {
            Path::new(&input.path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("attachment")
        });
        let name: String = name.chars().filter(|c| !c.is_control()).take(160).collect();
        let attachment = Attachment {
            kind,
            name,
            path: input.path.clone(),
        };
        let (bytes, extension) = if let Some(data) = input.inline_data {
            inline_count += 1;
            if inline_count > MAX_INLINE_ITEMS || !input.path.starts_with("inline:") {
                return Err("Invalid or excessive inline attachments.".into());
            }
            match input.kind.as_str() {
                "image" => {
                    let encoded = data
                        .strip_prefix(PNG_PREFIX)
                        .ok_or("Expected a PNG image.")?;
                    if encoded.len() > (MAX_INLINE_IMAGE_BYTES + 2) / 3 * 4 {
                        return Err("PDF page image is too large.".into());
                    }
                    let decoded = base64::engine::general_purpose::STANDARD
                        .decode(encoded)
                        .map_err(|_| "Invalid PDF page image encoding.")?;
                    let sanitized = sanitize_png(&decoded)?;
                    (Some(sanitized), "png")
                }
                "text" => {
                    if data.len() > MAX_INLINE_TEXT_BYTES || data.contains('\0') {
                        return Err("PDF page text is invalid or too large.".into());
                    }
                    (Some(data.into_bytes()), "txt")
                }
                _ => return Err("Invalid inline attachment type.".into()),
            }
        } else {
            (None, "")
        };
        prepared.push(Prepared {
            attachment,
            bytes,
            extension,
        });
    }
    if inline_count == 0 {
        return Ok(prepared.into_iter().map(|p| p.attachment).collect());
    }

    fs::create_dir_all(scratch)
        .map_err(|err| format!("Cannot create attachment scratch folder: {err}"))?;
    let added: u64 = prepared
        .iter()
        .filter_map(|p| p.bytes.as_ref())
        .map(|bytes| bytes.len() as u64)
        .sum();
    if scratch_usage(scratch)?.saturating_add(added) > MAX_SCRATCH_BYTES {
        return Err("PDF attachment storage is full; review the PDF scratch folder before attaching more pages.".into());
    }
    let directory = scratch.join(uuid::Uuid::new_v4().to_string());
    fs::create_dir(&directory).map_err(|err| format!("Cannot create PDF scratch folder: {err}"))?;
    for (index, item) in prepared.iter_mut().enumerate() {
        if let Some(bytes) = &item.bytes {
            let file = directory.join(format!("page-{index}.{}", item.extension));
            let mut handle = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&file)
                .map_err(|err| format!("Cannot save PDF page: {err}"))?;
            handle
                .write_all(bytes)
                .map_err(|err| format!("Cannot save PDF page: {err}"))?;
            item.attachment.path = file.to_string_lossy().into_owned();
        }
    }
    Ok(prepared.into_iter().map(|p| p.attachment).collect())
}

fn scratch_usage(root: &Path) -> Result<u64, String> {
    let mut total = 0u64;
    for (index, entry) in fs::read_dir(root).map_err(|e| e.to_string())?.enumerate() {
        if index >= 1_000 {
            return Err("Too many PDF attachment folders.".into());
        }
        let entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if !kind.is_dir() || kind.is_symlink() {
            return Err("Unexpected entry in PDF attachment storage.".into());
        }
        for (index, child) in fs::read_dir(entry.path())
            .map_err(|e| e.to_string())?
            .enumerate()
        {
            if index >= MAX_INLINE_ITEMS {
                return Err("Unexpected entries in PDF attachment folder.".into());
            }
            let child = child.map_err(|e| e.to_string())?;
            let kind = child.file_type().map_err(|e| e.to_string())?;
            if !kind.is_file() || kind.is_symlink() {
                return Err("Unexpected entry in PDF attachment storage.".into());
            }
            total = total.saturating_add(child.metadata().map_err(|e| e.to_string())?.len());
        }
    }
    Ok(total)
}

fn sanitize_png(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.len() > MAX_INLINE_IMAGE_BYTES
        || bytes.len() < 24
        || &bytes[..8] != b"\x89PNG\r\n\x1a\n"
        || &bytes[12..16] != b"IHDR"
    {
        return Err("Invalid or oversized PDF page image.".into());
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PAGE_PIXELS {
        return Err("Invalid PDF page dimensions.".into());
    }
    // Decode with explicit limits, then re-encode only pixel data. This rejects
    // malformed streams and discards metadata and trailing bytes before the
    // image reaches the model's native decoder.
    let decoder = png::Decoder::new_with_limits(
        std::io::Cursor::new(bytes),
        png::Limits { bytes: 16_000_000 },
    );
    let mut reader = decoder.read_info().map_err(|_| "Invalid PDF page PNG.")?;
    if reader.info().bit_depth != png::BitDepth::Eight
        || reader.info().color_type == png::ColorType::Indexed
    {
        return Err("Unsupported PDF page PNG format.".into());
    }
    let size = reader
        .output_buffer_size()
        .ok_or("Invalid PDF page PNG dimensions.")?;
    if size > 8_000_000 {
        return Err("Decoded PDF page is too large.".into());
    }
    let mut pixels = vec![0; size];
    let info = reader
        .next_frame(&mut pixels)
        .map_err(|_| "Invalid PDF page PNG data.")?;
    let mut sanitized = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut sanitized, width, height);
        encoder.set_color(info.color_type);
        encoder.set_depth(info.bit_depth);
        let mut writer = encoder
            .write_header()
            .map_err(|_| "Could not prepare PDF page.")?;
        writer
            .write_image_data(&pixels[..info.buffer_size()])
            .map_err(|_| "Could not prepare PDF page.")?;
    }
    if sanitized.len() > MAX_INLINE_IMAGE_BYTES {
        return Err("Prepared PDF page is too large.".into());
    }
    Ok(sanitized)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(kind: &str, data: &str) -> AttachmentInput {
        AttachmentInput {
            path: "inline:test:0".into(),
            kind: kind.into(),
            name: Some("worksheet".into()),
            inline_data: Some(data.into()),
        }
    }

    #[test]
    fn inline_text_is_saved_inside_generated_scratch_directory() {
        let root = std::env::temp_dir().join(format!("cordial-pdf-{}", uuid::Uuid::new_v4()));
        let result = prepare(vec![input("text", "Question 1")], &root).unwrap();
        let saved = Path::new(&result[0].path);
        assert!(saved.starts_with(&root));
        assert_eq!(fs::read_to_string(saved).unwrap(), "Question 1");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn page_image_and_text_are_persisted_for_the_session() {
        let root = std::env::temp_dir().join(format!("cordial-pdf-pages-{}", uuid::Uuid::new_v4()));
        let image = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAABHNCSVQICAgIfAhkiAAAAAFzUkdCAK7OHOkAAAANSURBVAiZY2BgYPgPAAEEAQB9ssjfAAAAAElFTkSuQmCC";
        let pages = prepare(
            vec![input("image", image), input("text", "Question 1")],
            &root,
        )
        .unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].kind, AttachmentKind::Image);
        assert_eq!(pages[1].kind, AttachmentKind::Text);
        assert!(fs::read(&pages[0].path)
            .unwrap()
            .starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(fs::read_to_string(&pages[1].path).unwrap(), "Question 1");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_invalid_or_excessive_inline_data_without_writing() {
        let root = std::env::temp_dir().join(format!("cordial-pdf-{}", uuid::Uuid::new_v4()));
        assert!(prepare(vec![input("image", "data:image/png;base64,AAAA")], &root).is_err());
        assert!(prepare((0..7).map(|_| input("text", "x")).collect(), &root).is_err());
        assert!(prepare(vec![input("text", "x\0y")], &root).is_err());
        let mut escaped = input("text", "x");
        escaped.path = "../../outside".into();
        assert!(prepare(vec![escaped], &root).is_err());
        assert!(prepare(vec![input("command", "x")], &root).is_err());
        assert!(!root.exists());
    }

    #[test]
    fn scratch_quota_refuses_more_output() {
        let root = std::env::temp_dir().join(format!("cordial-pdf-quota-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("old")).unwrap();
        let file = fs::File::create(root.join("old/page.txt")).unwrap();
        file.set_len(MAX_SCRATCH_BYTES).unwrap();
        assert!(prepare(vec![input("text", "x")], &root).is_err());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn png_dimensions_are_bounded() {
        let mut png = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAABHNCSVQICAgIfAhkiAAAAAFzUkdCAK7OHOkAAAANSURBVAiZY2BgYPgPAAEEAQB9ssjfAAAAAElFTkSuQmCC").unwrap();
        png.extend_from_slice(b"untrusted trailing content");
        let sanitized = sanitize_png(&png).unwrap();
        assert!(!sanitized
            .windows(b"untrusted trailing content".len())
            .any(|window| window == b"untrusted trailing content"));
        png[16..20].copy_from_slice(&100_000u32.to_be_bytes());
        assert!(sanitize_png(&png).is_err());
        assert!(sanitize_png(&png[..24]).is_err());
    }
}
