//! The HTTP layer: one trait, [`HttpTransport`], and its default, [`ReqwestTransport`].
//!
//! The client builds each attempt as an [`HttpRequest`], with every header already set, and hands
//! it to the transport. The timeouts, the retries and the errors are the client's. A test, the
//! conformance runner or the CLI can inject its own transport with
//! [`ClientBuilder::transport`](crate::ClientBuilder::transport).

use std::fmt;
use std::sync::Arc;

use bytes::Bytes;
use futures::future::BoxFuture;
use futures::stream::{BoxStream, StreamExt, TryStreamExt};

use crate::{Error, HttpMethod};

/// One attempt of a request, as it goes on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// The HTTP method.
    pub method: HttpMethod,
    /// The full URL, with the query.
    pub url: String,
    /// The headers, in order. The names are in lower case.
    pub headers: Vec<(String, String)>,
    /// The body, if any.
    pub body: Option<Bytes>,
}

impl HttpRequest {
    /// The value of the header `name` (in any case), if present.
    pub fn header(&self, name: &str) -> Option<&str> {
        find_header(&self.headers, name)
    }
}

/// The body of a response: a stream of chunks.
pub type ByteStream = BoxStream<'static, Result<Bytes, TransportError>>;

/// A response: the status, the headers and the body as a stream.
pub struct HttpResponse {
    /// The HTTP status.
    pub status: u16,
    /// The headers, in order. The names are in lower case.
    pub headers: Vec<(String, String)>,
    /// The body.
    pub body: ByteStream,
}

impl HttpResponse {
    /// A response with the whole body at once, for a test transport.
    pub fn new(status: u16, headers: Vec<(String, String)>, body: impl Into<Bytes>) -> Self {
        let body: Bytes = body.into();
        let headers = headers
            .into_iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value))
            .collect();
        Self {
            status,
            headers,
            body: futures::stream::once(async move { Ok(body) }).boxed(),
        }
    }

    /// The value of the header `name` (in any case), if present.
    pub fn header(&self, name: &str) -> Option<&str> {
        find_header(&self.headers, name)
    }

    /// Read the whole body.
    pub async fn bytes(self) -> Result<Bytes, TransportError> {
        let chunks: Vec<Bytes> = self.body.try_collect().await?;
        Ok(match chunks.len() {
            1 => chunks.into_iter().next().unwrap_or_default(),
            _ => chunks.concat().into(),
        })
    }
}

impl fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .finish_non_exhaustive()
    }
}

pub(crate) fn find_header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// A request that got no response.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    /// DNS, a refused or reset connection, TLS, or a body cut short.
    #[error("{0}")]
    Connection(String),
    /// The transport ran out of time on its own.
    #[error("the request timed out")]
    Timeout,
}

/// Sends one attempt of a request. Implement it to replace the HTTP layer.
pub trait HttpTransport: Send + Sync {
    /// Send `request` and give the response as soon as its headers arrive.
    fn send(&self, request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, TransportError>>;
}

/// The root certificates the default transport trusts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TlsRoots {
    /// The Mozilla roots bundled in the crate (`webpki-roots`). The default of the SDK.
    #[default]
    Bundled,
    /// The certificate store of the platform (`rustls-platform-verifier`). The CLI uses it,
    /// because corporate networks intercept TLS.
    Platform,
}

/// A `reqwest` client with rustls and ring (never OpenSSL), trusting `roots`.
///
/// The CLI may use it for its own requests, so they have the same TLS as the SDK.
pub fn reqwest_client(roots: TlsRoots) -> Result<reqwest::Client, Error> {
    reqwest_client_builder(roots)?
        .build()
        .map_err(|error| Error::Config(format!("the HTTP client could not be built: {error}")))
}

/// A `reqwest` client builder with the TLS of [`reqwest_client`], to add other settings.
///
/// The crate builds `reqwest` with no default crypto provider, so a client made with
/// `reqwest::Client::new()` or a plain `reqwest::Client::builder()` panics. Start from this
/// builder instead.
pub fn reqwest_client_builder(roots: TlsRoots) -> Result<reqwest::ClientBuilder, Error> {
    let tls_error = |error: rustls::Error| Error::Config(format!("the TLS setup failed: {error}"));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(tls_error)?;
    let config = match roots {
        TlsRoots::Bundled => {
            let store = rustls::RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            };
            builder.with_root_certificates(store).with_no_client_auth()
        }
        TlsRoots::Platform => {
            use rustls_platform_verifier::BuilderVerifierExt;
            builder
                .with_platform_verifier()
                .map_err(tls_error)?
                .with_no_client_auth()
        }
    };
    Ok(reqwest::Client::builder().tls_backend_preconfigured(config))
}

