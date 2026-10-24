//! What goes on the clipboard for a capture, and serving it.
//!
//! The media bytes come first so apps that paste images or video take them;
//! the file's URI and path follow for file managers, chats and terminals.

use std::path::Path;

use anyhow::{Context, Result};
use wl_clipboard_rs::copy::{MimeSource, MimeType, Options, Source};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Png,
    Gif,
    Mp4,
}

impl Kind {
    pub fn from_path(path: &Path) -> Option<Kind> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "png" => Some(Kind::Png),
            "gif" => Some(Kind::Gif),
            "mp4" => Some(Kind::Mp4),
            _ => None,
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Kind::Png => "image/png",
            Kind::Gif => "image/gif",
            Kind::Mp4 => "video/mp4",
        }
    }
}

/// The MIME types offered for `path`, in preference order, with their bytes.
/// A PNG offers no plain-text path: apps that ask for text first would paste
/// the path instead of the picture.
pub fn targets(kind: Kind, path: &Path, media: Vec<u8>) -> Vec<(String, Vec<u8>)> {
    let uri = file_uri(path);
    let mut targets = vec![
        (kind.mime().to_owned(), media),
        (
            "text/uri-list".to_owned(),
            format!("{uri}\r\n").into_bytes(),
        ),
    ];
    if kind != Kind::Png {
        let path = path.to_string_lossy().into_owned();
        targets.push((
            "text/plain;charset=utf-8".to_owned(),
            path.clone().into_bytes(),
        ));
        targets.push(("text/plain".to_owned(), path.into_bytes()));
    }
    targets
}

/// `file://` URI with the path's unsafe bytes percent-encoded.
pub fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                uri.push(byte as char)
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

/// Serves `path` on the clipboard until something else takes it. Blocks.
pub fn serve(path: &Path) -> Result<()> {
    let kind = Kind::from_path(path).context("not a .png, .gif or .mp4 file")?;
    let media = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let sources = targets(kind, path, media)
        .into_iter()
        .map(|(mime, bytes)| MimeSource {
            source: Source::Bytes(bytes.into_boxed_slice()),
            mime_type: MimeType::Specific(mime),
        })
        .collect();
    let mut options = Options::new();
    // Offer exactly these types: by default text/uri-list would also be
    // offered as text/plain, and apps that ask for text would paste the path.
    options
        .foreground(true)
        .omit_additional_text_mime_types(true);
    options.copy_multi(sources).context("serving the clipboard")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn png_offers_image_bytes_first_and_no_plain_text() {
        let path = PathBuf::from("/home/u/Screenshots/a.png");
        let t = targets(Kind::Png, &path, vec![1, 2, 3]);
        let mimes: Vec<_> = t.iter().map(|(m, _)| m.as_str()).collect();
        assert_eq!(mimes, ["image/png", "text/uri-list"]);
        assert_eq!(t[0].1, vec![1, 2, 3]);
        assert_eq!(t[1].1, b"file:///home/u/Screenshots/a.png\r\n");
    }

    #[test]
    fn gif_and_video_also_offer_their_path_as_text() {
        let path = PathBuf::from("/tmp/b.mp4");
        let mimes: Vec<_> = targets(Kind::Mp4, &path, vec![])
            .into_iter()
            .map(|(m, _)| m)
            .collect();
        assert_eq!(
            mimes,
            [
                "video/mp4",
                "text/uri-list",
                "text/plain;charset=utf-8",
                "text/plain"
            ]
        );
        let gif = targets(Kind::Gif, &PathBuf::from("/tmp/c.gif"), vec![9]);
        assert_eq!(gif[0], ("image/gif".to_owned(), vec![9]));
    }

    #[test]
    fn kinds_come_from_extensions() {
        assert_eq!(Kind::from_path(Path::new("x.PNG")), Some(Kind::Png));
        assert_eq!(Kind::from_path(Path::new("x.gif")), Some(Kind::Gif));
        assert_eq!(Kind::from_path(Path::new("x.mp4")), Some(Kind::Mp4));
        assert_eq!(Kind::from_path(Path::new("x.webm")), None);
    }

    #[test]
    fn uris_escape_spaces_and_unicode() {
        assert_eq!(
            file_uri(Path::new("/a b/é.png")),
            "file:///a%20b/%C3%A9.png"
        );
    }
}
