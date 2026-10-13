//! The client: its construction, and the sending of each call with the rules of
//! `clients/spec/README.md` (headers, timeouts, retries, `Idempotency-Key`, errors).

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use futures::future::BoxFuture;
use futures::stream::{StreamExt, TryStreamExt};
use serde::de::DeserializeOwned;

use crate::credentials::{Auth, Credential, TokenProvider};
use crate::files::{Download, disposition_file_name, encode_multipart};
use crate::hooks::Hooks;
use crate::pagination::PageStream;
use crate::retry::{RetryAfter, backoff, is_retriable_status, parse_retry_after};
use crate::transport::{
    HttpRequest, HttpResponse, HttpTransport, ReqwestTransport, TlsRoots, TransportError,
    find_header,
};
use crate::urls::{DEFAULT_BASE_URL, build_url, normalize_base_url, render_path};
use crate::user_agent::default_user_agent;
use crate::{
    ApiError, Error, HttpMethod, Page, RequestBody, RequestOptions, RequestSpec, ResponseKind,
};

/// The timeout of each attempt, unless configured.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
/// The number of retries after the first attempt, unless configured.
pub const DEFAULT_MAX_RETRIES: u32 = 2;

/// The headers a caller cannot replace.
const PROTECTED_HEADERS: [&str; 2] = ["authorization", "idempotency-key"];

/// The client of the Oscar API. Cheap to clone: the clones share one configuration and one
/// connection pool.
///
/// ```no_run
/// # async fn run() -> Result<(), oscaraccountant::Error> {
/// let oscar = oscaraccountant::Client::from_env()?;
/// let me = oscar.me().retrieve().send().await?;
/// # Ok(()) }
/// ```
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

struct Inner {
    base_url: String,
    auth: Auth,
    timeout: Duration,
    max_retries: u32,
    user_agent: String,
    transport: Arc<dyn HttpTransport>,
    hooks: Hooks,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base_url", &self.inner.base_url)
            .field("credential", &self.inner.auth.kind())
            .field("timeout", &self.inner.timeout)
            .field("max_retries", &self.inner.max_retries)
            .finish_non_exhaustive()
    }
}

/// The configuration of a [`Client`]. Every option is optional.
#[must_use = "a builder does nothing until `build` is called"]
pub struct ClientBuilder {
    credential: Option<Credential>,
    profile: Option<String>,
    base_url: Option<String>,
    timeout: Duration,
    max_retries: u32,
    user_agent: Option<String>,
    transport: Option<Arc<dyn HttpTransport>>,
    tls_roots: TlsRoots,
    hooks: Hooks,
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientBuilder")
            .field("credential", &self.credential)
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self {
            credential: None,
            profile: None,
            base_url: None,
            timeout: DEFAULT_TIMEOUT,
            max_retries: DEFAULT_MAX_RETRIES,
            user_agent: None,
            transport: None,
            tls_roots: TlsRoots::default(),
            hooks: Hooks::default(),
        }
    }
}

