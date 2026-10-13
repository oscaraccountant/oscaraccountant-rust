# oscaraccountant-cli

`oscar`, the command-line tool of the Oscar accounting API. It is written in
Rust on the `oscaraccountant` SDK crate, and it is shipped through npm:

```bash
npx @oscaraccountant/cli login
npx @oscaraccountant/cli invoices list --all
```

`cargo install oscaraccountant-cli` installs the same binary.

Log in once in the browser with `oscar login`. The login stores your
organization and a default company (it asks which one when it sees several):

```bash
oscar whoami
oscar companies list
oscar invoices list --all --json
oscar companies use <COMPANY>   # change the default company
```

With `OSCAR_API_KEY` set to an organization API key instead, no profile holds
the organization or the company, so give them on each call (or set
`OSCAR_COMPANY`):

```bash
oscar companies list --organization <ORGANIZATION_ID>
oscar invoices list --all --json --company <COMPANY>
```

`<COMPANY>` is the id of a company, its registry code, or the start of its
name.

The guides and the reference of the API are at
<https://oscaraccountant.com/docs>. `oscar --help` lists every command.

The shared behaviour of every Oscar client (the credentials, the store, the
requests, the OAuth login and the exit codes) is in `spec/README.md` of the
repository `oscaraccountant-rust`. This file says how the CLI applies it.

## Log in

| Command | Does |
|---|---|
| `oscar login` | Opens the browser (`$BROWSER` when set) and waits at most 5 minutes for the redirect to `http://127.0.0.1:<port>/callback`. It prints the URL when no browser opens. The browser then lands on the page `/cli/done` of the web app (or `/cli/error?reason=<code>`); a server with no such page gets a plain page of the CLI |
| `oscar login --device` | Prints a code and a URL to open on another device, and on a terminal a QR code of the URL with the code in it, then polls. The default when no display is found (Linux with no `DISPLAY` and no `WAYLAND_DISPLAY`, or any SSH session) |
| `oscar login --api-key` | Reads an organization API key from a prompt that does not echo on a terminal, else from standard input |
| `oscar logout` | Revokes the refresh token, then deletes the secret and the profile |

`--profile NAME`, `--base-url URL`, `--scope read|write` and
`-c/--company <x>` apply to `login`. The first login becomes the default
profile. A login to another base URL with no `--profile` is refused (exit
code 2) when the default profile is for another server: give `--profile NAME`
to add a second profile.

On a terminal with no flag, `oscar login` asks, in order:

1. The server, only when a second server is in play (two profiles, or a
   profile of another server than the default one). `--base-url` or
   `--profile` answers it.
2. The method: the browser, a device code, or an API key. `--device` or
   `--api-key` answers it.
3. The default company, when the login sees several: a list with the name,
   the registry code and the state. `-c/--company <x>` answers it.

A spinner runs while the CLI waits for the browser or the device. The last
line is `✓ Logged in as <email> · <organization> · company <name>`.

### No prompt: `--no-input`

Every prompt has a flag. With no terminal (standard input or standard error
is not one), or with `--no-input` or `OSCAR_NO_INPUT=1`, the CLI never asks
and never waits: it takes the default of each question (the browser, or the
device code with no display), and a value it needs and cannot default is a
usage error (exit code 2) that names the flag.

### Companies

A login reads `GET /api/me/` and caches the companies of the credential in the
profile (`companies` in `config.json`). One company becomes the default.
Several: the picker on a terminal; with no terminal the previous default
stays, else no company is chosen and the CLI says how to choose one. None:
the CLI offers `oscar onboard`.

`-c/--company <x>` on every company command, and `OSCAR_COMPANY`, take the
id of a company, its registry code, or a unique, case-insensitive start of
its name (`-c alpha`). An ambiguous value is a usage error that lists the
matches. The cache answers first; a value it does not know reads
`/api/me/` again and stores the fresh list.

