//! The errors of the client (`clients/spec/README.md`, section 3.7).
//!
//! Every error is a variant of [`Error`]. A response with a status of 400 or above becomes an
//! [`ApiError`], wrapped in the variant of its status. A missing or failed credential is
//! [`Error::Authentication`] with no response.

use std::fmt;
use std::time::Duration;

use serde::Deserialize;

/// The number of characters of a body that is not a problem, kept as the `detail`.
const DETAIL_LIMIT: usize = 2000;

/// The error of an API call, parsed from an RFC 9457 problem body.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    /// The HTTP status.
    pub status: u16,
    /// The stable, machine-readable `code` of the problem, or `unknown` when the body is not a
    /// problem.
    pub code: String,
    /// The `title` of the problem, else the reason phrase of the status.
    pub title: String,
    /// The `detail` of the problem, else the first 2000 characters of the body.
    pub detail: String,
    /// The `errors` of the problem: one entry per invalid field. Empty when the body has none.
    pub errors: Vec<serde_json::Value>,
    /// The `X-Request-Id` header of the response.
    pub request_id: Option<String>,
    /// The whole body of the response, as text. The CLI prints it for `oscar api`.
    pub body: String,
}

#[derive(Deserialize)]
struct Problem {
    code: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    errors: Option<Vec<serde_json::Value>>,
}

impl ApiError {
    /// The error of a response with `status`, its `X-Request-Id` and its body.
    pub fn from_response(status: u16, request_id: Option<String>, body: &[u8]) -> Self {
        let text = String::from_utf8_lossy(body).into_owned();
        let reason = reason_phrase(status);
        match serde_json::from_slice::<Problem>(body) {
            Ok(problem) => Self {
                status,
                code: problem.code,
                title: problem.title.unwrap_or_else(|| reason.to_owned()),
                detail: problem.detail.unwrap_or_default(),
                errors: problem.errors.unwrap_or_default(),
                request_id,
                body: text,
            },
            Err(_) => Self {
                status,
                code: "unknown".to_owned(),
                title: reason.to_owned(),
                detail: text.chars().take(DETAIL_LIMIT).collect(),
                errors: Vec::new(),
                request_id,
                body: text,
            },
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}: {}", self.status, self.code, self.detail)?;
        if let Some(id) = &self.request_id {
            write!(f, " (request {id})")?;
        }
        Ok(())
    }
}

impl std::error::Error for ApiError {}

/// The reason phrase of an HTTP status, or an empty string for a status with none.
fn reason_phrase(status: u16) -> &'static str {
    reqwest::StatusCode::from_u16(status)
        .ok()
        .and_then(|status| status.canonical_reason())
        .unwrap_or("")
}

/// No credential, a credential that failed, or a 401 from the API.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthenticationError {
    /// What went wrong, and how to log in. It never holds a credential.
    pub message: String,
    /// The response, for a 401 from the API.
    pub api: Option<Box<ApiError>>,
}

impl fmt::Display for AuthenticationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.api {
            Some(api) => api.fmt(f),
            None => f.write_str(&self.message),
        }
    }
}

/// Every error of the SDK. The [`ApiError`] of a variant is boxed, so the error stays small.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A 400 response.
    #[error("{0}")]
    BadRequest(Box<ApiError>),
    /// A 401 response, or no usable credential (section 1 of the spec).
    #[error("{0}")]
    Authentication(AuthenticationError),
    /// A 403 response.
    #[error("{0}")]
    PermissionDenied(Box<ApiError>),
    /// A 404 response.
    #[error("{0}")]
    NotFound(Box<ApiError>),
    /// A 409 response.
    #[error("{0}")]
    Conflict(Box<ApiError>),
    /// A 422 response.
    #[error("{0}")]
    UnprocessableEntity(Box<ApiError>),
    /// A 429 response.
    #[error("{0}")]
    RateLimit(Box<ApiError>),
    /// A response with a status of 500 or above.
    #[error("{0}")]
    InternalServer(Box<ApiError>),
    /// A response with any other status of 400 or above.
    #[error("{0}")]
    Api(Box<ApiError>),
    /// The request got no response: DNS, a refused or reset connection, TLS. Raised after the
    /// last retry.
    #[error("the connection failed: {0}")]
    Connection(String),
    /// An attempt ran out of time. Raised after the last retry.
    #[error("the request timed out after {0:?}")]
    Timeout(Duration),
    /// A usage error: a wrong base URL or a wrong argument.
    #[error("invalid configuration: {0}")]
    Config(String),
    /// A body that could not be encoded, or a response that could not be decoded.
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
}

impl Error {
    /// The error of a response with a status of 400 or above: the variant of its status.
    pub fn from_api(api: ApiError) -> Self {
        let api = Box::new(api);
        match api.status {
            400 => Self::BadRequest(api),
            401 => Self::Authentication(AuthenticationError {
                message: api.to_string(),
                api: Some(api),
            }),
            403 => Self::PermissionDenied(api),
            404 => Self::NotFound(api),
            409 => Self::Conflict(api),
            422 => Self::UnprocessableEntity(api),
            429 => Self::RateLimit(api),
            500.. => Self::InternalServer(api),
            _ => Self::Api(api),
        }
    }

