# The shared behaviour of the Oscar clients

Status: normative, version 0.1.0 of the clients (`clients/VERSION`).

This document states the rules that the TypeScript SDK, the Python SDK, the
Rust SDK and the `oscar` CLI all follow. A client that breaks a rule here has
a bug. The conformance cases in `clients/spec/conformance/` test these rules.
When a rule here and a client disagree, change the client,
or change this document first and then every client.

This document gives exact values where a general rule would leave a
choice open, so that the conformance cases can be exact. Each such value is marked **(value)**.

"Must" is a requirement. "May" is a permission. "The SDK" means each of the
three SDKs. "The client" means an SDK or the CLI.

## 1. Credentials

### 1.1 The order

A client finds its credential in this order. The first match wins.

1. **An explicit credential.** In code: the option `apiKey` (TypeScript),
   `api_key` (Python), `.api_key(...)` (Rust), or a token provider (a callback
   that returns an access token: `tokenProvider`, `token_provider`,
   `.token_provider(...)`). In the CLI: the global flag `--api-key <key>`.
2. **The environment variable `OSCAR_API_KEY`.**
3. **The stored login of the CLI, only when the caller asks for it**
   (section 1.3).
4. **Otherwise** the client raises `AuthenticationError` before it sends a
   request. The message says how to log in: set `OSCAR_API_KEY`, or run
   `oscar login` and use the CLI login option.

An empty string counts as not set, for the options and for the variables.

Both kinds of credential go in the header `Authorization: Bearer <value>`.
An API key starts with `osk_`. The client does not look at the value.

A token provider is called before each request, and each retry. It may cache
the token itself. When it fails, the client raises `AuthenticationError` with
its message and sends nothing.

### 1.2 In a browser

In a browser, the TypeScript SDK accepts a token provider only. It refuses
`apiKey` with an error at construction, because a browser must never hold an
organization API key. It does not read `OSCAR_API_KEY` and has no step 3.

### 1.3 The CLI login, step 3

A library never starts a process on its own. So a server with a missing
`OSCAR_API_KEY` fails, and never uses the login of a developer by accident.
Step 3 runs only when one of these is true:

- The caller sets the option `credential: 'cli'` (TypeScript),
  `credential="cli"` (Python) or `.credential(Credential::Cli)` (Rust).
- The environment variable `OSCAR_USE_CLI_LOGIN` is `1`.

The rules of step 3:

1. **Find the binary through `PATH` only.** Never look in the current
   directory, also not on Windows, where `CreateProcess` searches it first.
   Resolve the absolute path of `oscar` (on Windows `oscar.exe` or
   `oscar.cmd`, in the order of `PATHEXT`), and run that path. Never run it
   through a shell. When no `oscar` is on `PATH`, raise
   `AuthenticationError`: "the oscar CLI is not installed".
2. **Check the version once per process.** Run `oscar --version`. Its first
   line of standard output must match `^oscar-cli (\d+\.\d+\.\d+)(\S*)$`.
   Otherwise raise `AuthenticationError`: "`<path>` is not the oscar CLI".
3. **Ask for a token.** Run
   `oscar auth token --json --base-url <base URL of the SDK>`, and add
   `--profile <P>` when the caller set a profile (option `profile`, or
   `OSCAR_PROFILE`). The timeout is **30 seconds (value)**, because the CLI may
   refresh the token first. The environment of the child is the environment
   of the caller.
4. **Read the answer.** On exit code 0, standard output is one JSON object:

   ```json
   {
     "token": "<access token or API key>",
     "expires_at": "2026-09-27T10:00:00Z",
     "kind": "oauth",
     "base_url": "https://api.example.com",
     "profile": "default"
   }
   ```

   | Field | Type | Meaning |
   |---|---|---|
   | `token` | string | The value for `Authorization: Bearer` |
   | `expires_at` | string (RFC 3339, UTC) or `null` | When the token expires. `null` for an API key |
   | `kind` | `"oauth"` or `"api_key"` | How the profile logged in |
   | `base_url` | string | The base URL of the profile, normalized (section 3.1) |
   | `profile` | string | The name of the profile that answered |

   The client must check that `base_url` equals its own normalized base URL.
   Otherwise it raises `AuthenticationError` and never sends the token.
   A client ignores an unknown field.