impl ClientBuilder {
    /// An organization API key. It wins over `OSCAR_API_KEY`.
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.credential = Some(Credential::ApiKey(key.into()));
        self
    }

    /// A callback that gives an access token before each attempt. It wins over `OSCAR_API_KEY`.
    pub fn token_provider(mut self, provider: impl TokenProvider + 'static) -> Self {
        self.credential = Some(Credential::TokenProvider(Arc::new(provider)));
        self
    }

    /// The credential, in one of its forms. [`Credential::Cli`] asks for the stored login of the
    /// `oscar` CLI when there is no API key.
    pub fn credential(mut self, credential: Credential) -> Self {
        self.credential = Some(credential);
        self
    }

    /// The profile of the CLI login. Default `OSCAR_PROFILE`, else the CLI chooses.
    pub fn profile(mut self, profile: impl Into<String>) -> Self {
        self.profile = Some(profile.into());
        self
    }

    /// The base URL. Default `OSCAR_BASE_URL`, else [`DEFAULT_BASE_URL`].
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    /// The timeout of each attempt. Default 60 seconds.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The number of retries after the first attempt. Default 2; 0 turns them off.
    pub fn max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// The whole `User-Agent` header. The CLI sets `oscar-cli/<version> (...)` with
    /// [`user_agent`](crate::user_agent).
    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = Some(user_agent.into());
        self
    }

    /// The HTTP layer. Default a [`ReqwestTransport`] with the [`TlsRoots`] of
    /// [`tls_roots`](Self::tls_roots).
    pub fn transport(mut self, transport: impl HttpTransport + 'static) -> Self {
        self.transport = Some(Arc::new(transport));
        self
    }

    /// The root certificates of the default transport. Default [`TlsRoots::Bundled`].
    pub fn tls_roots(mut self, roots: TlsRoots) -> Self {
        self.tls_roots = roots;
        self
    }

    /// Where the client reads its environment variables. Default the process environment.
    pub fn env(mut self, env: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> Self {
        self.hooks.env = Arc::new(env);
        self
    }

    /// The clock of `Retry-After` dates and of the expiry of a CLI token. Default the system
    /// clock.
    pub fn clock(mut self, clock: impl Fn() -> SystemTime + Send + Sync + 'static) -> Self {
        self.hooks.clock = Arc::new(clock);
        self
    }

    /// The wait before a retry. Default `tokio::time::sleep`.
    pub fn sleep(
        mut self,
        sleep: impl Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync + 'static,
    ) -> Self {
        self.hooks.sleep = Arc::new(sleep);
        self
    }

    /// The jitter of the backoff, a number in `[0, 1)`. Default random.
    pub fn jitter(mut self, jitter: impl Fn() -> f64 + Send + Sync + 'static) -> Self {
        self.hooks.jitter = Arc::new(jitter);
        self
    }

    /// The source of the automatic `Idempotency-Key`. Default a random UUID version 4.
    pub fn uuid_source(mut self, uuid: impl Fn() -> String + Send + Sync + 'static) -> Self {
        self.hooks.uuid = Arc::new(uuid);
        self
    }

    /// The client. It fails with [`Error::Config`] on a wrong base URL. A missing credential is
    /// not an error here: each call fails with [`Error::Authentication`] before it sends
    /// anything.
    pub fn build(self) -> Result<Client, Error> {
        let base_url = match self.base_url.filter(|value| !value.is_empty()) {
            Some(value) => value,
            None => self
                .hooks
                .var("OSCAR_BASE_URL")
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned()),
        };
        let base_url = normalize_base_url(&base_url)?;
        let auth = Auth::resolve(self.credential, &base_url, self.profile, &self.hooks);
        let transport: Arc<dyn HttpTransport> = match self.transport {
            Some(transport) => transport,
            None => Arc::new(ReqwestTransport::new(self.tls_roots)?),
        };
        Ok(Client {
            inner: Arc::new(Inner {
                base_url,
                auth,
                timeout: self.timeout,
                max_retries: self.max_retries,
                user_agent: self.user_agent.unwrap_or_else(default_user_agent),
                transport,
                hooks: self.hooks,
            }),
        })
    }
}

/// A body ready to send.
#[derive(Debug, Clone)]
pub(crate) struct PreparedBody {
    pub(crate) content_type: String,
    pub(crate) bytes: Bytes,
}

/// A call with its path encoded and its body serialized: what every attempt sends.
#[derive(Debug, Clone)]
pub(crate) struct Prepared {
    pub(crate) method: HttpMethod,
    /// The encoded path, from `/`. It may hold a query of its own.
    pub(crate) path: String,
    pub(crate) query: Vec<(String, String)>,
    pub(crate) body: Option<PreparedBody>,
    pub(crate) idempotency_key: Option<String>,
    pub(crate) accept: &'static str,
    pub(crate) options: RequestOptions,
    /// Send `Authorization`. Only a raw call of the CLI turns it off.
    pub(crate) authenticated: bool,
}

/// A response whose headers arrived: a body read whole, or a download stream.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: AnswerBody,
}

pub(crate) enum AnswerBody {
    Whole(Bytes),
    Stream(HttpResponse),
}

impl Answer {
    pub(crate) fn into_bytes(self) -> Bytes {
        match self.body {
            AnswerBody::Whole(bytes) => bytes,
            // Only a download keeps its stream, and a download never asks for the whole body.
            AnswerBody::Stream(_) => Bytes::new(),
        }
    }
}

