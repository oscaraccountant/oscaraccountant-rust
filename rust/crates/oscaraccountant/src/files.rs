//! Uploads and downloads (`clients/spec/README.md`, section 3.8).

use std::fmt;
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::{BufMut, Bytes, BytesMut};
use futures::stream::{BoxStream, Stream, StreamExt, TryStreamExt};

use crate::{Error, MultipartField};

/// The file name of an upload when the caller gives none.
const DEFAULT_FILE_NAME: &str = "upload";
/// The content type of an upload when the caller gives none.
const DEFAULT_CONTENT_TYPE: &str = "application/octet-stream";

/// A file to upload. The content is held in memory, so a retry sends the same bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct Upload {
    content: Bytes,
    file_name: Option<String>,
    content_type: Option<String>,
}

impl Upload {
    /// An upload of `content`, with the file name `upload` and the type
    /// `application/octet-stream` unless set.
    pub fn new(content: impl Into<Bytes>) -> Self {
        Self {
            content: content.into(),
            file_name: None,
            content_type: None,
        }
    }

    /// Read the file at `path`. Its name becomes the file name of the upload.
    pub async fn from_path(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref();
        let content = tokio::fs::read(path).await?;
        let mut upload = Self::new(content);
        upload.file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        Ok(upload)
    }

    /// Set the file name.
    pub fn file_name(mut self, name: impl Into<String>) -> Self {
        self.file_name = Some(name.into());
        self
    }

    /// Set the content type, for example `application/pdf`.
    pub fn content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    /// The content.
    pub fn content(&self) -> &Bytes {
        &self.content
    }

    /// The file name that is sent.
    pub fn sent_file_name(&self) -> &str {
        self.file_name.as_deref().unwrap_or(DEFAULT_FILE_NAME)
    }

    /// The content type that is sent.
    pub fn sent_content_type(&self) -> &str {
        self.content_type.as_deref().unwrap_or(DEFAULT_CONTENT_TYPE)
    }
}

impl fmt::Debug for Upload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Upload")
            .field("file_name", &self.sent_file_name())
            .field("content_type", &self.sent_content_type())
            .field("length", &self.content.len())
            .finish()
    }
}

/// A `multipart/form-data` body: the `Content-Type` with its boundary, and the bytes.
pub(crate) fn encode_multipart(fields: &[MultipartField], boundary: &str) -> (String, Bytes) {
    let mut body = BytesMut::new();
    for field in fields {
        body.put_slice(format!("--{boundary}\r\n").as_bytes());
        match field {
            MultipartField::Text { name, value } => {
                body.put_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{}\"\r\n\r\n",
                        quote(name)
                    )
                    .as_bytes(),
                );
                body.put_slice(value.as_bytes());
            }
            MultipartField::File { name, file } => {
                body.put_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
                        quote(name),
                        quote(file.sent_file_name()),
                        file.sent_content_type().replace(['\r', '\n'], "")
                    )
                    .as_bytes(),
                );
                body.put_slice(file.content());
            }
        }
        body.put_slice(b"\r\n");
    }
    body.put_slice(format!("--{boundary}--\r\n").as_bytes());
    (
        format!("multipart/form-data; boundary={boundary}"),
        body.freeze(),
    )
}

/// A name in a quoted `Content-Disposition` parameter, escaped as the HTML standard does.
fn quote(value: &str) -> String {
    value
        .replace('"', "%22")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// The bytes of a download, as a stream, with what the response says about them.
pub struct Download {
    content_type: Option<String>,
    content_length: Option<u64>,
    file_name: Option<String>,
    body: BoxStream<'static, Result<Bytes, Error>>,
}

impl Download {
    pub(crate) fn new(
        content_type: Option<String>,
        content_length: Option<u64>,
        file_name: Option<String>,
        body: BoxStream<'static, Result<Bytes, Error>>,
    ) -> Self {
        Self {
            content_type,
            content_length,
            file_name,
            body,
        }
    }

    /// The `Content-Type` of the response.
    pub fn content_type(&self) -> Option<&str> {
        self.content_type.as_deref()
    }

    /// The `Content-Length` of the response, when known.
    pub fn content_length(&self) -> Option<u64> {
        self.content_length
    }

    /// The file name of the `Content-Disposition` header.
    pub fn file_name(&self) -> Option<&str> {
        self.file_name.as_deref()
    }

    /// Read the whole content.
    pub async fn bytes(self) -> Result<Bytes, Error> {
        let chunks: Vec<Bytes> = self.body.try_collect().await?;
        Ok(chunks.concat().into())
    }
}

impl Stream for Download {
    type Item = Result<Bytes, Error>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.body.poll_next_unpin(context)
    }
}

impl fmt::Debug for Download {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Download")
            .field("content_type", &self.content_type)
            .field("content_length", &self.content_length)
            .field("file_name", &self.file_name)
            .finish_non_exhaustive()
    }
}

/// The file name of a `Content-Disposition` header: `filename*` (RFC 5987, UTF-8) wins over
/// `filename`.
pub(crate) fn disposition_file_name(value: &str) -> Option<String> {
    let mut plain = None;
    for part in split_parameters(value).into_iter().skip(1) {
        let Some((name, raw)) = part.split_once('=') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let raw = raw.trim();
        if name == "filename*" {
            let mut pieces = raw.splitn(3, '\'');
            let (Some(charset), Some(_language), Some(encoded)) =
                (pieces.next(), pieces.next(), pieces.next())
            else {
                continue;
            };
            if charset.eq_ignore_ascii_case("utf-8") {
                if let Ok(decoded) = percent_encoding::percent_decode_str(encoded).decode_utf8() {
                    return Some(decoded.into_owned());
                }
            }
        } else if name == "filename" {
            plain = Some(unquote(raw));
        }
    }
    plain.filter(|name| !name.is_empty())
}

