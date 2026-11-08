//! Image attachments: read from a local file and checked before the upload.

use std::{
    io::Read,
    path::{Path, PathBuf},
};

use crate::model::MAX_ATTACHMENT_BYTES;

/// An image ready for upload.
#[derive(Debug)]
pub struct Image {
    pub data: Vec<u8>,
    pub mime_type: &'static str,
    /// Name sent to Pushover. The local file name stays local.
    pub file_name: &'static str,
}

/// Reads the image at `path`, an absolute path.
///
/// The type comes from the file's first bytes, not its name, so a file that
/// is not an image cannot be sent by renaming it.
pub async fn load_image(path: PathBuf) -> Result<Image, String> {
    tokio::task::spawn_blocking(move || read_image(&path))
        .await
        .map_err(|e| format!("Could not read the attachment: {e}"))?
}

fn read_image(path: &Path) -> Result<Image, String> {
    let shown = path.display();
    // Checked before opening: opening a named pipe blocks until something
    // writes to it.
    let metadata = std::fs::metadata(path)
        .map_err(|e| format!("Could not read the attachment {shown}: {e}"))?;
    if !metadata.is_file() {
        return Err(format!("The attachment {shown} is not a file"));
    }
    if metadata.len() > MAX_ATTACHMENT_BYTES {
        return Err(too_large(path, metadata.len()));
    }
    let file = std::fs::File::open(path)
        .map_err(|e| format!("Could not open the attachment {shown}: {e}"))?;

    // Read one byte past the limit, in case the file grew since the check.
    let mut data = Vec::new();
    file.take(MAX_ATTACHMENT_BYTES + 1)
        .read_to_end(&mut data)
        .map_err(|e| format!("Could not read the attachment {shown}: {e}"))?;
    if data.len() as u64 > MAX_ATTACHMENT_BYTES {
        return Err(too_large(path, data.len() as u64));
    }

    let (mime_type, file_name) = image_type(&data)
        .ok_or_else(|| format!("The attachment {shown} is not a JPEG, PNG, GIF or WebP image"))?;
    Ok(Image {
        data,
        mime_type,
        file_name,
    })
}

fn too_large(path: &Path, size: u64) -> String {
    format!(
        "The attachment {} has {size} bytes; Pushover accepts at most {MAX_ATTACHMENT_BYTES} (5 MB)",
        path.display()
    )
}

/// MIME type and upload name of an image, from its signature.
fn image_type(data: &[u8]) -> Option<(&'static str, &'static str)> {
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(("image/jpeg", "attachment.jpg"))
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("image/png", "attachment.png"))
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some(("image/gif", "attachment.gif"))
    } else if data.len() >= 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
        Some(("image/webp", "attachment.webp"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_types_come_from_the_signature() {
        assert_eq!(
            image_type(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10]).map(|t| t.0),
            Some("image/jpeg")
        );
        assert_eq!(
            image_type(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").map(|t| t.0),
            Some("image/png")
        );
        assert_eq!(image_type(b"GIF89a\x01\0").map(|t| t.0), Some("image/gif"));
        assert_eq!(
            image_type(b"RIFF\x24\0\0\0WEBPVP8 ").map(|t| t.0),
            Some("image/webp")
        );
        assert_eq!(image_type(b"RIFF\x24\0\0\0WAVEfmt "), None);
        assert_eq!(image_type(b"-----BEGIN OPENSSH PRIVATE KEY-----"), None);
        assert_eq!(image_type(b""), None);
    }

    #[test]
    fn files_are_checked_before_upload() {
        let dir = tempfile::tempdir().unwrap();

        let png = dir.path().join("chart.png");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\nrest").unwrap();
        let image = read_image(&png).unwrap();
        assert_eq!(image.mime_type, "image/png");
        assert_eq!(image.file_name, "attachment.png");
        assert_eq!(image.data, b"\x89PNG\r\n\x1a\nrest");

        let renamed = dir.path().join("secret.png");
        std::fs::write(&renamed, b"not an image").unwrap();
        let error = read_image(&renamed).unwrap_err();
        assert!(error.contains("not a JPEG, PNG, GIF or WebP"), "{error}");

        let error = read_image(dir.path()).unwrap_err();
        assert!(error.contains("not a file"), "{error}");

        let error = read_image(&dir.path().join("missing.png")).unwrap_err();
        assert!(error.contains("Could not read the attachment"), "{error}");

        let large = dir.path().join("large.png");
        let file = std::fs::File::create(&large).unwrap();
        file.set_len(MAX_ATTACHMENT_BYTES + 1).unwrap();
        let error = read_image(&large).unwrap_err();
        assert!(error.contains("at most 5242880"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn a_named_pipe_is_refused_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = dir.path().join("pipe.png");
        let status = std::process::Command::new("mkfifo")
            .arg(&pipe)
            .status()
            .expect("mkfifo runs");
        assert!(status.success());

        // Opening the pipe would wait for a writer forever, so the check runs
        // in a thread that has to answer in time.
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || sender.send(read_image(&pipe)));
        let error = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("read_image returns without blocking")
            .unwrap_err();
        assert!(error.contains("not a file"), "{error}");
    }
}
