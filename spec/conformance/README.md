# The conformance cases

Status: normative, with `clients/spec/README.md`. Plan section 8.

A conformance case is one call of a client, the HTTP exchanges the call must
make, and the result or the error it must give. Each SDK has a runner that
plays every case with its own HTTP mock. So the three SDKs obey the rules of
`clients/spec/README.md` in the same way, and a new rule gets one case, not
three hand-written tests.

| Path | What it is |
|---|---|
| `case.schema.json` | The JSON Schema (Draft 2020-12) of a case file |
| `cases/<topic>.json` | The cases of one topic. The id of each case starts with `<topic>.` |
| `clients/mock-server/tests/test_conformance_cases.py` | The validator of the cases (see "Checking the cases") |

| File | Tests |
|---|---|
| `operations.json` | Every public operation once: the method, the path, the query, the body and the result |
| `pagination.json` | One page, and the iterator over every page (section 3.6) |
| `errors.json` | Each status mapped to its error type, with the parsed problem (section 3.7) |
| `retries.json` | The retried statuses, the backoff, `Retry-After`, the limits (section 3.4) |
| `idempotency.json` | The automatic key, the caller key, the key on retries (section 3.5) |
| `auth.json` | The credential order (section 1.1) |
| `requests.json` | The base URL, the path and query encoding, the headers, the body (sections 3.1 to 3.3, 3.9) |
| `files.json` | Upload and download (section 3.8) |
| `timeouts.json` | The timeout of an attempt, and a timeout as a retry reason (section 3.4) |

## The runners

| SDK | HTTP mock | Where |
|---|---|---|
| TypeScript | A custom `fetch` given to the client | `clients/typescript` |
| Python | `respx` | `clients/python` |
| Rust | `wiremock` | `clients/rust/crates/oscaraccountant` |

The CLI uses the Rust SDK, so the Rust runner covers its requests. The CLI
itself is tested against `clients/mock-server` over real HTTP.

A runner reads every file of `cases/`, in the order of the file names, and
plays every case in the order of the file. It reports a failure by the case
id. A case that names the runner in `skip` is reported as skipped, with the
reason.

## A case file

```json
{
  "$schema": "../case.schema.json",
  "title": "Retries",
  "description": "...",
  "cases": [ { "id": "retries.jitter", ... } ]
}
```

### A case

| Field | Required | Meaning |
|---|---|---|
| `id` | yes | Unique across every file: `<topic>.<name>` |
| `description` | yes | The rule the case tests, in one or two sentences |
| `spec` | yes | The sections of `clients/spec/README.md` the case tests |
| `schema_valid` | no | `false` when the requests or answers break `public.yaml` on purpose (an id with characters to encode, an enum value that the document does not know). Default `true` |
| `skip` | no | `{"<runner>": "<reason>"}`: the runners that skip the case (`typescript`, `python`, `rust`) |
| `client` | no | The options the client is made with (see "The client") |
| `env` | no | The `OSCAR_*` variables the client sees |
| `inject` | no | The clock, the jitter and the UUID source (see "Injection") |
| `origin` | yes | The scheme, host and port every request goes to: the normalized base URL |
| `call` | yes | The call (see "The call") |
| `exchanges` | yes | The requests, in order, and the answer to each. Empty: the call must fail before it sends anything |
| `sleeps` | no | Every call of the injected sleep, in seconds, in order. Default `[]` |
| `expect` | yes | `{"result": ...}` or `{"error": ...}` |

### The client

| Field | TypeScript | Python | Rust |
|---|---|---|---|
| `base_url` | `baseUrl` | `base_url` | `.base_url(...)` |
| `api_key` | `apiKey` | `api_key` | `.api_key(...)` |
| `token_provider` | `tokenProvider` | `token_provider` | `.token_provider(...)` |
| `max_retries` | `maxRetries` | `max_retries` | `.max_retries(...)` |
| `timeout_ms` | `timeoutMs` | `timeout` (seconds, `timeout_ms / 1000`) | `.timeout(Duration::from_millis(...))` |

An option that is absent is not passed. An empty string is passed as it is:
the client must treat it as not set (section 1.1).

`token_provider` is a list of values. The runner gives the client a provider
that returns the next value at each call. When no value is left, the
provider fails. So `["a", "b"]` checks that the provider is called before
each attempt.

### The environment

Before the case, the runner unsets every variable whose name starts with
`OSCAR_`, then sets the variables of `env`. After the case, it puts the
environment back. A variable with the value `""` is set to the empty string,
which counts as not set.

### Injection