5. **On a non-zero exit code**, raise `AuthenticationError` with the first
   line of standard error. Exit code 3 means "log in first" (section 4.7).
6. **Cache the token** in memory until 60 seconds before `expires_at`, then
   run step 3 again. A token with `expires_at: null` is cached for the life of
   the client. After a 401 from the API, drop the cache once and run step 3
   again before the error goes to the caller.
7. **Never log the token**, never put it in an error message, and never write
   it to disk.

The CLI side: `oscar auth token --json --base-url U` answers only from a
profile whose base URL equals `U`. So a production token never goes to
another host. With `--profile P`, the profile `P` must have that base URL,
else the CLI fails with exit code 3. Without `--profile`, the CLI uses the
default profile when its base URL is `U`, else the only profile with base URL
`U`, else it fails with exit code 3. It refreshes an OAuth token that expires
within 60 seconds before it answers.

## 2. The store of the CLI

Only the CLI writes the store. The SDKs never read it; they use step 3.

| Item | Where |
|---|---|
| Config directory | `OSCAR_CONFIG_DIR` when set, else `dirs::config_dir()/oscaraccountant`: `~/.config/oscaraccountant` on Linux, `~/Library/Application Support/oscaraccountant` on macOS, `%APPDATA%\oscaraccountant` on Windows |
| `config.json` | In the config directory. The profiles. No secret |
| Secrets | The OS keychain: service `oscaraccountant`, account `profile:<name>` |
| Fallback file | `credentials.json`, when no keychain service answers. In `OSCAR_CONFIG_DIR` when set, else `dirs::data_local_dir()/oscaraccountant` (`%LOCALAPPDATA%` on Windows, so a roaming profile never copies it) |
| Lock files | `<profile>.lock` (the name percent-encoded), next to `credentials.json`. Empty. The lock of a refresh across processes (section 4.5) |

### 2.1 `config.json`

```json
{
  "version": 1,
  "default_profile": "default",
  "profiles": {
    "default": {
      "base_url": "https://api.example.com",
      "auth": "oauth",
      "organization": { "id": "k3j9x2m1q0", "name": "Example OÜ" },
      "company": "p8d7c6b5a4",
      "companies": [
        { "id": "p8d7c6b5a4", "name": "Example OÜ", "registry_code": "12345678",
          "state": "ready", "organization_id": "k3j9x2m1q0" }
      ]
    }
  }
}
```

`auth` is `oauth` or `api_key`. `organization` and `company` may be `null`.
`companies` is a cache of the companies of `GET /api/me/` (section 4.8); it
may be missing.
The CLI writes the file atomically (a temporary file in the same directory,
then a rename) and keeps unknown keys. The directory is created with mode
`0700` on Unix.

### 2.2 The secrets

The keychain entry of a profile is one JSON string:

- OAuth: `{"refresh_token": "...", "access_token": "...", "expires_at": "<RFC 3339>"}`
- API key: `{"api_key": "osk_..."}`

The fallback file holds the same entries by profile name:
`{"version": 1, "profiles": {"<name>": <entry>}}`. A read takes the file
entry first, then the keychain: a copy in the file is always the newer one,
because a write that the keychain takes removes it. On Unix it has mode
`0600`, and the CLI refuses to read it when group or others may read it. On
Windows it gets a DACL that gives access to the owner only. The CLI says once,
on standard error, the first time it uses the file: "No keychain is
available; the credentials are in `<path>`."

"No keychain service answers" means: the keychain call fails with an error
that says there is no backend or no service (a server, CI, a container, a
Linux desktop with no Secret Service, which is common on Omarchy). Any other
keychain error is shown to the user, and the CLI does not fall back.

The environment variable `OSCAR_CREDENTIAL_STORE=file` makes the CLI use the
file only, and never ask the keychain (tests, and machines where the keychain
prompts). Unset, or `auto`, is the rule above. With the file forced, the
notice is: "The credentials are in `<path>` (OSCAR_CREDENTIAL_STORE=file)."

### 2.3 The build