/// Split a header value on `;`, except inside a quoted string.
fn split_parameters(value: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut start, mut quoted, mut escaped) = (0, false, false);
    for (index, character) in value.char_indices() {
        match character {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ';' if !quoted => {
                parts.push(&value[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&value[start..]);
    parts
}

fn unquote(value: &str) -> String {
    let Some(inner) = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return value.to_owned();
    };
    let mut result = String::with_capacity(inner.len());
    let mut characters = inner.chars();
    while let Some(character) = characters.next() {
        if character == '\\' {
            if let Some(next) = characters.next() {
                result.push(next);
            }
        } else {
            result.push(character);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_upload_has_defaults() {
        let upload = Upload::new(vec![1, 2, 3]);
        assert_eq!(upload.sent_file_name(), "upload");
        assert_eq!(upload.sent_content_type(), "application/octet-stream");
        assert_eq!(upload.content().as_ref(), &[1, 2, 3]);
        assert_eq!(
            format!("{upload:?}"),
            r#"Upload { file_name: "upload", content_type: "application/octet-stream", length: 3 }"#
        );
        let named = upload.file_name("a.pdf").content_type("application/pdf");
        assert_eq!(named.sent_file_name(), "a.pdf");
        assert_eq!(named.sent_content_type(), "application/pdf");
    }

    #[tokio::test]
    async fn an_upload_from_a_path_takes_its_name() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("invoice 1.pdf");
        std::fs::write(&path, b"%PDF").unwrap();
        let upload = Upload::from_path(&path).await.unwrap();
        assert_eq!(upload.sent_file_name(), "invoice 1.pdf");
        assert_eq!(upload.content().as_ref(), b"%PDF");
        assert!(
            Upload::from_path(directory.path().join("missing"))
                .await
                .is_err()
        );
    }

    #[test]
    fn encodes_a_multipart_body() {
        let fields = vec![
            MultipartField::File {
                name: "file",
                file: Upload::new("abc")
                    .file_name("a\"b.pdf")
                    .content_type("application/pdf"),
            },
            MultipartField::Text {
                name: "kind",
                value: "invoice".into(),
            },
        ];
        let (content_type, body) = encode_multipart(&fields, "B");
        assert_eq!(content_type, "multipart/form-data; boundary=B");
        assert_eq!(
            std::str::from_utf8(&body).unwrap(),
            "--B\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a%22b.pdf\"\r\n\
             Content-Type: application/pdf\r\n\r\nabc\r\n\
             --B\r\nContent-Disposition: form-data; name=\"kind\"\r\n\r\ninvoice\r\n--B--\r\n"
        );
        assert_eq!(quote("a\r\nb"), "a%0D%0Ab");
    }

    #[test]
    fn reads_the_file_name_of_a_disposition() {
        let cases = [
            ("attachment; filename=\"invoice.pdf\"", Some("invoice.pdf")),
            ("inline; filename=plain.pdf", Some("plain.pdf")),
            (
                "attachment; filename=\"a;b \\\"c\\\".pdf\"",
                Some("a;b \"c\".pdf"),
            ),
            (
                "attachment; filename=\"x.pdf\"; filename*=UTF-8''arve%20%C3%B6.pdf",
                Some("arve ö.pdf"),
            ),
            ("attachment; filename*=utf-8'et'%C3%96.pdf", Some("Ö.pdf")),
            (
                "attachment; filename*=ISO-8859-1''x.pdf; filename=\"y.pdf\"",
                Some("y.pdf"),
            ),
            (
                "attachment; filename*=UTF-8''%FF; filename=z.pdf",
                Some("z.pdf"),
            ),
            (
                "attachment; filename*=broken; filename=w.pdf",
                Some("w.pdf"),
            ),
            ("attachment; flag; filename=\"\"", None),
            ("inline", None),
            ("attachment; filename=\"trailing\\", Some("\"trailing\\")),
        ];
        for (value, expected) in cases {
            assert_eq!(disposition_file_name(value).as_deref(), expected, "{value}");
        }
        // A trailing backslash escapes nothing and is dropped.
        assert_eq!(unquote("\"a\\\""), "a");
    }

    #[tokio::test]
    async fn a_download_streams_and_collects() {
        let chunks =
            futures::stream::iter(vec![Ok(Bytes::from("ab")), Ok(Bytes::from("c"))]).boxed();
        let download = Download::new(
            Some("application/pdf".into()),
            Some(3),
            Some("a.pdf".into()),
            chunks,
        );
        assert_eq!(download.content_type(), Some("application/pdf"));
        assert_eq!(download.content_length(), Some(3));
        assert_eq!(download.file_name(), Some("a.pdf"));
        assert!(format!("{download:?}").contains("a.pdf"));
        assert_eq!(download.bytes().await.unwrap(), Bytes::from("abc"));

        let chunks = futures::stream::iter(vec![Ok(Bytes::from("x"))]).boxed();
        let mut download = Download::new(None, None, None, chunks);
        assert_eq!(download.next().await.unwrap().unwrap(), Bytes::from("x"));
        assert!(download.next().await.is_none());
    }
}
