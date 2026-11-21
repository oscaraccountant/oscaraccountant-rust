# Changelog

All notable changes to the `oscaraccountant` crate are recorded here. The
version is the one of every Oscar client (the file `VERSION`).

## 0.2.0 - 2026-11-20

This version breaks the code that used organizations. The API has no
organizations any more: every user, key and login reaches companies.

### Removed

- `oscar.organizations().list()` and `oscar.organizations().retrieve()`, and the endpoints `GET /api/organizations/` and
  `GET /api/organizations/{organization_id}/`. The types `Organization`, `PaginatedOrganizationList` and `MeOrganization` are gone.
- The field `organizations` of the current user (`me`), and the field
  `organization_id` of a company (in `me`, in the list of the companies and
  in the detail of one company).

### Changed

- `oscar.companies().list()` lists every company the credential reaches
  (`GET /api/companies/`). It takes no organization: `oscar.companies().list(organization_id)` of
  0.1 called `GET /api/organizations/{organization_id}/companies/`, which is
  gone.
- The permissions `organization.read` and `organization.manage` are now
  `companies.read` and `companies.manage`.

### Added

- The fields `role` and `is_member` of each company of the current user
  (`me`): the role the caller acts with in the company, and whether the
  user is a member of it (firm staff see companies with no membership).
- `oscar.invoices().bank_accounts()`, `oscar.invoices().set_bank_accounts()`, `oscar.invoices().set_default_bank_accounts()`, `oscar.parties().update()`, `oscar.parties().set_address()` and `oscar.files().delete()`.
- Invoices: the bank accounts that an own sales invoice prints. The field
  `bank_account_ids` of a revision, and the same field when you record,
  revise or issue an invoice; the read of the accounts an invoice prints;
  the choice for one draft and the default of the company.
- Parties: edit the name, the codes, the contact details and the notes of a
  party, and set its postal address.
- Files: delete a file that the books do not use. The field `deleted_at` of
  a file, and `deletion` (whether you can delete it, and what blocks it) on
  the detail of one file. An upload takes `direction` for a sales invoice
  issued elsewhere.
- Documents: the fields `stored_filename`, `withdrawn` and `withdrawn_at`,
  and on the detail of one document `references`, `status_events`,
  `withdrawable`, `withdraw_blocked_by` and `deletion`. A revision of an
  invoice has `removed_documents`.
- Bank accounts: the fields `invoice_position`, `ledger_currency`,
  `broker_key`, `mmf_account_code`, `first_booking_date` and
  `booked_months` (each month with a transaction).
- Bank transactions: the fields `receipt_state`, `receipt_detail`,
  `match_proposal_id` and `matched_invoice_candidates` say why a payment to
  explain is still open: no receipt, a receipt that is not booked yet, or a
  booked invoice that is not matched. The list parameter `receipt_state` lists one
  state. With `receipt_state` or `to_explain`, the page of the list has `coverage`:
  the bank accounts and the months that the answer checked.
- The status of a company: `vat_months_not_checked`, `vat_not_checked_from`
  and `vat_not_checked_to`. The feature `lightyear` of the current user.

## 0.1.2 - 2026-11-09

- New values of the response enums: the permissions `einvoicing.manage`,
  `foreign_tax.file`, `reconciliation.decide` and `support.use`; the company
  state `deleted`; the bank transaction statuses `migrated` and `retracted`;
  the VAT treatments `import` and `foreign_scheme`. The status and the
  treatment are also new filter values of their lists. A client of an
  earlier version reads each of them as an unknown value and does not fail.
- Bank transactions: the fields `migrated_event_id` (the imported event of a
  `migrated` line) and `retraction` (why a `retracted` line is out of the
  books), and `retractable` on the detail of one line.
- Invoices: the fields `posted_at` and `review_decision`, and on the detail
  of one invoice `allocations` and `migrated_from`. A revision has the
  fields `buyer_contact_name`, `order_number`, `contract_number` and
  `buyer_reference`, and you can set them when you create, revise or issue an
  own sales invoice. A document of a revision has `file_id`, `source` and
  `download_url`.
- Parties: the field `roles`, and the list parameters `role` and
  `exclude_kind`.
- VAT codes: the fields `special_scheme`, `supply_kind`, `rate_type`,
  `member_state` and `goods_location`, and the list parameter
  `special_scheme`, for the OSS and IOSS schemes, UK VAT and the
  Australian GST.
- The general ledger leaves out the correction pairs by default. The
  parameter `include_corrections` includes them, and the field
  `hidden_corrections` counts them.
- The parameter `omit_empty` of the balance sheet and of the profit and
  loss report: it leaves out the lines that are 0.
- Employees: the field `names` of a submission, the fields `occupation_name`
  and `workplace_address` of an employment, and `workplace_address` in the
  employment of a new submission.
- The field `due_date` of the VAT preview, and the features
  `foreign_tax_gb`, `foreign_tax_au`, `kmd_submission` and `payroll` of the
  current user.
- The field `counter_role` of a bank transaction exemption can be null: an
  exemption to an account of the chart names `counter_account_code` instead.
  Its type is now `Option<BooksExemptionRoleEnum>`. Version 0.1.1 failed to
  read the detail of such a bank transaction.

## 0.1.1 - 2026-10-15

- The operations of the employee register and of payroll in the public
  API: list and read the employees, list, create and read the submissions
  of an employee change, and list and read the pay runs. Their models and
  enums come with them.
- The feature `employees` of the current user, and the permission
  `employees.submit`.
- The parameter `include_settled_external` of the parties list. It includes the parties
  with a key in another system (a Stripe customer) that have no open
  item. By default the list leaves them out unless it is filtered by
  `search`, `reg_code`, `vat_number` or `external_key`.
- The README says to install the CLI before `oscar login`: the SDK runs
  `oscar` from `PATH`, so a login with `npx` alone does not serve it.

## 0.1.0 - 2026-10-15

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