- The `keyring` crate uses pure-Rust backends only: `apple-native`,
  `windows-native`, and `async-secret-service` with `crypto-rust` and `tokio`.
  So the Linux binary is a static musl build with no `libdbus`.
- No `openssl-sys` anywhere in the tree. `reqwest` uses rustls with `ring`.
- The CLI uses the certificate store of the platform
  (`rustls-tls-native-roots`), because corporate networks intercept TLS. The
  Rust SDK uses the bundled roots by default.

## 3. Requests

### 3.1 Base URL

The order, the first match wins: the option of the client (`baseUrl`,
`base_url`, `.base_url(...)`, the CLI flag `--base-url`), then
`OSCAR_BASE_URL`, then the profile (CLI only), then the default constant.

The default constant is defined in one place per client, named
`DEFAULT_BASE_URL`. **Its value is the production origin,
`https://oscaraccountant.com` (value).** The API is served under `/api/` and
OAuth under `/oauth/` of that origin, so the base URL has no path.

A client normalizes a base URL: it removes a trailing `/`, and it lower-cases
the scheme and the host. The scheme must be `https`, or `http` for a loopback
host (`localhost`, `127.0.0.1`, `[::1]`) or a host that ends in `.personal`
(the development overlay). Any other base URL is a usage error at
construction.

The API paths start with `/api/` and the OAuth paths with `/oauth/`, and both
are relative to the base URL. So the base URL is the origin of the frontend,
which serves both (the root `AGENTS.md`, "Development services").

### 3.2 The URL of a call

- **Path.** A generated method gives a template, for example
  `/api/companies/{company_id}/invoices/`, and a value for each name. The
  client percent-encodes each value (every byte except the RFC 3986
  unreserved characters `A-Z a-z 0-9 - . _ ~`) and puts it in place. A
  value that is empty, `.` or `..` is a `UsageError`, raised before the call
  sends anything: a URL parser removes or merges such a segment (also its
  encoded form `%2E%2E`), so the call would reach another path.
- **Query.** The client leaves out a parameter whose value is absent or null.
  It writes `true` and `false` for a boolean, the decimal form for a number,
  `YYYY-MM-DD` for a date, and the string value for an enum. Each name appears
  once. The order does not matter; the conformance cases compare the query as
  a sorted multiset.

### 3.3 Headers

| Header | When | Value |
|---|---|---|
| `Authorization` | Always | `Bearer <credential>` |
| `User-Agent` | Always, except in a browser | Section 3.9 |
| `Accept` | Always | `application/json`, and `*/*` for a download |
| `Content-Type` | A JSON body | `application/json` |
| `Content-Type` | A multipart body | `multipart/form-data; boundary=...` |
| `Idempotency-Key` | An operation that accepts it | Section 3.5 |

The caller may add headers for one call (option `headers`, `extra_headers`,
`.header(...)`). They cannot replace `Authorization` or `Idempotency-Key`.

A JSON body holds only the fields the caller set. In Python that is
`model_dump(mode='json', by_alias=True, exclude_unset=True)`. A field set to
null is sent as `null`.

### 3.4 Timeout and retries

| Rule | Value |
|---|---|
| Timeout | 60 seconds for each attempt, from the start of the connection to the end of the response body. Configurable per client and per call. A download stream has the timeout for its headers only |
| Retries | 2, so at most 3 attempts. Configurable per client and per call; 0 turns them off |
| Which requests | A `GET`, and a request that carries an `Idempotency-Key`. No other request is ever sent twice |
| When | A connection error (no response: DNS, refused, reset, TLS), a timeout **(value)**, and the statuses 408, 429, 500, 502, 503, 504, and 409 with the code `idempotency_in_progress` |
| Delay before retry `n` (1, 2, ...) | `min(8, 0.5 * 2^(n-1)) * (1 - 0.25 * jitter)` seconds, with `jitter` in `[0, 1)` **(value)** |
| `Retry-After` | When the response has it, it wins over the delay: whole seconds, or an HTTP date against the clock. A value above 60 seconds ends the retries, and the error goes to the caller **(value)** |

The clock (the current time), the sleep and the jitter source are injectable,
and so is the UUID source of section 3.5. The conformance cases inject all
four, so the delays and the keys are exact.