| Command | Does |
|---|---|
| `oscar companies use <x>` | Makes the company the default of the profile |
| `oscar whoami` | Shows the user, the organizations, the companies and the default company |
| `oscar config set company <id>` | Sets the default company by id, with no check |

The success line of a write names the company:
`✓ Created party 00000179 · Alpha OÜ · <link>`.

## The credential of a call

The first match wins:

1. `--api-key KEY`, for this call only. It is never stored.
2. `OSCAR_API_KEY`.
3. The profile: `--profile`, else `OSCAR_PROFILE`, else the default profile.
   With `--base-url` or `OSCAR_BASE_URL`, only a profile of that base URL
   lends its credential.

An OAuth access token that expires within 60 seconds is refreshed before the
call, and the rotated refresh token is stored at once.

`oscar auth token --json --base-url URL` is the command the SDKs run for the
CLI login (spec section 1.3). It prints
`{"token", "expires_at", "kind", "base_url", "profile"}` and answers only
for a profile of that base URL.

## The store

| File | Where |
|---|---|
| `config.json` (the profiles, no secret) | `OSCAR_CONFIG_DIR`, else the config directory of the OS plus `oscaraccountant` |
| The secrets | The OS keychain: service `oscaraccountant`, account `profile:<name>` |
| `credentials.json` (the fallback) | `OSCAR_CONFIG_DIR`, else the local data directory of the OS plus `oscaraccountant`. Mode 0600 on Unix; an owner-only access list on Windows |

The fallback is used when no keychain service answers (a server, CI, a
container, a Linux desktop with no Secret Service). The CLI says so on
standard error when it creates the file. `OSCAR_CREDENTIAL_STORE=file`
keeps the secrets in the file and never asks the keychain (for tests, and
for machines where the keychain prompts).

## The API commands

`oscar <resource> <command>` runs one operation of the public API. The table
of commands is generated from the OpenAPI document of the public API by
`oscar-codegen` into `src/generated/` (never edit it by hand).

- A list is `list`, one item is `get <ID>`, and a report or a single object
  is the resource itself: `oscar invoices list`, `oscar invoices get <ID>`,
  `oscar trial-balance`, `oscar open-items`, `oscar me`. The method name of
  the SDKs (`retrieve`) still works as a hidden alias.
- A path parameter is an argument: `oscar invoices get <INVOICE_ID>`. An id
  also takes a unique prefix, or the short form that `-o wide` shows (the
  last 8 characters of a ULID), in every command: `get`, `issue`, `cancel`,
  `revise`.
- `-c/--company` defaults to `OSCAR_COMPANY`, then to the company of the
  profile (`oscar companies use <x>`); it takes an id, a registry code or the
  start of a name (see "Companies"). `--organization` defaults to the
  organization of the profile.
- A query parameter is a flag: `--state issued --limit 50`. The CLI checks
  numbers, booleans and dates before the call.
- A JSON body is `--data '<json>'`, `--data @file.json` or `--data -`.
- A command that accepts an `Idempotency-Key` sends one automatically;
  `--idempotency-key` gives your own.
- `--all` walks every page of a list.
- `oscar files upload <PATH> [--kind ...]` and
  `oscar files download <FILE_ID> [-o PATH|-]` are hand-written.
- `oscar api <METHOD> <PATH> [--data ...]` calls any path with the stored
  credential. It prints the body, also the body of an error.

### Output

On a terminal, a list is a table and one object is a card. When standard
output is not a terminal, or with `--json`, the output is the JSON of the
API, unchanged.

- A table has no border and never wraps. When a row does not fit, the text
  columns are cut with `…` down to 16 characters, then the columns that
  matter least go: the title, the status, the short id and the key amount
  stay longest, the other columns keep their order. Ids, codes, numbers,
  dates, amounts and statuses are never cut. A row with no title yet (a
  draft has no number) shows its short id.