| Field | Default | The runner gives the client |
|---|---|---|
| `now` | `2026-09-27T10:00:00Z` | A clock that starts at `now` and moves on by each injected sleep |
| `jitter` | `0` | A jitter source that always returns this value |
| `uuids` | `[]` | A UUID source that returns these values in order and fails when none is left |

The injected sleep does not wait. It records its argument in seconds. So a
case with retries runs at once, and `sleeps` is exact. The UUID source fails
when it is empty, so a client that makes a second key for a retry, or a key
for a `GET`, fails the case.

### The call

An operation call names a generated method:

| Field | Meaning |
|---|---|
| `operation` | `<resource>.<method>` of `clients/codegen/names.yaml`, for example `bank_transactions.list` |
| `path` | The path parameters by wire name, not yet encoded. The runner passes them in the order of the path |
| `query` | The query parameters by wire name. A `null` value is passed as null (not set) |
| `body` | The JSON body, with wire names. Only the fields the caller sets |
| `multipart` | An upload: `file` (`field`, `filename`, `content_type`, `content_base64`) and the text `fields`. An absent `filename` or `content_type` means the caller gives bytes with no name or type |
| `options` | The options of the call: `idempotency_key`, `timeout_ms`, `max_retries`, `headers` (mapped as in "The client") |
| `mode` | For a list: `page` (one page, the default) or `all` (the iterator over every item) |

Each runner maps the names to its language: `oscar.bankTransactions.list(...)`
in TypeScript, `oscar.bank_transactions.list(...)` in Python,
`oscar.bank_transactions().list(...)` in Rust. `mode: page` is `await` of
the call in TypeScript, `first_page()` in Python and `.send()` in Rust.
`mode: all` is `for await` in TypeScript, `for` in Python (the async runner
also plays every case with `async for`), and `.stream()` in Rust. A download
reads the whole stream.

A spec call hands a request spec to the runtime directly, for a rule that no
public operation shows (for example a `POST` without an `Idempotency-Key`,
because every public `POST` accepts one):

```json
{"spec": {"operation_id": "...", "method": "POST", "path": "/api/companies/{company_id}/things",
          "path_params": {"company_id": "cmp0000001"}, "query": {}, "body": {"name": "thing"},
          "idempotent": false, "response": "json"},
 "options": {}}
```

### An exchange

```json
{"request": { ... }, "response": { ... }}
{"request": { ... }, "fault": "connection_error"}
```

`request` says what the request must be. `response` says what the mock
answers. `fault` says that the attempt gets no answer:

| Fault | The mock |
|---|---|
| `connection_error` | Raises the connection error of its transport: no response at all |
| `timeout` | Makes the attempt time out: it raises the timeout error of its transport, or it answers later than the timeout of the attempt |

A response:

| Field | Meaning |
|---|---|
| `status` | The HTTP status |
| `headers` | Header names in lower case |
| `body` | `{"kind": "empty"}`, `{"kind": "json", "value": ...}`, `{"kind": "text", "value": "..."}` or `{"kind": "base64", "value": "..."}` |

When `headers` has no `content-type`, the mock sets it from the body:
`application/json` for JSON with a status below 400,
`application/problem+json` for JSON with a status of 400 and above,
`text/plain; charset=utf-8` for text, `application/octet-stream` for base64.
The mock server takes the same shape in `POST /_mock/responses`.

## The matching rules

The mock answers the n-th request with the n-th exchange, after it checks
the request against `request`. A request that does not match fails the case
at once, with the difference. A request after the last exchange fails the
case. At the end, every exchange must have been used.

| Part | Rule |
|---|---|
| URL | `origin` + `path` exactly. The path is compared in its percent-encoded form. The case writes the encoding of section 3.2: every byte except `A-Z a-z 0-9 - . _ ~` is encoded, with upper-case hex digits |
| Method | Equal |
| Query | The decoded pairs of the query string, compared as a sorted multiset: sort both lists by name, then by value, and compare them. So the order does not matter, a repeated name counts twice, and `+` or `%20` for a space gives the same pair. `[]` means no query |
| Headers | Only the headers that `request.headers` lists are checked. Names are compared in lower case. A string must equal the value. `null` means the header must be absent. `{"pattern": "..."}` must match the value (a regular expression, searched; the patterns are anchored with `^` and `$` where they must be) |
| Body `none` | The request has no body (an empty body counts as none) |
| Body `json` | The body is parsed as JSON and compared with `value` as parsed JSON: equal objects (key order does not matter), arrays, strings, numbers (`1` equals `1.0`), booleans and null |
| Body `multipart` | The body is parsed as `multipart/form-data`. The set of part names must equal the names in `parts`, and the order of the parts does not matter. A text part must have `value` as its content. A file part must have the file name `filename`, the SHA-256 of its content `sha256`, and, when the case lists it, the content type `content_type` |
| `timeout_ms` | The timeout of this attempt, in milliseconds. A runner checks it when its mock can see the timeout (for example the `timeout` of a `respx` request), and ignores it otherwise |