The client reads and buffers the whole body of an error response before it
decides on a retry. A retry of a multipart body sends the same bytes again;
a client that cannot replay the file content (a one-shot stream) sends the
request once only.

### 3.5 `Idempotency-Key`

- The client sends it on every operation that accepts it (the generated
  request spec says `idempotent: true`), and on no other operation.
- The caller may give a key for one call (option `idempotencyKey`,
  `idempotency_key`, `.idempotency_key(...)`). Else the client makes a
  UUID version 4, in lower case with hyphens, from the injectable source.
- The key is made once per call, and every retry of the call sends the same
  key.
- A response with `Idempotent-Replayed: true` is a normal success.

### 3.6 Pagination

A list operation returns `{"items": [...], "next_cursor": "..." | null}` and
takes the query parameters `cursor` and `limit` (at most 200).

- Each list method gives one page: `await` the result in TypeScript,
  `first_page()` in Python, `.send()` in Rust.
- Each list method also gives an iterator over every item of every page:
  `for await` in TypeScript, `for` and `async for` in Python, `.stream()` (a
  `Stream`) in Rust. The iterator sends the first request with the caller's
  parameters. Then it sends the same parameters with `cursor` set to the last
  `next_cursor`, until `next_cursor` is null. Each page is a separate `GET`
  with its own retries.
- The iterator fetches a page only when the caller needs its first item.
- The CLI flag `--all` walks every page the same way.

### 3.7 Errors

Every error of a client derives from one base: `OscarError` in TypeScript
and Python, the enum `Error` in Rust.

A response with a status of 400 or above becomes an `ApiError`. The body is
an RFC 9457 problem (`application/problem+json`):

| Field of `ApiError` | From |
|---|---|
| `status` | The HTTP status |
| `code` | The `code` of the body, stable and machine-readable (`validation_error`, `idempotency_conflict`, ...) |
| `title`, `detail` | The body |
| `errors` | The `errors` of the body (one entry per invalid field), else an empty list |
| `requestId` / `request_id` | The response header `X-Request-Id`, else null |

When the body is not a problem (not JSON, or no `code`), `code` is `unknown`,
`title` is the reason phrase of the status, and `detail` is the first 2000
characters of the body **(value)**.

The subtype by status:

| Status | Subtype |
|---|---|
| 400 | `BadRequestError` |
| 401 | `AuthenticationError` |
| 403 | `PermissionDeniedError` |
| 404 | `NotFoundError` |
| 409 | `ConflictError` |
| 422 | `UnprocessableEntityError` |
| 429 | `RateLimitError` |
| 500 and above | `InternalServerError` |
| Any other status of 400 and above | `ApiError` itself |

`AuthenticationError` is also the error of section 1, with no status.
`UsageError` is a call the client cannot send as given (section 3.2); it has
no status. It is `UsageError` in TypeScript and Python (a `ValueError` too)
and `Error::Config` in Rust.
Two errors have no response: `ConnectionError` (the request got no response)
and `TimeoutError` (an attempt ran out of time). Both are raised after the
last retry. In Python, they subclass `OscarError`, not the builtins of the
same name. In Rust, they are variants of `Error`.

The message of an `ApiError` is `<status> <code>: <detail>`, and ends with
` (request <id>)` when the request id is known. No message holds a
credential.

### 3.8 Downloads and uploads

- **Upload** (`files.upload`, `POST /api/companies/{company_id}/files/`): a
  `multipart/form-data` body with every field of `UploadRequest`: `file`,
  `kind`, `received_on`, `bank_transaction_id`. A field the caller did not
  set is not sent. The file part has a file name: the name the caller gave,
  else the name of a `File` or of a path, else `upload` **(value)**; and a
  content type: the one the caller gave, else `application/octet-stream`.
- **Download** (`files.download`,
  `GET /api/companies/{company_id}/files/{file_id}/content/`): the body is
  bytes, given as a stream (`ReadableStream` in TypeScript, an iterator of
  `bytes` in Python, a `Stream` of `Bytes` in Rust), with the content type,
  the length when known, and the file name of `Content-Disposition`. The DRF
  query parameter `format` of this endpoint is not exposed.