/// What one attempt gave.
enum Attempt {
    Answer(Answer),
    Failed(Error, Option<RetryAfter>),
}

impl Client {
    /// A builder, with the process environment.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// A client configured by the environment only: `OSCAR_API_KEY`, `OSCAR_BASE_URL`,
    /// `OSCAR_USE_CLI_LOGIN` and `OSCAR_PROFILE`.
    pub fn from_env() -> Result<Self, Error> {
        Self::builder().build()
    }

    /// The normalized base URL of the client.
    pub fn base_url(&self) -> &str {
        &self.inner.base_url
    }

    /// A new `Idempotency-Key`, from the UUID source.
    pub(crate) fn new_idempotency_key(&self) -> String {
        (self.inner.hooks.uuid)()
    }

    /// Turn a generated request spec into what every attempt sends.
    fn prepare(&self, spec: RequestSpec) -> Result<Prepared, Error> {
        let path = render_path(spec.path, &spec.path_params)?;
        let query = spec
            .query
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect();
        let body = match spec.body {
            None => None,
            Some(RequestBody::Json(value)) => Some(PreparedBody {
                content_type: "application/json".to_owned(),
                bytes: serde_json::to_vec(&value)?.into(),
            }),
            Some(RequestBody::Multipart(fields)) => {
                let boundary = format!("oscar-{}", uuid::Uuid::new_v4().simple());
                let (content_type, bytes) = encode_multipart(&fields, &boundary);
                Some(PreparedBody {
                    content_type,
                    bytes,
                })
            }
        };
        let idempotency_key = match (spec.idempotent, spec.idempotency_key) {
            (false, _) => None,
            (true, Some(key)) if !key.is_empty() => Some(key),
            (true, _) => Some(self.new_idempotency_key()),
        };
        let accept = if spec.response == ResponseKind::Binary {
            "*/*"
        } else {
            "application/json"
        };
        Ok(Prepared {
            method: spec.method,
            path,
            query,
            body,
            idempotency_key,
            accept,
            options: spec.options,
            authenticated: true,
        })
    }

    /// Send a call and parse its JSON answer.
    pub(crate) async fn execute<T: DeserializeOwned>(&self, spec: RequestSpec) -> Result<T, Error> {
        let prepared = self.prepare(spec)?;
        let body = self.send(prepared, false).await?.into_bytes();
        Ok(serde_json::from_slice(&body)?)
    }

