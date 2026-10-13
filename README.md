# oscaraccountant-rust

The Rust clients of the Oscar accounting API:

| Path | What it is | Published as |
|---|---|---|
| `rust/crates/oscaraccountant` | The Rust SDK | the crate `oscaraccountant` |
| `rust/crates/oscaraccountant-cli` | `oscar`, the command-line tool | the crate `oscaraccountant-cli`, and the npm package `@oscaraccountant/cli` |
| `npm/` | The npm packaging of `oscar`: a small Node script and one binary package per platform | `@oscaraccountant/cli`, `@oscaraccountant/cli-<platform>` |
| `scripts/build-cli.mjs` | Builds the `oscar` binary of one target for the npm packages | - |
| `spec/` | The behaviour every Oscar client follows, and the conformance cases the SDK plays | - |
| `VERSION` | The version of every Oscar client | - |

Run the CLI with no install:

```bash
npx @oscaraccountant/cli login
npx @oscaraccountant/cli whoami
```

Or install it with `cargo install oscaraccountant-cli`. See
`rust/crates/oscaraccountant/README.md` for the SDK and
`rust/crates/oscaraccountant-cli/README.md` for the CLI.

## Build and test

```bash
cd rust
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test -p oscaraccountant
cargo test -p oscaraccountant-cli --bins

cd ../npm
node --test test/shim.test.js test/assemble.test.mjs
```

The Linux binaries of `oscar` are static musl builds, made with
`cargo-zigbuild`: `node scripts/build-cli.mjs --target x86_64-unknown-linux-musl`.

The guides and the reference of the API are at
<https://oscaraccountant.com/docs>.

This repository is exported from the private source repository of Oscar, so
a pull request here is not merged as is. Open an issue for a bug or a wish.

## License

MIT License, copyright OscarAI OÜ. See `LICENSE`.