- An amount is `1 220.00 EUR`, right aligned. A negative amount is red, or
  in parentheses with no colour. A status is a dot and the word, coloured by
  its meaning; an invoice shows its state when it says more than its
  settlement (`cancelled`, `issued`). Basis points are a percentage. A date
  is `2026-10-12`; a time less than 7 days ago is
  relative (`3h ago`); a due date in the past is `overdue 5d`.
- A card has a header (title, status, amount), the fields as
  `Label  value`, a section per nested object, a short table per list (10
  rows, then `… 14 more (--json for all)`), and the ids in `Meta`. A row
  that holds a list of its own (an account of `oscar general-ledger`) has
  that list right below it.
- Notes go to standard error: `Showing 20 · more: --cursor ... (or --all)`,
  the empty state, and the success line of a write, which names the object
  and its amount (`✓ Issued invoice 2026-0042 (1 220.00 EUR) · <link>`).

| Option | Effect |
|---|---|
| `-o table` | A table or a card (the default on a terminal) |
| `-o wide` | More columns, and the short id |
| `-o json` | The JSON of the API (the default when standard output is not a terminal) |
| `-o ndjson` | One JSON item per line; with `--all`, page by page |
| `--columns a,b` | These columns, in this order: a field name or a header (`gross`, `status`). An unknown name lists the close ones |
| `--json` / `--json=a,b` | JSON, also on a terminal; with names, only these fields |
| `--jq <expr>` | Filter the JSON with a jq expression (jaq); a string prints as it is |
| `--template <text>` | Print with a template (minijinja): `'{{ number }} {{ state }}'`. On a list it runs once per item, unless it names `items` |

Colours follow `NO_COLOR` (off) and `CLICOLOR_FORCE` (on, also in a pipe).
`TERM=dumb` and a locale that is not UTF-8 give ASCII symbols. A terminal
that supports them gets links to the web app (OSC 8); `FORCE_HYPERLINK=1`
or `0` decides it. `COLUMNS` sets the width.

| Exit code | Meaning |
|---|---|
| 0 | Success |
| 1 | An error of the API |
| 2 | A usage error |
| 3 | Authentication needed |
| 4 | A network error |
| 130 | Stopped with Ctrl-C |

## Onboard a company

`oscar onboard` takes a company through the onboarding of Oscar in the
terminal, in the order of the server, and starts again at the first step that
is not done. `oscar onboard status` prints the steps.

| Step | Where | Flags |
|---|---|---|
| 0. Account | Log in (`oscar login`). The CLI asks first whether you have a company in Oscar. No company yet (new, or a sign-up that stopped before the company): the CLI opens the onboarding in the browser for the account and the company, then logs in. A login whose consent page has no organization to offer takes the same path | |
| 1. Your company | The Business Register search with autocomplete, the register data, the books answers, the website, the written confirmation of the beneficial owners | `--registry-code`, `--books-today`, `--books-no-reason`, `--books-software`, `--books-software-other`, `--website` or `--no-website`, `--confirm-owners` |
| 2. About your business | The questions one at a time, a review, then the submission | `--answers <file.json>`, `--answer key=value` (repeat it) |
| 3. Verify your identity | The privacy notices and the PEP question; the Didit link and its QR code, in the browser or on a phone; the CLI waits for the result. Every other person who must verify is listed | `--consent`, `--pep`, `--pep-related`, `--pep-details`, `--invite-links`, `--language` |
| 4. Client agreement | The agreement pager, then `I accept` | `--language` (no flag accepts it) |
| 5. Choose your plan | The prices, then Checkout in the browser; the CLI waits for the payment | `--plan month` or `--plan year` |
| 6. Set up | The checklist with the state of each item and the link to the web app | |

- A company in a new organization needs a new login: the CLI runs
  `oscar login` again after step 1, so the login reaches the new company.
