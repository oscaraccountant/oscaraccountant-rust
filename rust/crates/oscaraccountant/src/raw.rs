//! A raw call of any path, for `oscar api` and for an endpoint the generated code does not have
//! yet. It follows the same rules as a generated call: the credential, the headers, the timeout,
//! the retries and the errors.

use std::time::Duration;

use bytes::Bytes;
use serde::de::DeserializeOwned;

use crate::client::{Prepared, PreparedBody};
use crate::transport::find_header;
use crate::{Client, Error, HttpMethod, RequestOptions};

/// A raw call, from [`Client::request`].
#[derive(Debug)]
#[must_use = "a request does nothing until it is sent"]
pub struct RawRequest<'a> {
    client: &'a Client,
    method: HttpMethod,
    path: String,
    query: Vec<(String, String)>,
    body: Option<PreparedBody>,
    idempotency_key: Option<String>,
    accept: &'static str,
    options: RequestOptions,
    authenticated: bool,
}

/// The answer of a raw call with a status below 400.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawResponse {
    /// The HTTP status.
    pub status: u16,
    /// The headers, in order. The names are in lower case.
    pub headers: Vec<(String, String)>,
    /// The body.
    pub body: Bytes,
}

impl RawResponse {
    /// The value of the header `name` (in any case), if present.
    pub fn header(&self, name: &str) -> Option<&str> {
        find_header(&self.headers, name)
    }

    /// The body parsed as JSON.
    pub fn json<T: DeserializeOwned>(&self) -> Result<T, Error> {
        Ok(serde_json::from_slice(&self.body)?)
    }
}

impl Client {
    /// A raw call of `path`, relative to the base URL (`/api/me/`). The path is sent as given,
    /// so the caller encodes it; it may hold a query.
    pub fn request(&self, method: HttpMethod, path: impl Into<String>) -> RawRequest<'_> {
        RawRequest {
            client: self,
            method,
            path: path.into(),
            query: Vec::new(),
            body: None,
            idempotency_key: None,
            accept: "application/json",
            options: RequestOptions::default(),
            authenticated: true,
        }
    }
}

impl RawRequest<'_> {
    /// Add a query parameter. The value is percent-encoded.
    pub fn query(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.query.push((name.into(), value.into()));
        self
    }

    /// A JSON body.
    pub fn json(mut self, value: &serde_json::Value) -> Result<Self, Error> {
        let bytes = serde_json::to_vec(value)?;
        self.body = Some(PreparedBody {
            content_type: "application/json".to_owned(),
            bytes: bytes.into(),
        });
        Ok(self)
    }

    /// A body of any type, for example `application/x-www-form-urlencoded`.
    pub fn body(mut self, content_type: impl Into<String>, bytes: impl Into<Bytes>) -> Self {
        self.body = Some(PreparedBody {
            content_type: content_type.into(),
            bytes: bytes.into(),
        });
        self
    }

    /// Send this `Idempotency-Key`. A call with a key may be retried.
    pub fn idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.idempotency_key = Some(key.into());
        self
    }

    /// Send an `Idempotency-Key` from the UUID source of the client.
    pub fn idempotent(mut self) -> Self {
        self.idempotency_key = Some(self.client.new_idempotency_key());
        self
    }

    /// The `Accept` header. Default `application/json`.
    pub fn accept(mut self, accept: &'static str) -> Self {
        self.accept = accept;
        self
    }

    /// Send no `Authorization` header: for the OAuth endpoints of the CLI, and the metadata
    /// document.
    pub fn without_credential(mut self) -> Self {
        self.authenticated = false;
        self
    }

    /// The timeout of each attempt of this call.
    pub fn timeout(mut self, value: Duration) -> Self {
        self.options.timeout = Some(value);
        self
    }

    /// The number of retries of this call. 0 turns them off.
    pub fn max_retries(mut self, value: u32) -> Self {
        self.options.max_retries = Some(value);
        self
    }

    /// An extra header. It cannot replace `Authorization` or `Idempotency-Key`.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.options.headers.push((name.into(), value.into()));
        self
    }

    /// Send the call. A status of 400 or above is an [`Error`]; its [`ApiError`](crate::ApiError)
    /// holds the whole body.
    pub async fn send(self) -> Result<RawResponse, Error> {
        let path = if self.path.starts_with('/') {
            self.path
        } else {
            format!("/{}", self.path)
        };
        let prepared = Prepared {
            method: self.method,
            path,
            query: self.query,
            body: self.body,
            idempotency_key: self.idempotency_key,
            accept: self.accept,
            options: self.options,
            authenticated: self.authenticated,
        };
        let answer = self.client.send(prepared, false).await?;
        let (status, headers) = (answer.status, answer.headers.clone());
        Ok(RawResponse {
            status,
            headers,
            body: answer.into_bytes(),
        })
    }
}