    /// Send a call whose answer has no body.
    // The contract of the generated code: `public.yaml` has no such operation today.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn execute_empty(&self, spec: RequestSpec) -> Result<(), Error> {
        let prepared = self.prepare(spec)?;
        self.send(prepared, false).await?;
        Ok(())
    }

    /// Send a call whose answer is bytes, and give them as a stream.
    pub(crate) async fn download(&self, spec: RequestSpec) -> Result<Download, Error> {
        let prepared = self.prepare(spec)?;
        let answer = self.send(prepared, true).await?;
        let header = |name: &str| find_header(&answer.headers, name).map(str::to_owned);
        let content_type = header("content-type");
        let content_length = header("content-length").and_then(|value| value.trim().parse().ok());
        let file_name =
            header("content-disposition").and_then(|value| disposition_file_name(&value));
        let stream = match answer.body {
            AnswerBody::Stream(response) => response.body.map_err(connection_error).boxed(),
            AnswerBody::Whole(bytes) => futures::stream::once(async move { Ok(bytes) }).boxed(),
        };
        Ok(Download::new(
            content_type,
            content_length,
            file_name,
            stream,
        ))
    }

    /// Every item of every page of a list call, fetched as the caller needs them.
    pub(crate) fn paginate<P>(&self, spec: RequestSpec) -> PageStream<P>
    where
        P: Page + Send + 'static,
        P::Item: Send + 'static,
    {
        PageStream::new(self.clone(), self.prepare(spec))
    }

    /// Send a prepared call: the attempts, with their retries, until an answer or an error.
    ///
    /// With `stream`, a success keeps its body as a stream, and the timeout covers the headers
    /// only. Every other answer is read whole within the timeout of its attempt.
    pub(crate) async fn send(&self, prepared: Prepared, stream: bool) -> Result<Answer, Error> {
        let inner = &self.inner;
        let url = build_url(&inner.base_url, &prepared.path, &prepared.query);
        let max_retries = prepared.options.max_retries.unwrap_or(inner.max_retries);
        let timeout = prepared.options.timeout.unwrap_or(inner.timeout);
        let retriable = prepared.method == HttpMethod::Get || prepared.idempotency_key.is_some();
        let mut retries = 0;
        let mut reauthenticated = false;
        loop {
            let request = self.http_request(&prepared, &url).await?;
            let (error, retry_after) = match self.attempt(request, timeout, stream).await {
                Attempt::Answer(answer) => return Ok(answer),
                Attempt::Failed(error, retry_after) => (error, retry_after),
            };
            // A 401 with the CLI login: the cached token may be stale. Drop it, and send the call
            // again once with a new token when the call may be sent twice.
            if error.status() == Some(401) {
                if let Auth::Cli(login) = &inner.auth {
                    login.invalidate().await;
                    if retriable && !reauthenticated {
                        reauthenticated = true;
                        continue;
                    }
                }
            }
            let may_retry = match &error {
                Error::Connection(_) | Error::Timeout(_) => true,
                error => error
                    .api_error()
                    .is_some_and(|api| is_retriable_status(api.status, &api.code)),
            };
            if !retriable
                || !may_retry
                || retries >= max_retries
                || retry_after == Some(RetryAfter::TooLong)
            {
                return Err(error);
            }
            retries += 1;
            let delay = match retry_after {
                Some(RetryAfter::Wait(delay)) => delay,
                _ => backoff(retries, (inner.hooks.jitter)()),
            };
            (inner.hooks.sleep)(delay).await;
        }
    }

    /// The request of one attempt, with every header.
    async fn http_request(&self, prepared: &Prepared, url: &str) -> Result<HttpRequest, Error> {
        let inner = &self.inner;
        let mut headers: Vec<(String, String)> =
            Vec::with_capacity(6 + prepared.options.headers.len());
        if prepared.authenticated {
            // The token provider is called before each attempt.
            let token = inner.auth.token().await?;
            headers.push(("authorization".to_owned(), format!("Bearer {token}")));
        }
        headers.push(("user-agent".to_owned(), inner.user_agent.clone()));
        headers.push(("accept".to_owned(), prepared.accept.to_owned()));
        if let Some(body) = &prepared.body {
            headers.push(("content-type".to_owned(), body.content_type.clone()));
        }
        if let Some(key) = &prepared.idempotency_key {
            headers.push(("idempotency-key".to_owned(), key.clone()));
        }
        for (name, value) in &prepared.options.headers {
            let name = name.to_ascii_lowercase();
            if PROTECTED_HEADERS.contains(&name.as_str()) {
                continue;
            }
            headers.retain(|(existing, _)| *existing != name);
            headers.push((name, value.clone()));
        }
        Ok(HttpRequest {
            method: prepared.method,
            url: url.to_owned(),
            headers,
            body: prepared.body.as_ref().map(|body| body.bytes.clone()),
        })
    }

    /// One attempt within `timeout`.
    async fn attempt(&self, request: HttpRequest, timeout: Duration, stream: bool) -> Attempt {
        let transport = self.inner.transport.clone();
        let exchange = async move {
            let response = transport.send(request).await?;
            let status = response.status;
            let headers = response.headers.clone();
            let body = if stream && status < 400 {
                AnswerBody::Stream(response)
            } else {
                AnswerBody::Whole(response.bytes().await?)
            };
            Ok::<_, TransportError>(Answer {
                status,
                headers,
                body,
            })
        };
        let answer = match tokio::time::timeout(timeout, exchange).await {
            Err(_) | Ok(Err(TransportError::Timeout)) => {
                return Attempt::Failed(Error::Timeout(timeout), None);
            }
            Ok(Err(TransportError::Connection(message))) => {
                return Attempt::Failed(Error::Connection(message), None);
            }
            Ok(Ok(answer)) => answer,
        };
        if answer.status < 400 {
            return Attempt::Answer(answer);
        }
        let retry_after = find_header(&answer.headers, "retry-after")
            .and_then(|value| parse_retry_after(value, (self.inner.hooks.clock)()));
        let request_id = find_header(&answer.headers, "x-request-id").map(str::to_owned);
        let status = answer.status;
        let api = ApiError::from_response(status, request_id, &answer.into_bytes());
        Attempt::Failed(Error::from_api(api), retry_after)
    }
}

