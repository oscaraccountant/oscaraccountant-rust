# Changelog

All notable changes to the `oscaraccountant` crate are recorded here. The
version is the one of every Oscar client (the file `VERSION`).

## 0.1.0 - unreleased

The first version.

- `Client::builder()` and `Client::from_env()`, with one request builder per
  operation of the public API: `.send()` for one result or one page,
  `.stream()` for every item of every page.
- The credential order: an API key or a token provider, then
  `OSCAR_API_KEY`, then the stored login of the `oscar` CLI when asked for.
- Retries with backoff and `Retry-After`, an automatic `Idempotency-Key`,
  RFC 9457 errors with the request id, multipart uploads and streamed
  downloads.
- A raw call (`Client::request`) for any path, and an injectable HTTP
  transport, clock, sleep, jitter and UUID source.
