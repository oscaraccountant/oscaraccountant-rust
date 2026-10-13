# @oscaraccountant/cli

`oscar`, the command-line tool of the Oscar accounting API.

```bash
npx @oscaraccountant/cli login          # log in in the browser (or --device, or --api-key)
npx @oscaraccountant/cli whoami
npx @oscaraccountant/cli invoices list --all
```

Or install it once: `npm install --global @oscaraccountant/cli`, then run `oscar`.

`oscar login` stores its credential in the keychain of the OS. On a server or
in CI, set `OSCAR_API_KEY` to an organization API key instead: it wins over
the login.

The package holds a small Node script. The binary itself comes in one optional package per
platform (`@oscaraccountant/cli-linux-x64`, `-linux-arm64`, `-darwin-arm64`, `-darwin-x64`,
`-win32-x64`, `-win32-arm64`), and npm installs only the one for your machine. There is no
install script.

When `oscar` says that the binary is missing, npm skipped the optional package: this happens
when a lockfile was made on another platform, or with `--omit=optional`. Delete `node_modules`
and `package-lock.json` and install again.

The documentation of the commands is `oscar --help`, and the guides and the
reference of the API are at <https://oscaraccountant.com/docs>. The source is
<https://github.com/oscaraccountant/oscaraccountant-rust>.

MIT License, copyright OscarAI OÜ.