### 3.9 `User-Agent`

`oscaraccountant-<lang>/<version> (<os>; <arch>; <runtime>)`, and
`oscar-cli/<version> (<os>; <arch>; <runtime>)` for the CLI.

| Part | Values |
|---|---|
| `<lang>` | `node`, `python`, `rust` |
| `<version>` | `clients/VERSION` |
| `<os>` | `linux`, `macos`, `windows`, or `unknown` |
| `<arch>` | `x86_64`, `arm64`, or the name the runtime gives |
| `<runtime>` | `node/<version>`, `deno/<version>`, `bun/<version>`, `python/<version>`, `rust` |

Example: `oscaraccountant-python/0.1.0 (linux; x86_64; python/3.14.2)`. A
browser does not let a page set `User-Agent`, so the TypeScript SDK sends
none there.

## 4. The OAuth login of the CLI

The CLI is the public client `oscar-cli` of the authorization server. It asks for the API as its resource, so its tokens never work at
the MCP service.

### 4.1 Discovery

The CLI reads `<base URL>/.well-known/oauth-authorization-server`. It takes
`issuer`, `authorization_endpoint`, `token_endpoint`,
`device_authorization_endpoint` and `revocation_endpoint` from it. The
`issuer` must equal the normalized base URL; else the login stops with exit
code 3.

The resource is `<base URL>/api`, with no trailing `/`. The scope is
`read write` by default, and `read` with `--scope read`.

### 4.2 Loopback login (the default)

1. Listen on `127.0.0.1` on a free port that the OS chooses. Use `[::1]`
   only when IPv4 loopback is not available.
2. Make a PKCE verifier (32 random bytes, base64url without padding), its
   S256 challenge, and a `state` (32 random bytes, base64url).
3. Open the system browser at the authorization endpoint with
   `response_type=code`, `client_id=oscar-cli`,
   `redirect_uri=http://127.0.0.1:<port>/callback` (or
   `http://[::1]:<port>/callback`), `scope`, `state`, `code_challenge`,
   `code_challenge_method=S256`, `prompt=consent` and `resource`. The browser
   command is `$BROWSER` when set, else `open` on macOS, `start` on Windows,
   `xdg-open` on Linux. When the browser cannot be opened, print the URL and
   keep waiting.
4. Wait 5 minutes at most. Accept one request only: the first `GET
   /callback`. Answer it with a redirect (`302`) to `<base URL>/cli/done`,
   or to `<base URL>/cli/error?reason=<code>` when the callback is not good:
   `state_mismatch` (another `state`), `access_denied` (the user refused),
   `invalid_request` (another error of the request, another `iss`, or no
   `code`), `server_error` (any other `error`). The pages of the web app get
   no code and no token. Before the login, the CLI sends `GET <base
   URL>/cli/done` with no credential (3 seconds, no retry): when it does not
   answer 2xx (an older server), the CLI answers the callback with a plain
   page of its own instead (`Content-Type: text/html; charset=utf-8`). Both
   answers carry `Referrer-Policy: no-referrer` and `Cache-Control:
   no-store`. Then close the listener.
5. Check that `state` equals the one sent, and that `iss` equals the
   `issuer` (RFC 9207). An `error` parameter ends the login with its
   `error_description`.
6. Exchange the code at the token endpoint (a form `POST`) with
   `grant_type=authorization_code`, `code`, the exact `redirect_uri` of step
   3 (with the real port), `client_id=oscar-cli`, `code_verifier` and
   `resource`.
7. Call `GET /api/me/`, choose the default company (section 4.8), then
   store the tokens (section 2), the organization, the company and the
   cache of the companies.

The server never skips its consent page for a loopback redirect URI.

### 4.3 Device login

`oscar login --device`, and the default when no display is found: on Linux
when neither `DISPLAY` nor `WAYLAND_DISPLAY` is set, and on any system when
`SSH_CONNECTION` is set **(value)**.

1. `POST` the device authorization endpoint with `client_id=oscar-cli`,
   `scope` and `resource`.
2. Print the `user_code` and the `verification_uri` (and
   `verification_uri_complete` when present) on standard error. When
   standard error is a terminal, also print a QR code of
   `verification_uri_complete` (else `verification_uri`), for a login from
   a phone.