    /// An [`Error::Authentication`] with no response.
    pub fn authentication(message: impl Into<String>) -> Self {
        Self::Authentication(AuthenticationError {
            message: message.into(),
            api: None,
        })
    }

    /// The response of the error, when the API answered.
    pub fn api_error(&self) -> Option<&ApiError> {
        match self {
            Self::BadRequest(api)
            | Self::PermissionDenied(api)
            | Self::NotFound(api)
            | Self::Conflict(api)
            | Self::UnprocessableEntity(api)
            | Self::RateLimit(api)
            | Self::InternalServer(api)
            | Self::Api(api) => Some(api),
            Self::Authentication(error) => error.api.as_deref(),
            Self::Connection(_) | Self::Timeout(_) | Self::Config(_) | Self::Json(_) => None,
        }
    }

    /// The HTTP status, when the API answered.
    pub fn status(&self) -> Option<u16> {
        self.api_error().map(|api| api.status)
    }

    /// The problem `code`, when the API answered.
    pub fn code(&self) -> Option<&str> {
        self.api_error().map(|api| api.code.as_str())
    }

    /// The `X-Request-Id` of the response, when the API answered and sent one.
    pub fn request_id(&self) -> Option<&str> {
        self.api_error().and_then(|api| api.request_id.as_deref())
    }

    /// The request got no response, or ran out of time.
    pub fn is_network(&self) -> bool {
        matches!(self, Self::Connection(_) | Self::Timeout(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problem(status: u16) -> ApiError {
        let body = serde_json::json!({
            "type": "about:blank",
            "title": "Invalid input",
            "status": status,
            "code": "validation_error",
            "detail": "The name is required.",
            "errors": [{"field": "name", "code": "required"}],
        });
        ApiError::from_response(status, Some("req-1".into()), body.to_string().as_bytes())
    }

    #[test]
    fn parses_a_problem() {
        let api = problem(400);
        assert_eq!(api.status, 400);
        assert_eq!(api.code, "validation_error");
        assert_eq!(api.title, "Invalid input");
        assert_eq!(api.detail, "The name is required.");
        assert_eq!(api.errors.len(), 1);
        assert_eq!(api.request_id.as_deref(), Some("req-1"));
        assert_eq!(
            api.to_string(),
            "400 validation_error: The name is required. (request req-1)"
        );
    }

    #[test]
    fn a_problem_without_optional_fields() {
        let api = ApiError::from_response(409, None, br#"{"code": "idempotency_in_progress"}"#);
        assert_eq!(api.title, "Conflict");
        assert_eq!(api.detail, "");
        assert!(api.errors.is_empty());
        assert_eq!(api.to_string(), "409 idempotency_in_progress: ");
    }

    #[test]
    fn a_body_that_is_not_a_problem() {
        let long = "x".repeat(2500);
        let api = ApiError::from_response(502, None, long.as_bytes());
        assert_eq!(api.code, "unknown");
        assert_eq!(api.title, "Bad Gateway");
        assert_eq!(api.detail.len(), 2000);
        assert_eq!(api.body.len(), 2500);
        let json = ApiError::from_response(500, None, br#"{"error": "boom"}"#);
        assert_eq!(json.code, "unknown");
        assert_eq!(json.detail, r#"{"error": "boom"}"#);
        let odd = ApiError::from_response(599, None, b"");
        assert_eq!(odd.title, "");
    }

    #[test]
    fn maps_each_status_to_its_variant() {
        type Check = fn(&Error) -> bool;
        let cases: [(u16, Check); 10] = [
            (400, |e| matches!(e, Error::BadRequest(_))),
            (401, |e| matches!(e, Error::Authentication(_))),
            (403, |e| matches!(e, Error::PermissionDenied(_))),
            (404, |e| matches!(e, Error::NotFound(_))),
            (409, |e| matches!(e, Error::Conflict(_))),
            (422, |e| matches!(e, Error::UnprocessableEntity(_))),
            (429, |e| matches!(e, Error::RateLimit(_))),
            (500, |e| matches!(e, Error::InternalServer(_))),
            (503, |e| matches!(e, Error::InternalServer(_))),
            (418, |e| matches!(e, Error::Api(_))),
        ];
        for (status, check) in cases {
            let error = Error::from_api(problem(status));
            assert!(check(&error), "{status}: {error:?}");
            assert_eq!(error.status(), Some(status));
            assert_eq!(error.code(), Some("validation_error"));
            assert_eq!(error.request_id(), Some("req-1"));
            assert!(
                error
                    .to_string()
                    .starts_with(&format!("{status} validation_error"))
            );
            assert!(!error.is_network());
        }
    }

    #[test]
    fn errors_without_a_response() {
        let auth = Error::authentication("log in first");
        assert_eq!(auth.to_string(), "log in first");
        assert_eq!(auth.status(), None);
        assert_eq!(auth.code(), None);
        assert_eq!(auth.request_id(), None);
        assert!(Error::Connection("refused".into()).is_network());
        assert!(Error::Timeout(Duration::from_secs(1)).is_network());
        assert_eq!(
            Error::Config("bad".into()).to_string(),
            "invalid configuration: bad"
        );
        let json = Error::from(serde_json::from_str::<u8>("x").unwrap_err());
        assert!(json.to_string().starts_with("invalid JSON"));
        assert!(json.api_error().is_none());
    }
}
