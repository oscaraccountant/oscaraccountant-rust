# Changelog

All notable changes to the `oscaraccountant-cli` crate (the `oscar` binary and
the npm package `@oscaraccountant/cli`) are recorded here. The version is the
one of every Oscar client (the file `VERSION`).

## 0.2.0 - 2026-11-20

This version follows the API with no organizations: a login reaches
companies.

- `oscar organizations list` and `oscar organizations get` are gone, and so is
  the option `--organization`.
- `oscar companies list` asks nothing and needs no company: it calls
  `GET /api/companies/` and lists every company the login reaches, one page
  at a time (`--cursor`, `--limit`, `--all`).
- The profile in `config.json` keeps the companies of the login and the
  default company. The keys `organizations` and `organization` of an older
  CLI, and `organization_id` in the cached companies, are dropped when the
  CLI reads the file, and never written again. A login of 0.1 keeps working:
  no new login is needed.
- The JSON of `oscar auth status` and `oscar profiles list` has `companies`
  in place of `organizations` and `organization`. `oscar whoami` shows no
  organizations.
- The last line of a login is `Logged in as <email> · <n> companies · company
  <name>`; the number shows when the login reaches more than one company.
- `oscar onboard` says "company" where it said "organization". A new company
  needs a new login: on the consent page, choose that company. An onboarding
  of a company that the login does not reach says so, and names the company
  to choose.
- New commands: `oscar invoices bank-accounts`, `oscar invoices
  set-bank-accounts`, `oscar invoices set-default-bank-accounts`, `oscar
  parties update`, `oscar parties set-address` and `oscar files delete`.
- The new fields of the API in the commands of the files, the documents, the
  invoices, the bank accounts and the companies. `oscar files get` shows
  whether the file can be deleted.
- `oscar bank-accounts list` has the fields `first_booking_date` and
  `booked_months`. `oscar bank-transactions list --receipt-state <state>`
  lists the payments to explain in one receipt state (`missing`,
  `receipt_not_booked` or `invoice_not_matched`).
- `oscar bank-transactions list --to-explain true` answers which receipts
  are missing, on a server that sends the receipt state of each payment:
  - The column `RECEIPT` says `missing`, `received, not booked` or
    `invoice booked, not matched`. In the payments to explain it stands in
    the place of `STATUS`, which moves to `-o wide`; in any other list it
    comes after `STATUS`. `oscar bank-transactions get` shows the receipt,
    why a received receipt is not booked yet, and the booked invoices that
    fit the payment.
  - After the table, the bank lines the answer was checked against: per
    bank account its first and last booking date (`DD.MM.YYYY`), the
    number of lines and the months with no lines, then the accounts with no
    bank lines and the bank feed accounts that the books do not have.
    Months and accounts outside this range are not checked.
  - `--all` with `--json` keeps the fields of the first page besides
    `items` and `next_cursor`, so the JSON has the `coverage` of the
    answer. `-o ndjson` stays one payment per line, with no coverage line.
  - A server that sends no receipt state prints as before.

## 0.1.2 - 2026-11-09

- The new values and fields of the public API in the commands of the bank
  transactions, the invoices, the parties, the VAT codes, the general
  ledger, the reports and the employees. `oscar bank-transactions` shows
  the statuses `migrated` and `retracted`, and a retracted line shows why.
- The options `--include-corrections` of `oscar general-ledger`,
  `--role` and `--exclude-kind` of `oscar parties list`, `--special-scheme`
  of `oscar vat-codes list`, and `--omit-empty` of
  `oscar reports balance-sheet` and `oscar reports profit-and-loss`.
- `oscar onboard` follows the current onboarding:
  - The PEP questions (a politically exposed person) are on the company
    step, with the books. A later step does not ask them again.
  - A young company is asked if it already has accounting, and is not asked
    why it keeps no books.
  - `--books-software` accepts `e_arveldaja` (e-Arveldaja, e-Financials).
    `directo` is not an answer any more.
  - The questionnaire asks the share of the turnover in cash, when the
    company uses cash. The answer is a whole number from 0 to 100.
  - When the Business Register does not show the whole ownership, the
    questionnaire asks three more questions: bearer shares, a trust or a
    foundation among the owners, and nominees.
  - The consent of the identity check names the biometric data, and the
    CLI sends the version of the consent text. The link to the Didit
    privacy notice is the current one.
  - Plain wording of two questions: who runs the company, and how long the
    founders have done this kind of work.
- A browser that `$BROWSER` names starts in its own process group, so it
  stays open when `oscar` ends.
- `oscar companies list` with no `--organization` lists the companies of
  every organization of the login, also with `OSCAR_API_KEY`. Before, a
  login of several organizations (or a key with no profile) failed with
  "Give --organization <id>".
- The lists of the invoices, the documents and the bank transactions show
  the newest first (`-invoice_date`, `-created_at`, `-booking_date`) unless
  `--ordering` is given. Before, they showed the oldest first.
- `-c/--company` also works before the command: `oscar -c 16978080 invoices
  list`. A `-c` after the command wins.
- `oscar companies get <COMPANY>` takes the company as an argument (the id,
  the registry code or the start of the name). With `-c` of another company
  too, it is a usage error.

## 0.1.1 - 2026-10-15

- The commands of the employee register and of payroll in the public API:
  list and read the employees, list, create and read the submissions of an
  employee change, and list and read the pay runs.
- `oscar me` shows the row `Employees`: whether the employee register is on.
- The option `--include-settled-external` of `oscar parties list`. It
  includes the parties with a key in another system (a Stripe customer)
  that have no open item. By default the list leaves them out unless it
  is filtered by `--search`, `--reg-code`, `--vat-number` or
  `--external-key`.
- The README has no notes for contributors any more (the build, the tests
  and the build constraints). It links to the source repository.

## 0.1.0 - 2026-10-15

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