## The expected outcome

### `sleeps`

The recorded sleeps must equal `sleeps`: the same number, and each value
equal within `1e-6` seconds.

### A result

| Kind | The call must give |
|---|---|
| `json` | A value that the runner turns back into wire JSON (wire names, the model's own serialization); it must match `value` |
| `items` | Every item of the iterator, in order, each turned into wire JSON; the list must match `value` |
| `none` | Nothing (no content) |
| `binary` | Bytes with this SHA-256 and size, the content type, and the file name of `Content-Disposition` (null without one) |

"Match" is this rule, the same for every runner:

- An object matches when every key of the expected object matches in the
  actual one. A key whose expected value is null may be absent. The actual
  object may have more keys: a model may keep a field it does not know, or
  drop it.
- An array matches an array of the same length, item by item.
- A string, number, boolean or null matches an equal value. `true` never
  matches `1`.

A date or date-time that a model reads into a typed value is written back in
the form of the case: `YYYY-MM-DD`, and RFC 3339 in UTC with `Z`.

### An error

`{"error": {"type": "NotFoundError", "status": 404, "code": "not_found", ...}}`

The call must raise the error type of `type` (section 3.7). Each other field
that the case lists must equal the field of the error; a field that is
absent is not checked.

| Field | TypeScript | Python | Rust |
|---|---|---|---|
| `type` | the class name | the class name | the variant or kind of `Error` that the Rust SDK maps to that name |
| `status` | `status` | `status` | the status; `null` when the error has no response |
| `code`, `title`, `detail`, `errors` | same name | same name | same name |
| `request_id` | `requestId` | `request_id` | `request_id` |
| `message` | `message` | `str(error)` | `to_string()` |

`AuthenticationError` with `status: null` is the error of section 1: no
credential, or a token provider that failed. The call sends nothing.

## Checking the cases

`clients/mock-server/tests/test_conformance_cases.py` runs in the gates of
the mock server (`uv run pytest` in `clients/mock-server`). For every case it
checks:

1. The file matches `case.schema.json`, and every id is unique and starts
   with the name of its file.
2. The call fits its operation in `clients/codegen/ir.json`: the path
   parameters, the query names, the required query, the body kind, and
   `mode: all` only on a list.
3. The case agrees with the spec. A reference model of
   `clients/spec/README.md` (`tests/conformance.py`) plays the call against
   the answers of the case. It computes the requests (path, query, body,
   credential, `Idempotency-Key`, accept, content type, timeout), the sleeps
   and the outcome, and the test compares them with the case.
4. Every request is a valid request of the OpenAPI document of the public
   API, and every answer is a valid answer of its operation, or a valid
   `Problem` (unless `schema_valid` is `false`).
5. The mock server accepts every answer as a scripted answer.

Every operation and every resource of the IR must have a case, and every
error type must appear in a case.

## Adding a case

1. Add it to the file of its topic, or add a file for a new topic.
2. Write the requests and answers by hand. Take bodies from
   `clients/mock-server/src/oscar_mock_server/fixtures/api.json`, which are
   valid answers of `public.yaml`.
3. Run `uv run pytest tests/test_conformance_cases.py` in
   `clients/mock-server`. A failure names the case and the difference.
4. Run the runner of each SDK.

When `public.yaml` changes, the validator shows each case whose request or
answer is no longer valid.

## Not covered by the cases

| Rule | Where it is tested |
|---|---|
| Step 3 of the credentials, the CLI login (section 1.3) | It starts a process: a unit test of each SDK with a fake `oscar` |
| The browser rules (section 1.2) | The TypeScript unit tests |
| A base URL that is refused at construction (section 3.1) | The unit tests; the spec does not name the error type |
| The iterator fetches a page only when the caller needs it (section 3.6) | The unit tests |
| The timeout of a download stream after its headers (section 3.4) | The unit tests |
| The drop of the cached CLI token after a 401 (section 1.3) | The unit tests |
| The CLI, its store and its OAuth login (sections 2 and 4) | The CLI suite against `clients/mock-server` |
