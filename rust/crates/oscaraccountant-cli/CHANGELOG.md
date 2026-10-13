# Changelog

All notable changes to the `oscaraccountant-cli` crate (the `oscar` binary and
the npm package `@oscaraccountant/cli`) are recorded here. The version is the
one of every Oscar client (the file `VERSION`).

## 0.1.0 - unreleased

The first version.

- `oscar login`: in the browser (loopback redirect with PKCE), with a device
  code, or with an API key. `oscar logout` revokes the login.
- `oscar whoami`, `oscar auth token [--json]`, `oscar auth status`,
  `oscar profiles list|use`, `oscar config get|set company`.
- One command per operation of the public API, generated from its OpenAPI
  document, with `--all` for every page. `oscar files upload|download`,
  `oscar api` for any path, and `oscar completion`.
- The secrets in the OS keychain, or in `credentials.json` (mode 0600) when
  no keychain answers.
- A table on a terminal, JSON otherwise. Exit codes 0 to 4, and 130 for Ctrl-C.
