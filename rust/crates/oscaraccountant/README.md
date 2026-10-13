# oscaraccountant

The Rust SDK of the Oscar accounting API.

- Async, on `tokio` and `reqwest` with rustls. No OpenSSL.
- One request builder per operation, generated from the OpenAPI document of
  the public API.
- Credentials, retries, idempotency keys, pagination, errors and files work
  the same way as in the TypeScript and Python SDKs and the `oscar` CLI.

Rust 1.87 or later.

## Install

```toml
[dependencies]
oscaraccountant = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
futures = "0.3"
```

## Quick start

```rust
use futures::TryStreamExt;
use oscaraccountant::Client;

#[tokio::main]
async fn main() -> Result<(), oscaraccountant::Error> {
    let oscar = Client::from_env()?; // reads OSCAR_API_KEY and OSCAR_BASE_URL

    let me = oscar.me().retrieve().send().await?;
    let company_id = &me.companies[0].id;

    // One page.
    let page = oscar.invoices().list(company_id).state("issued").limit(50).send().await?;
    println!("{} invoices on the first page", page.items.len());

    // Every invoice of every page.
    let mut invoices = oscar.invoices().list(company_id).state("issued").stream();
    while let Some(invoice) = invoices.try_next().await? {
        println!("{} {}", invoice.id, invoice.state);
    }
    Ok(())
}
```

A required parameter is an argument of the method. An optional parameter is
a setter on the request builder. Then call `.send().await`, or `.stream()`
for a list.

## Authentication

The client takes the first credential it finds, in this order:

1. The credential you give in code:
   `Client::builder().api_key("osk_...")`, or a token provider (below).
2. The environment variable `OSCAR_API_KEY`.
3. The stored login of the `oscar` CLI, **only when you ask for it**:
   `.credential(Credential::Cli)`, or `OSCAR_USE_CLI_LOGIN=1`.

When there is no credential, every call fails with
`Error::Authentication` before it sends anything. Both kinds of credential
go in `Authorization: Bearer ...`.

### A token provider

A token provider is called before each attempt of each request. It may cache
its token.

```rust
use oscaraccountant::{BoxError, Client};

let oscar = Client::builder()
    .token_provider(|| async { Ok::<_, BoxError>(fetch_access_token().await?) })
    .build()?;
```

### The login of the `oscar` CLI

For a script on your own machine, log in once with `oscar login`, then:

```rust
use oscaraccountant::{Client, Credential};

let oscar = Client::builder().credential(Credential::Cli).build()?;
```

The SDK finds `oscar` on `PATH` only (never in the current directory), runs
it with no shell, and checks `oscar --version`. It then runs
`oscar auth token --json --base-url <base URL>` (with `--profile` when you
set `.profile(...)` or `OSCAR_PROFILE`). It uses the token only when the CLI
answers for the same base URL. It keeps the token in memory until 60 seconds
before it expires, and never logs it. After a 401, it asks the CLI again.

A server must use `OSCAR_API_KEY`. The SDK never starts a process unless you
ask for the CLI login.

## Configuration

| Builder method | Default |
|---|---|
| `.base_url(url)` | `OSCAR_BASE_URL`, else `DEFAULT_BASE_URL` (`https://oscaraccountant.com`) |
| `.timeout(duration)` | 60 seconds for each attempt |
| `.max_retries(n)` | 2 (so at most 3 attempts); 0 turns retries off |
| `.tls_roots(TlsRoots::Platform)` | `TlsRoots::Bundled` (the Mozilla roots in the crate) |
| `.user_agent(value)` | `oscaraccountant-rust/<version> (<os>; <arch>; rust)` |
| `.profile(name)` | `OSCAR_PROFILE`, for the CLI login |

The base URL must use `https`. `http` is allowed only for `localhost`,
`127.0.0.1`, `[::1]` and hosts that end in `.personal`.

