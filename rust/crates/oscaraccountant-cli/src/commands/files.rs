//! `oscar files upload <PATH>` and `oscar files download <FILE_ID>`: the two file operations, on
//! the SDK's upload and streamed download.

use std::path::{Path, PathBuf};

use clap::ArgMatches;
use futures::StreamExt;
use oscaraccountant::Upload;
use serde_json::json;
use tokio::io::AsyncWriteExt;

use super::operation::check_value;
use super::string;
use crate::cli::flag;
use crate::error::{CliError, Result};
use crate::generated::{Body, Command};
use crate::session::App;

/// Upload a file to the company.
pub async fn upload(app: &mut App, operation: &'static Command, leaf: &ArgMatches) -> Result<()> {
    let config = app.config()?;
    let selection = app.select(&config)?;
    let selection = app.with_company(selection, string(leaf, "company")).await?;
    let company = app.company(string(leaf, "company"), &selection)?;
    let mut fields = Vec::new();
    if let Body::Multipart { fields: table } = operation.body {
        for field in table {
            if let Some(value) = string(leaf, &format!("field:{}", field.name)) {
                check_value(field.long, field.kind, value)?;
                fields.push((field.name, value.to_owned()));
            }
        }
    }
    let path = PathBuf::from(string(leaf, "file").unwrap_or_default());
    let file = Upload::from_path(&path)
        .await
        .map_err(|error| CliError::usage(format!("Cannot read {}: {error}", path.display())))?;
    let client = app.client_for(&selection).await?;
    let mut request = client.files().upload(company, file);
    for (name, value) in fields {
        request = match name {
            "kind" => request.kind(value.as_str()),
            "received_on" => request.received_on(value),
            "bank_transaction_id" => request.bank_transaction_id(value),
            other => {
                return Err(CliError::usage(format!(
                    "This version of the CLI cannot send `{other}`."
                )));
            }
        };
    }
    if let Some(key) = string(leaf, "idempotency-key") {
        request = request.idempotency_key(key);
    }
    let stored = request.send().await?;
    let value =
        serde_json::to_value(&stored).map_err(|error| CliError::failed(error.to_string()))?;
    let json = app.json();
    app.out.value(&value, json);
    Ok(())
}

/// Download the content of a file: to `-o PATH`, to standard output with `-o -`, else to the
/// file name the server gives, in the current directory.
pub async fn download(app: &mut App, leaf: &ArgMatches) -> Result<()> {
    let config = app.config()?;
    let selection = app.select(&config)?;
    let selection = app.with_company(selection, string(leaf, "company")).await?;
    let company = app.company(string(leaf, "company"), &selection)?;
    let file_id = string(leaf, "path:file_id").unwrap_or_default().to_owned();
    let output = string(leaf, "output").map(str::to_owned);
    let force = flag(leaf, "force");
    if let Some(path) = output.as_deref().filter(|path| *path != "-") {
        refuse_existing(Path::new(path), force)?;
    }
    let client = app.client_for(&selection).await?;
    let mut download = client
        .files()
        .download(company, file_id.clone())
        .send()
        .await?;
    let content_type = download.content_type().map(str::to_owned);
    if output.as_deref() == Some("-") {
        while let Some(chunk) = download.next().await {
            let chunk = chunk?;
            app.out
                .stdout()
                .write_all(&chunk)
                .map_err(|error| CliError::failed(format!("Cannot write the file: {error}")))?;
        }
        return Ok(());
    }
    let target = match output {
        Some(path) => PathBuf::from(path),
        None => {
            let path = PathBuf::from(safe_name(download.file_name(), &file_id));
            refuse_existing(&path, force)?;
            path
        }
    };
    let mut file = tokio::fs::File::create(&target)
        .await
        .map_err(|error| CliError::io("write", &target, &error))?;
    let mut size: u64 = 0;
    while let Some(chunk) = download.next().await {
        let chunk = chunk?;
        size += chunk.len() as u64;
        file.write_all(&chunk)
            .await
            .map_err(|error| CliError::io("write", &target, &error))?;
    }
    file.flush()
        .await
        .map_err(|error| CliError::io("write", &target, &error))?;
    if app.json() {
        app.out.json(&json!({
            "path": target.display().to_string(),
            "size": size,
            "content_type": content_type,
        }));
    } else {
        app.out
            .note(&format!("Saved {} ({size} bytes).", target.display()));
    }
    Ok(())
}

fn refuse_existing(path: &Path, force: bool) -> Result<()> {
    if path.exists() && !force {
        return Err(CliError::usage(format!(
            "{} exists. Add --force to replace it, or choose another path with -o.",
            path.display()
        )));
    }
    Ok(())
}

/// The file name of the server as a plain name in the current directory: no directory part, and
/// never `.` or `..`. `fallback` when there is none.
pub fn safe_name(name: Option<&str>, fallback: &str) -> String {
    let base = name
        .and_then(|name| name.rsplit(['/', '\\']).next())
        .map(str::trim)
        .filter(|base| !base.is_empty() && *base != "." && *base != "..");
    base.map_or_else(|| fallback.to_owned(), str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_safe_file_name() {
        assert_eq!(safe_name(Some("invoice.pdf"), "f1"), "invoice.pdf");
        assert_eq!(safe_name(Some("../../etc/passwd"), "f1"), "passwd");
        assert_eq!(safe_name(Some("C:\\x\\a.pdf"), "f1"), "a.pdf");
        assert_eq!(safe_name(Some(".."), "f1"), "f1");
        assert_eq!(safe_name(Some("dir/"), "f1"), "f1");
        assert_eq!(safe_name(None, "f1"), "f1");
    }

    #[test]
    fn refuses_to_replace() {
        let file = tempfile::NamedTempFile::new().unwrap();
        assert_eq!(
            refuse_existing(file.path(), false).unwrap_err().code,
            crate::error::EXIT_USAGE
        );
        refuse_existing(file.path(), true).unwrap();
        refuse_existing(Path::new("/no/such/file"), false).unwrap();
    }
}