3. Poll the token endpoint with
   `grant_type=urn:ietf:params:oauth:grant-type:device_code`, `device_code`
   and `client_id=oscar-cli`, every `interval` seconds (5 when the server
   gives none). On `authorization_pending`, poll again. On `slow_down`, add 5
   seconds to the interval, for the rest of the login. On `access_denied`,
   `expired_token` or `invalid_grant`, stop with exit code 3.
4. Store the tokens as in section 4.2, step 7.

### 4.4 API key login

`oscar login --api-key` reads the key from a prompt that does not echo when
the CLI may ask (section 4.8), else from standard input when it is not a
terminal; a terminal with `--no-input` is exit code 2. The CLI checks it with
`GET /api/me/` and then stores it (`auth: "api_key"`).

Every login writes one profile: `--profile` (or `OSCAR_PROFILE`); else, with
`--base-url`, the profile of that base URL (section 1.3); else the default
profile, or `default`. When that last profile exists for another base URL,
the login stops with exit code 2 and asks for `--profile <name>`, so a login
to a second server never replaces the first one. The login reads
`config.json` again just before it saves it, so a change another command made
while the login waited is kept. The global flag
`--api-key <key>` is accepted for one call and is never stored.

### 4.5 Refresh

Before each call, when the access token expires within 60 seconds, the CLI
refreshes it: `grant_type=refresh_token`, `refresh_token`,
`client_id=oscar-cli`, `resource`. The server rotates the refresh token; the
CLI stores the new one at once, before it uses the new access token. Before
the refresh, the CLI takes an exclusive lock of the profile across processes
(its lock file, section 2) and reads the store again: when another process
refreshed in the meantime, the CLI uses that access token and does not
refresh. So a rotated refresh token is never sent twice. A
refresh that fails with `invalid_grant` (a reused or a dead refresh token)
means "log in again": exit code 3.

### 4.6 Logout

`oscar logout [--profile P]` revokes the refresh token at the revocation
endpoint (`token`, `token_type_hint=refresh_token`, `client_id=oscar-cli`).
Then it deletes the secrets and the profile entry, also when the revocation
fails; in that case it says so. When the keychain does not answer, the CLI
still deletes the file copy and the profile entry, and warns that a copy may
remain in the keychain; the exit code stays 0. An API key profile has nothing to revoke:
the key is deleted locally and stays valid at the server.

### 4.7 Exit codes of the CLI

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | An error of the API |
| 2 | A usage error |
| 3 | Authentication needed |
| 4 | A network error |
| 130 | Stopped with Ctrl-C while the CLI waits or asks |

### 4.8 Companies and prompts of the CLI

The default company of a login, from the companies of `GET /api/me/` (of
the organization of the credential): `-c/--company <x>` when given; else the
only company; else, on a terminal, a picker (name, registry code, state);
else the previous default of the profile when it is still listed; else none,
and the CLI says how to choose one (`oscar companies use <x>`). With no
company, the CLI offers `oscar onboard`.

`<x>` of `-c/--company`, `OSCAR_COMPANY` and `oscar companies use` is the id,
the registry code, or a unique, case-insensitive prefix of the name; among
several names with the prefix, the name equal to `<x>` wins. An ambiguous
value is exit code 2, with one line per match. The cache of the profile
answers first; a value it does not know reads `GET /api/me/` again and stores
the fresh list. The default company of the profile is used as stored.

The CLI asks only when standard input and standard error are terminals, and
neither `--no-input` nor `OSCAR_NO_INPUT` (any value but `0` or `false`) is
given. Otherwise it never waits: it takes the default of a question, and a
value with no default is exit code 2 with a message that names the flag.

## 5. The generated code

The resources, models and the CLI command table are generated from
the OpenAPI document of the public API by `clients/codegen` (`uv run oscar-codegen`).
The names come from `clients/codegen/names.yaml`. The contract between the
generated resources and the hand-written runtime of each language is in
`clients/codegen/README.md`. The generated code builds a request spec
(operation id, method, path template, path values, query, body,
`idempotent`, response kind); the runtime applies every rule of this
document to it.