Each request builder also has `.timeout(...)`, `.max_retries(...)` and
`.header(name, value)` for one call. An extra header cannot replace
`Authorization` or `Idempotency-Key`.

## Errors

Every error is an `oscaraccountant::Error`:

| Variant | When |
|---|---|
| `BadRequest` | 400 |
| `Authentication` | 401, or no usable credential |
| `PermissionDenied` | 403 |
| `NotFound` | 404 |
| `Conflict` | 409 |
| `UnprocessableEntity` | 422 |
| `RateLimit` | 429 |
| `InternalServer` | 500 and above |
| `Api` | any other status of 400 and above |
| `Connection` | no response (DNS, a refused connection, TLS), after the last retry |
| `Timeout` | an attempt ran out of time, after the last retry |
| `Config` | a wrong base URL or argument |
| `Json` | a body that could not be encoded or decoded |

An API error holds an `ApiError`: `status`, `code` (stable, for example
`validation_error`), `title`, `detail`, `errors` (one entry per invalid
field), `request_id` (the `X-Request-Id` header) and the whole `body`.

```rust
match oscar.invoices().retrieve(company_id, invoice_id).send().await {
    Ok(invoice) => println!("{}", invoice.id),
    Err(error) if error.status() == Some(404) => println!("no such invoice"),
    Err(error) => return Err(error),
}
```

## Retries and idempotency

The client retries a `GET`, and a request that carries an `Idempotency-Key`.
It retries on a connection error, a timeout, and the statuses 408, 429, 500,
502, 503, 504, and 409 with the code `idempotency_in_progress`. It waits
`min(8, 0.5 * 2^(n-1))` seconds, shortened by up to 25 percent of jitter,
before retry `n`. A `Retry-After` header wins; a value above 60 seconds ends
the retries.

Every operation that accepts an `Idempotency-Key` gets one automatically (a
random UUID). Every retry of the call sends the same key. To choose the key,
call `.idempotency_key("...")` on the request builder.

## Files

```rust
use oscaraccountant::Upload;

let upload = Upload::from_path("invoice.pdf").await?.content_type("application/pdf");
let document = oscar.files().upload(company_id, upload).kind("invoice").send().await?;

let download = oscar.files().download(company_id, &document.id).send().await?;
println!("{:?} {:?}", download.file_name(), download.content_type());
let bytes = download.bytes().await?; // or read it as a Stream of chunks
```

`Upload::new(bytes)` takes bytes from memory. The file name is `upload`
and the type `application/octet-stream` unless you set them.

## A raw call

For an endpoint that this version does not have yet:

```rust
use oscaraccountant::HttpMethod;

let response = oscar.request(HttpMethod::Get, "/api/me/").query("expand", "companies").send().await?;
let value: serde_json::Value = response.json()?;
```

A raw call has the same credential, headers, timeout, retries and errors.
`.json(&value)?` or `.body(content_type, bytes)` sets a body, and
`.idempotent()` adds an `Idempotency-Key`.

## Tests of your code

The HTTP layer and the sources of time and randomness can be replaced:

| Builder method | Replaces |
|---|---|
| `.transport(impl HttpTransport)` | The HTTP layer (`ReqwestTransport`) |
| `.env(|name| ...)` | The process environment |
| `.clock(|| SystemTime)` | The clock of `Retry-After` dates and token expiry |
| `.sleep(|duration| future)` | The wait before a retry |
| `.jitter(|| f64)` | The jitter of the backoff, in `[0, 1)` |
| `.uuid_source(|| String)` | The automatic `Idempotency-Key` |

`HttpTransport` has one method: `send(HttpRequest) -> HttpResponse`.
`HttpResponse::new(status, headers, body)` makes a response in a test.

## Documentation

The guides and the reference of the API are at
<https://oscaraccountant.com/docs>.

## Versions

The version is the one of every Oscar client. The changes are in
`CHANGELOG.md`.

## License

MIT. Copyright (c) 2026 OscarAI OÜ.