- One onboarding is open at a time, as in the web app: `--registry-code`
  of another company is refused until the open one is done. An onboarding
  open in another organization than the login's is named with that
  organization; in a terminal the CLI offers the login to it.
- `--no-wait` prints the link of a browser step and stops; run
  `oscar onboard` again later.
- With no terminal, or with `--no-input`, a missing answer is a usage error
  that names its flag. The client agreement then cannot be accepted: the CLI
  prints the link of the agreement in the web app.
- An older CLI than the server's `cli_min_version` asks you to update first.
- Before the launch, a refusal `pre_launch` says how to use an invite link.

### The agreement pager

`oscar onboard agreement` (and step 4) shows the client agreement full
screen, in your language (`l` switches between English and Estonian), with its
version and SHA-256 in the footer. The CLI checks that the SHA-256 is the one
of the text it shows.

| Key | Does |
|---|---|
| Arrows, `j` `k`, PgUp, PgDn, Space, Home, End, `g` `G` | Scroll |
| `/`, then `n` and `N` | Search, next and previous match |
| `l` | The other language |
| `a` | Accept, once the end of the text was on the screen: type `I accept` |
| `q`, Esc | Leave without accepting |

When the server answers that the text changed, the new version opens to be
read again. `oscar onboard agreement --download [PATH]` saves the PDF of the
acceptance.

## Build and test

From `rust/`:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test -p oscaraccountant-cli           # unit tests and the CLI suite
cargo llvm-cov -p oscaraccountant-cli --summary-only --ignore-filename-regex '[/\\]generated[/\\]'
```

The CLI suite (`tests/cli.rs`) runs the real binary against
the mock server of the API, which it starts with `uv run`. The mock server
is not in the public repository, so run `cargo test -p oscaraccountant-cli --bins`
there. So `uv` must be on
`PATH`. It covers the three logins, refresh, logout, the file store, every
command in JSON and table form, and the exit codes. Two variables change it:

| Variable | Effect |
|---|---|
| `OSCAR_CLI_BIN` | Test this binary instead of the one Cargo built (a musl or a cross-built binary) |
| `OSCAR_MOCK_SERVER_DIR` | The directory of the mock server project |

`OSCAR_FORCE_TTY=1` makes the CLI print tables when standard output is not a
terminal. The suite uses it to check the table form. `OSCAR_NOW=<RFC 3339>`
fixes the time of a run, for relative times in a test.

`tests/onboard.rs` runs `oscar onboard` against the scripted onboarding of
the mock server: every step with flags and no terminal, the stops, and the
agreement pager in a pseudo terminal (Unix only). `OSCAR_POLL_INTERVAL_MS`
and `OSCAR_POLL_TIMEOUT_SECS` set how often and how long the CLI waits for
Didit and Checkout. `onboarding-calls.json` lists every hand-written call of
the onboarding; a unit test keeps it equal to the code
(`OSCAR_UPDATE_MANIFEST=1 cargo test -p oscaraccountant-cli manifest` writes
it), and the server checks it against its schema.

`tests/render.rs` keeps a snapshot (`insta`) of every list and detail
command at 80 and 120 columns, with and without colour, against the
fixtures of the mock server. `cargo insta review` shows a change.

The release binaries are built by `scripts/build-cli.mjs`, and the npm
packages by `npm/scripts/assemble.mjs`.

## Build constraints

- The `keyring` crate has pure-Rust backends only (`apple-native`,
  `windows-native`, `async-secret-service` with `crypto-rust` and `tokio`),
  so the Linux binary is a static musl build with no `libdbus`.
- No `openssl-sys` and no `aws-lc-sys`: `cargo tree -p oscaraccountant-cli
  --target all -i openssl-sys` must find no package. TLS is rustls with ring,
  and the CLI trusts the certificate store of the platform.
- The keychain calls block, so they run on the blocking threads of tokio
  (the Secret Service backend starts its own runtime, and panics inside a
  tokio task).

MIT License, copyright OscarAI OÜ.