/// The default transport, on `reqwest`.
#[derive(Debug, Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    /// A transport with its own `reqwest` client, trusting `roots`.
    pub fn new(roots: TlsRoots) -> Result<Self, Error> {
        Ok(Self {
            client: reqwest_client(roots)?,
        })
    }

    /// A transport on a `reqwest` client of the caller, made from [`reqwest_client_builder`].
    pub fn from_client(client: reqwest::Client) -> Self {
        Self { client }
    }
}

fn transport_error(error: reqwest::Error) -> TransportError {
    if error.is_timeout() {
        return TransportError::Timeout;
    }
    // The whole chain, without the URL: the message says what failed, the caller knows where.
    let error = error.without_url();
    let mut message = error.to_string();
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    TransportError::Connection(message)
}

impl HttpTransport for ReqwestTransport {
    fn send(&self, request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, TransportError>> {
        Box::pin(async move {
            let method = match request.method {
                HttpMethod::Get => reqwest::Method::GET,
                HttpMethod::Post => reqwest::Method::POST,
                HttpMethod::Put => reqwest::Method::PUT,
                HttpMethod::Patch => reqwest::Method::PATCH,
                HttpMethod::Delete => reqwest::Method::DELETE,
            };
            let mut builder = self.client.request(method, &request.url);
            for (name, value) in &request.headers {
                builder = builder.header(name, value);
            }
            if let Some(body) = request.body {
                builder = builder.body(body);
            }
            let response = builder.send().await.map_err(transport_error)?;
            let status = response.status().as_u16();
            let headers = response
                .headers()
                .iter()
                .map(|(name, value)| {
                    (
                        name.as_str().to_owned(),
                        String::from_utf8_lossy(value.as_bytes()).into_owned(),
                    )
                })
                .collect();
            let body = response.bytes_stream().map_err(transport_error).boxed();
            Ok(HttpResponse {
                status,
                headers,
                body,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_response_reads_its_body_and_headers() {
        let response = HttpResponse::new(200, vec![("X-Request-Id".into(), "r1".into())], "hello");
        assert_eq!(response.header("x-request-id"), Some("r1"));
        assert_eq!(response.header("missing"), None);
        assert!(format!("{response:?}").contains("200"));
        assert_eq!(response.bytes().await.unwrap(), Bytes::from("hello"));

        let chunks =
            futures::stream::iter(vec![Ok(Bytes::from("a")), Ok(Bytes::from("b"))]).boxed();
        let response = HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: chunks,
        };
        assert_eq!(response.bytes().await.unwrap(), Bytes::from("ab"));

        let broken =
            futures::stream::iter(vec![Err(TransportError::Connection("reset".into()))]).boxed();
        let response = HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: broken,
        };
        assert_eq!(response.bytes().await.unwrap_err().to_string(), "reset");
    }

    #[test]
    fn a_request_finds_its_headers() {
        let request = HttpRequest {
            method: HttpMethod::Get,
            url: "https://x.test/".into(),
            headers: vec![("accept".into(), "application/json".into())],
            body: None,
        };
        assert_eq!(request.header("Accept"), Some("application/json"));
    }

    #[test]
    fn builds_both_tls_setups() {
        ReqwestTransport::new(TlsRoots::Bundled).unwrap();
        // The platform verifier needs a certificate store; a minimal container may have none.
        let _ = ReqwestTransport::new(TlsRoots::Platform);
        let transport = ReqwestTransport::from_client(reqwest_client(TlsRoots::Bundled).unwrap());
        assert!(format!("{transport:?}").contains("ReqwestTransport"));
        assert_eq!(TlsRoots::default(), TlsRoots::Bundled);
        assert_eq!(TransportError::Timeout.to_string(), "the request timed out");
    }

    #[tokio::test]
    async fn a_refused_connection_is_a_connection_error() {
        // Port 1 (tcpmux) has no listener. A port taken from a bound and dropped
        // listener is free again at once, and a mock server of a parallel test
        // can take it before this request is sent.
        let port = 1;
        let transport = ReqwestTransport::new(TlsRoots::Bundled).unwrap();
        let request = HttpRequest {
            method: HttpMethod::Get,
            url: format!("http://127.0.0.1:{port}/api/me/"),
            headers: Vec::new(),
            body: None,
        };
        let error = transport.send(request).await.unwrap_err();
        match error {
            TransportError::Connection(message) => {
                assert!(!message.contains("/api/me/"), "{message}")
            }
            TransportError::Timeout => panic!("not a timeout"),
        }
    }

    #[tokio::test]
    async fn a_reqwest_timeout_is_a_timeout() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        });
        let client = reqwest_client_builder(TlsRoots::Bundled)
            .unwrap()
            .timeout(std::time::Duration::from_millis(50))
            .build()
            .unwrap();
        let transport = ReqwestTransport::from_client(client);
        let request = HttpRequest {
            method: HttpMethod::Get,
            url: format!("http://127.0.0.1:{port}/"),
            headers: Vec::new(),
            body: None,
        };
        assert_eq!(
            transport.send(request).await.unwrap_err(),
            TransportError::Timeout
        );
        server.abort();
    }
}