fn connection_error(error: TransportError) -> Error {
    match error {
        TransportError::Connection(message) => Error::Connection(message),
        TransportError::Timeout => Error::Connection("the download timed out".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use futures::StreamExt;

    use super::*;
    use crate::PaginatedOrganizationList;

    /// Answers every request with `status` and `body`, and records the requests.
    #[derive(Default)]
    struct Fixed {
        status: u16,
        body: &'static str,
        requests: Mutex<Vec<HttpRequest>>,
    }

    impl HttpTransport for Arc<Fixed> {
        fn send(
            &self,
            request: HttpRequest,
        ) -> BoxFuture<'_, Result<HttpResponse, TransportError>> {
            self.requests.lock().unwrap().push(request);
            let response = HttpResponse::new(self.status, Vec::new(), self.body);
            Box::pin(async move { Ok(response) })
        }
    }

    fn client(fixed: &Arc<Fixed>) -> Client {
        Client::builder()
            .env(|_| None)
            .api_key("osk_test")
            .base_url("https://api.example.com")
            .transport(fixed.clone())
            .build()
            .unwrap()
    }

    fn spec(path: &'static str, response: ResponseKind) -> RequestSpec {
        RequestSpec {
            operation_id: "test",
            method: HttpMethod::Delete,
            path,
            path_params: vec![("thing_id", "t1".to_owned())],
            query: Vec::new(),
            body: None,
            idempotent: false,
            idempotency_key: None,
            response,
            options: RequestOptions::default(),
        }
    }

    #[tokio::test]
    async fn an_empty_answer() {
        let fixed = Arc::new(Fixed {
            status: 204,
            ..Fixed::default()
        });
        client(&fixed)
            .execute_empty(spec("/api/things/{thing_id}/", ResponseKind::Empty))
            .await
            .unwrap();
        let requests = fixed.requests.lock().unwrap();
        assert_eq!(requests[0].method, HttpMethod::Delete);
        assert_eq!(requests[0].url, "https://api.example.com/api/things/t1/");
    }

    #[tokio::test]
    async fn a_spec_that_cannot_be_prepared_sends_nothing() {
        let fixed = Arc::new(Fixed {
            status: 200,
            body: "{}",
            ..Fixed::default()
        });
        let client = client(&fixed);
        let error = client
            .execute_empty(spec("/api/{missing}/", ResponseKind::Empty))
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Config(_)));
        let error = client
            .execute::<serde_json::Value>(spec("/api/{missing}/", ResponseKind::Json))
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Config(_)));
        let error = client
            .download(spec("/api/{missing}/", ResponseKind::Binary))
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Config(_)));
        let items: Vec<_> = client
            .paginate::<PaginatedOrganizationList>(spec("/api/{missing}/", ResponseKind::Json))
            .collect()
            .await;
        assert!(matches!(items[..], [Err(Error::Config(_))]));
        let pages: Vec<_> = client
            .paginate::<PaginatedOrganizationList>(spec("/api/{missing}/", ResponseKind::Json))
            .pages()
            .collect()
            .await;
        assert!(matches!(pages[..], [Err(Error::Config(_))]));
        assert!(fixed.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn a_stream_answer_has_no_whole_body() {
        let answer = Answer {
            status: 200,
            headers: Vec::new(),
            body: AnswerBody::Stream(HttpResponse::new(200, Vec::new(), "x")),
        };
        assert!(answer.into_bytes().is_empty());
        assert!(matches!(
            connection_error(TransportError::Timeout),
            Error::Connection(message) if message == "the download timed out"
        ));
        assert!(matches!(
            connection_error(TransportError::Connection("cut".into())),
            Error::Connection(_)
        ));
    }
}
