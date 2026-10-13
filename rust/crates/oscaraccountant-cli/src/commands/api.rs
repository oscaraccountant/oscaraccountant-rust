//! `oscar api <METHOD> <PATH>`: a raw call with the stored credential, like `gh api`.

use clap::ArgMatches;
use serde_json::Value;

use super::{method, read_data, string};
use crate::error::Result;
use crate::session::App;

/// Send the call and print the body. An error body is printed on standard output too, so a
/// script can read it.
pub async fn run(app: &mut App, matches: &ArgMatches) -> Result<()> {
    let leaf = super::leaf(matches);
    let method = method(string(leaf, "method").unwrap_or_default())?;
    let path = string(leaf, "path").unwrap_or_default().to_owned();
    let data = string(leaf, "data").map(read_data).transpose()?;
    let (client, _) = app.client().await?;
    let mut request = client.request(method, path);
    if let Some(data) = &data {
        request = request.json(data)?;
    }
    if let Some(key) = string(leaf, "idempotency-key") {
        request = request.idempotency_key(key);
    }
    match request.send().await {
        Ok(response) => {
            write_body(app, &response.body);
            Ok(())
        }
        Err(error) => {
            if let Some(api) = error.api_error() {
                write_body(app, api.body.as_bytes());
            }
            Err(error.into())
        }
    }
}

/// JSON is printed as JSON (indented on a terminal); any other body as it is.
fn write_body(app: &mut App, body: &[u8]) {
    if body.is_empty() {
        return;
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(value) => app.out.json(&value),
        Err(_) => {
            let _ = app.out.stdout().write_all(body);
        }
    }
}
