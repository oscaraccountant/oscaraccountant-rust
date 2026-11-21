//! `oscar <resource> <method>`: one public operation from the generated table.

use clap::ArgMatches;
use oscaraccountant::{Client, HttpMethod};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};

use super::{method, read_data, string};
use crate::cli::{COMPANY_POSITIONAL, default_ordering, flag, path_id};
use crate::error::{CliError, Result};
use crate::generated::{Body, Command, Response, ValueKind, View};
use crate::render::{Format, ListInfo, show};
use crate::session::{App, Selection};
use crate::time;

/// Every byte but the RFC 3986 unreserved characters is encoded in a path value
/// (`clients/spec/README.md`, section 3.2).
const PATH_VALUE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// A call built from the arguments, before it is sent.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// The method.
    pub method: HttpMethod,
    /// The path with its values encoded.
    pub path: String,
    /// The query.
    pub query: Vec<(String, String)>,
    /// The JSON body.
    pub body: Option<Value>,
    /// The `Idempotency-Key` the caller gave.
    pub idempotency_key: Option<String>,
}

/// Run the operation and print its answer.
pub async fn run(app: &mut App, operation: &'static Command, leaf: &ArgMatches) -> Result<()> {
    let config = app.config()?;
    let selection = app.select(&config)?;
    crate::cli::check_output(&app.globals.output)?;
    let company = company_flag(app, leaf)?;
    let selection = if operation.args.iter().any(|arg| arg.name == "company_id") {
        app.with_company(selection, company.as_deref()).await?
    } else {
        selection
    };
    let mut call = build(app, operation, leaf, &selection)?;
    let client = app.client_for(&selection).await?;
    resolve_ids(&client, operation, &mut call).await?;
    if operation.response == Response::Binary {
        let response = send(&client, operation, &call, None).await?;
        let _ = app.out.stdout().write_all(&response.body);
        return Ok(());
    }
    let options = app.globals.output.clone();
    let value = if operation.paginated && flag(leaf, "all") {
        let stream = options.format == Some(Format::Ndjson)
            && options.jq.is_none()
            && options.template.is_none();
        let mut items = Vec::new();
        let mut cursor: Option<String> = None;
        let mut first: Option<Value> = None;
        loop {
            let page: Value = send(&client, operation, &call, cursor.as_deref())
                .await?
                .json()?;
            if first.is_none() {
                first = Some(page.clone());
            }
            if let Some(Value::Array(page_items)) = page.get("items") {
                if stream {
                    // `-o ndjson --all` prints each page as it comes.
                    let page = json!({ "items": page_items, "next_cursor": null });
                    show(
                        &mut app.out,
                        &options,
                        &page,
                        operation.view,
                        &list_info(operation, &call),
                    )?;
                } else {
                    items.extend(page_items.iter().cloned());
                }
            }
            match page.get("next_cursor").and_then(Value::as_str) {
                Some(next) if !next.is_empty() => cursor = Some(next.to_owned()),
                _ => break,
            }
        }
        if stream {
            return Ok(());
        }
        every_page(items, first.as_ref())
    } else {
        let response = send(&client, operation, &call, None).await?;
        if operation.response == Response::Empty || response.body.is_empty() {
            app.out.note("Done.");
            return Ok(());
        }
        response.json()?
    };
    let mut info = list_info(operation, &call);
    info.link = web_link(&selection.base_url, operation, &call.path);
    if operation.http_method != "GET"
        && !options.machine(app.out.stdout_tty)
        && options.jq.is_none()
        && options.template.is_none()
        && let Some(message) = success_message(
            operation,
            &value,
            item(&client, operation, &call, &value).await.as_ref(),
            call.body.as_ref(),
            None,
        )
    {
        // The company of the write, by name, then the link.
        let company = operation
            .args
            .iter()
            .any(|arg| arg.name == "company_id")
            .then(|| app.company_label(&selection, company.as_deref()))
            .flatten();
        let message = crate::company::success(message, company.as_deref(), info.link.as_deref());
        crate::render::success_line(&mut app.out, &message);
        if value.get("command_id").is_some() {
            return Ok(());
        }
    }
    show(&mut app.out, &options, &value, operation.view, &info)
}

/// The one page of `--all`: every item, no next cursor, and the other fields of the first page
/// as the API gave them (the `coverage` of the payments to explain is the same on every page).
fn every_page(items: Vec<Value>, first: Option<&Value>) -> Value {
    let mut page = serde_json::Map::new();
    page.insert("items".into(), Value::Array(items));
    page.insert("next_cursor".into(), Value::Null);
    if let Some(Value::Object(first)) = first {
        for (name, value) in first {
            if !matches!(name.as_str(), "items" | "next_cursor") {
                page.insert(name.clone(), value.clone());
            }
        }
    }
    Value::Object(page)
}

/// The company the command line gives: the positional `<COMPANY>` of `oscar companies get`,
/// else `-c/--company` (of the command, else of the root). The positional and `-c` with two
/// different values is a usage error.
pub fn company_flag(app: &App, leaf: &ArgMatches) -> Result<Option<String>> {
    let given = |value: &&str| !value.trim().is_empty();
    let flag = string(leaf, "company")
        .filter(given)
        .or_else(|| app.globals.company.as_deref().filter(given));
    match (string(leaf, COMPANY_POSITIONAL).filter(given), flag) {
        (Some(positional), Some(flag)) if positional.trim() != flag.trim() => Err(CliError::usage(
            format!("Give the company once: `{positional}` or -c `{flag}`, not both."),
        )),
        (positional, flag) => Ok(positional.or(flag).map(str::to_owned)),
    }
}

/// The page of the web app for the answer of an operation on a company: `<base>/c/<id>/<page>`.
pub fn web_link(base_url: &str, operation: &Command, path: &str) -> Option<String> {
    let company = path
        .strip_prefix("/api/companies/")?
        .split('/')
        .next()
        .filter(|company| !company.is_empty())?;
    let page = match (operation.resource, operation.method) {
        ("companies", _) => "overview",
        ("reports", "balance-sheet") => "balance-sheet",
        ("reports", "profit-and-loss") => "profit-and-loss",
        (
            "invoices" | "accounts" | "documents" | "parties" | "periods" | "dimensions"
            | "general-ledger" | "open-items" | "trial-balance",
            _,
        ) => operation.resource,
        ("files", _) => "documents",
        ("inbox", _) => "documents/forwarding",
        ("vat-preview", _) => "vat-returns",
        ("bank-transactions", _) => "bank",
        ("bank-accounts", _) => "banking",
        _ => return None,
    };
    Some(format!(
        "{}/c/{company}/{page}",
        base_url.trim_end_matches('/')
    ))
}

/// `invoices` is `invoice`, `parties` is `party`.
fn singular(resource: &str) -> String {
    let words = resource.replace('-', " ");
    if let Some(stem) = words.strip_suffix("ies") {
        format!("{stem}y")
    } else {
        words.strip_suffix('s').unwrap_or(&words).to_owned()
    }
}

/// The object a write acted on, read back for its success line: its `get` command, at the id of
/// the path (`issue`, `cancel`), or at the id the answer gives (`record`, `create`). `None` when
/// there is no such command or the read fails: the line then names what the answer has.
async fn item(
    client: &Client,
    operation: &Command,
    call: &Call,
    value: &Value,
) -> Option<(&'static View, Value)> {
    let (get, path) = item_call(operation, &call.path, value)?;
    let read = Call {
        method: HttpMethod::Get,
        path,
        query: Vec::new(),
        body: None,
        idempotency_key: None,
    };
    let answer: Value = send(client, get, &read, None).await.ok()?.json().ok()?;
    Some((get.view?, answer))
}

/// The `get` command of the object a write acted on, and its path.
fn item_call(operation: &Command, path: &str, value: &Value) -> Option<(&'static Command, String)> {
    let template: Vec<&str> = operation.path.trim_end_matches('/').split('/').collect();
    let segments: Vec<&str> = path.trim_end_matches('/').split('/').collect();
    let is_get = |other: &&Command, wanted: &str| {
        other.http_method == "GET" && other.path.trim_end_matches('/') == wanted
    };
    // The last id of the path, when the write acts on one object.
    if let Some(index) = template
        .iter()
        .rposition(|segment| segment.starts_with('{') && !is_scope_id(segment))
    {
        let wanted = template[..=index].join("/");
        let get = crate::generated::COMMANDS
            .iter()
            .find(|other| is_get(other, &wanted))?;
        return Some((get, segments.get(..=index)?.join("/") + "/"));
    }
    // A write that makes an object: its id is in the answer, its `get` one segment deeper.
    let result = value
        .get("result")
        .filter(|result| result.is_object())
        .unwrap_or(value);
    let base = operation.path.trim_end_matches('/');
    crate::generated::COMMANDS.iter().find_map(|other| {
        let tail = other
            .path
            .trim_end_matches('/')
            .strip_prefix(base)?
            .strip_prefix("/{")?
            .strip_suffix('}')?;
        if other.http_method != "GET" || tail.contains('/') {
            return None;
        }
        let id = result
            .get(tail)
            .or_else(|| result.get("id"))
            .and_then(Value::as_str)?;
        let id = utf8_percent_encode(id, PATH_VALUE).to_string();
        Some((other, format!("{}/{id}/", path.trim_end_matches('/'))))
    })
}

/// `{company_id}` chooses where, not what.
fn is_scope_id(segment: &str) -> bool {
    segment == "{company_id}"
}

/// The success line of a write: `Issued invoice 2026-0042 (1 220.00 EUR) · <link>`.
///
/// The object is named by what a person knows it by: the title of the object read back (`item`),
/// else the number or name of the answer, else the name of the request body, and only then its
/// short id. The key amount of the object read back follows in brackets.
pub fn success_message(
    operation: &Command,
    value: &Value,
    item: Option<&(&'static View, Value)>,
    body: Option<&Value>,
    link: Option<&str>,
) -> Option<String> {
    let verb = match operation.method {
        "issue" => "Issued",
        "cancel" => "Cancelled",
        "record" => "Recorded",
        "revise" => "Revised",
        "create" => "Created",
        "rotate" => "Rotated",
        "upload" => "Uploaded",
        _ => return None,
    };
    let result = value
        .get("result")
        .filter(|result| result.is_object())
        .unwrap_or(value);
    let named = |object: Option<&Value>| {
        ["number", "name", "filename", "address"]
            .iter()
            .find_map(|key| object?.get(*key).and_then(Value::as_str))
            .filter(|text| !text.is_empty())
            .map(crate::render::format::one_line)
    };
    let short = |object: Option<&Value>| {
        object?.as_object()?.iter().find_map(|(key, id)| {
            (key == "id" || key.ends_with("_id"))
                .then(|| id.as_str().map(crate::render::format::short_id))
                .flatten()
        })
    };
    let item_object = item.and_then(|(_, object)| object.as_object());
    let title = item.and_then(|(view, object)| {
        object
            .get(view.title)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(crate::render::format::one_line)
    });
    let name = title
        .or_else(|| named(Some(result)))
        .or_else(|| named(body))
        .or_else(|| short(item.map(|(_, object)| object)))
        .or_else(|| short(Some(result)));
    let amount = item.zip(item_object).and_then(|((view, _), object)| {
        let field = view
            .columns
            .iter()
            .filter_map(|name| view.field(name))
            .find(|field| matches!(field.kind, crate::generated::FieldKind::Money { .. }))?;
        let crate::generated::FieldKind::Money { currency_field } = field.kind else {
            return None;
        };
        let minor = object.get(field.name)?.as_i64()?;
        let currency = object.get(currency_field).and_then(Value::as_str);
        Some(crate::render::format::money_text(
            i128::from(minor),
            currency,
        ))
    });
    let mut message = format!("{verb} {}", singular(operation.resource));
    if let Some(name) = name {
        message.push(' ');
        message.push_str(&name);
    }
    if let Some(amount) = amount {
        message.push_str(&format!(" ({amount})"));
    }
    if value.get("replayed").and_then(Value::as_bool) == Some(true) {
        message.push_str(" (already done: the server replayed the first answer)");
    }
    if let Some(link) = link {
        let dot = crate::render::theme::symbol(crate::render::theme::Sym::Middot);
        message.push_str(&format!(
            " {dot} {}",
            crate::render::theme::link(link, link)
        ));
    }
    Some(message)
}

/// What the empty state and the footer of a list say.
fn list_info(operation: &Command, call: &Call) -> ListInfo {
    let create = crate::generated::COMMANDS
        .iter()
        .find(|other| {
            other.resource == operation.resource
                && matches!(other.method, "create" | "record" | "upload")
        })
        .map(|other| {
            let tail = match other.body {
                Body::Json { .. } => " --data @file.json",
                Body::Multipart { .. } => " <PATH>",
                Body::None => "",
            };
            format!("oscar {} {}{tail}", other.resource, other.name)
        });
    ListInfo {
        noun: operation.resource.replace('-', " "),
        filtered: call
            .query
            .iter()
            .any(|(name, _)| !matches!(name.as_str(), "cursor" | "limit" | "ordering")),
        create,
        link: None,
    }
}

/// The shortest id an argument may be and still be taken as a prefix.
const MIN_PREFIX: usize = 4;

/// The pages read to resolve a prefix, at most.
const PREFIX_PAGES: usize = 20;

/// Each id of the path that is not a whole id (`get`, `issue`, `cancel`, ...): find the one item
/// of the list of that place that it names. A value names an item by the start or the end of its
/// id (the short form of `-o wide` is the end of a ULID), by the start of that short form, or by
/// the title a person sees in the list (an invoice number, a file name). No match: the value goes
/// to the server as it is.
async fn resolve_ids(client: &Client, operation: &'static Command, call: &mut Call) -> Result<()> {
    let template: Vec<&str> = operation.path.trim_end_matches('/').split('/').collect();
    for (index, segment) in template.iter().enumerate() {
        if !segment.starts_with('{') || is_scope_id(segment) {
            continue;
        }
        let list_template = template[..index].join("/") + "/";
        let Some(list) = crate::generated::COMMANDS
            .iter()
            .find(|other| other.paginated && other.path == list_template)
        else {
            continue;
        };
        resolve_one(client, list, call, index).await?;
    }
    Ok(())
}

/// An id value that may be the start or the end of an id: letters and digits, shorter than a ULID.
fn is_partial_id(given: &str) -> bool {
    (MIN_PREFIX..26).contains(&given.len())
        && given.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// Whether `id` is named by `wanted` (upper case): its start, its end, or the start of its short
/// form.
fn id_matches(id: &str, wanted: &str) -> bool {
    let upper = id.to_ascii_uppercase();
    upper.starts_with(wanted)
        || upper.ends_with(wanted)
        || crate::render::format::short_id(&upper).starts_with(wanted)
}

/// Resolve the id at the segment `index` of the path of `call` against the list `list`.
async fn resolve_one(
    client: &Client,
    list: &'static Command,
    call: &mut Call,
    index: usize,
) -> Result<()> {
    let trailing = if call.path.ends_with('/') { "/" } else { "" };
    let mut segments: Vec<String> = call
        .path
        .trim_end_matches('/')
        .split('/')
        .map(str::to_owned)
        .collect();
    let Some(encoded) = segments.get(index).cloned() else {
        return Ok(());
    };
    let given = percent_encoding::percent_decode_str(&encoded)
        .decode_utf8_lossy()
        .into_owned();
    let partial = is_partial_id(&given);
    // The title of a row (`number`, `name`), when it is not an id.
    let title = list
        .view
        .map(|view| view.title)
        .filter(|title| !title.is_empty() && *title != "id");
    if given.len() >= 26 || given.is_empty() || (!partial && title.is_none()) {
        return Ok(());
    }
    let wanted = given.to_ascii_uppercase();
    // The server filters the title exactly: an invoice number typed in lower case is tried in
    // upper case too, as invoice numbers are written.
    let mut filters = vec![given.clone()];
    if wanted != given {
        filters.push(wanted.clone());
    }
    let mut by_id: Vec<(String, String)> = Vec::new();
    let mut by_title: Vec<(String, String)> = Vec::new();
    // The scan stopped at `PREFIX_PAGES` before the end of the list: a later row may match too.
    let mut truncated = false;
    for filter in &filters {
        let mut query = vec![("limit".to_owned(), "200".to_owned())];
        // The list filters on the title (`--number`): one small page, unless the value may be an id.
        if let Some(title) = title.filter(|_| !partial)
            && list.flags.iter().any(|flag| flag.name == title)
        {
            query.push((title.to_owned(), filter.clone()));
        }
        let list_call = Call {
            method: HttpMethod::Get,
            path: segments[..index].join("/") + "/",
            query,
            body: None,
            idempotency_key: None,
        };
        let mut cursor: Option<String> = None;
        let mut ended = false;
        for _ in 0..PREFIX_PAGES {
            let page: Value = send(client, list, &list_call, cursor.as_deref())
                .await?
                .json()?;
            for item in page
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(id) = item.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let name = title
                    .and_then(|title| item.get(title))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let entry = (id.to_owned(), name.to_owned());
                if title.is_some() && name.eq_ignore_ascii_case(given.trim()) {
                    if !by_title.contains(&entry) {
                        by_title.push(entry);
                    }
                } else if partial && id_matches(id, &wanted) && !by_id.contains(&entry) {
                    by_id.push(entry);
                }
            }
            match page.get("next_cursor").and_then(Value::as_str) {
                Some(next) if !next.is_empty() => cursor = Some(next.to_owned()),
                _ => {
                    ended = true;
                    break;
                }
            }
        }
        truncated |= !ended;
        let filtered = title.is_some_and(|title| list.flags.iter().any(|flag| flag.name == title));
        if !by_title.is_empty() || partial || !filtered {
            break;
        }
    }
    // A title is what a person sees: it wins over the end of an id.
    let titled = !by_title.is_empty();
    let matches = if titled { by_title } else { by_id };
    // A unique match of the scanned part is final only when it is the whole id or the title given.
    if truncated
        && !titled
        && matches.len() == 1
        && !matches[0].0.eq_ignore_ascii_case(given.trim())
    {
        return Err(CliError::usage(format!(
            "`{given}` cannot be resolved: the list has more than {} items. Give the whole id.",
            PREFIX_PAGES * 200
        )));
    }
    match matches.as_slice() {
        [] => Ok(()),
        [(one, _)] => {
            segments[index] = utf8_percent_encode(one, PATH_VALUE).to_string();
            call.path = segments.join("/") + trailing;
            Ok(())
        }
        many => {
            let mut error = CliError::usage(format!(
                "`{given}` matches {} items. Give more of the id.",
                many.len()
            ));
            error.details.extend(many.iter().take(10).map(|(id, name)| {
                if name.is_empty() {
                    id.clone()
                } else {
                    format!("{id}  {}", crate::render::format::one_line(name))
                }
            }));
            Err(error)
        }
    }
}

/// A path value that would change the path: empty, `.` or `..` (the URL takes them as a move up
/// or a no-op), or with a `/` or `\` (a proxy may decode `%2F` back into a separator).
fn check_path_value(name: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
    {
        return Err(CliError::usage(format!(
            "`{value}` is not a valid {}: an id has no `/` or `\\`, and is not `.` or `..`.",
            name.to_uppercase()
        )));
    }
    Ok(())
}

/// Build the call from the arguments: the path values, the checked flags, and the body. A list
/// with an `ordering` and none given asks for the newest first ([`default_ordering`]).
pub fn build(
    app: &App,
    operation: &'static Command,
    leaf: &ArgMatches,
    selection: &Selection,
) -> Result<Call> {
    let mut path = operation.path.to_owned();
    for arg in operation.args {
        let value = match arg.name {
            "company_id" => app.company(company_flag(app, leaf)?.as_deref(), selection)?,
            name => {
                let value = string(leaf, &path_id(name))
                    .ok_or_else(|| CliError::usage(format!("Give {}.", name.to_uppercase())))?;
                check_path_value(name, value)?;
                value.to_owned()
            }
        };
        let encoded = utf8_percent_encode(&value, PATH_VALUE).to_string();
        path = path.replace(&format!("{{{}}}", arg.name), &encoded);
    }
    let mut query = Vec::new();
    for item in operation.flags {
        if let Some(value) = string(leaf, &format!("query:{}", item.name)) {
            check_value(item.long, item.kind, value)?;
            query.push((item.name.to_owned(), value.to_owned()));
        }
    }
    if let Some(default) = default_ordering(operation)
        && !query.iter().any(|(name, _)| name == "ordering")
    {
        query.push(("ordering".to_owned(), default.to_owned()));
    }
    let body = match operation.body {
        Body::None => None,
        Body::Json { .. } => string(leaf, "data").map(read_data).transpose()?,
        Body::Multipart { .. } => {
            return Err(CliError::usage(format!(
                "`oscar {} {}` takes a file; send it with `oscar api`.",
                operation.resource, operation.method
            )));
        }
    };
    Ok(Call {
        method: method(operation.http_method)?,
        path,
        query,
        body,
        idempotency_key: string(leaf, "idempotency-key").map(str::to_owned),
    })
}

/// Check the value of a flag against its kind. An enum value the CLI does not know is passed on:
/// the server may know it.
pub fn check_value(long: &str, kind: ValueKind, value: &str) -> Result<()> {
    let valid = match kind {
        ValueKind::Integer => value.parse::<i64>().is_ok(),
        ValueKind::Number | ValueKind::Decimal => value.parse::<f64>().is_ok_and(f64::is_finite),
        ValueKind::Boolean => matches!(value, "true" | "false"),
        ValueKind::Date => is_date(value),
        ValueKind::DateTime => time::parse(value).is_some(),
        ValueKind::Email => value.contains('@'),
        ValueKind::Enum(_) | ValueKind::String | ValueKind::File => true,
    };
    if valid {
        Ok(())
    } else {
        let expected = match kind {
            ValueKind::Integer => "a whole number",
            ValueKind::Number | ValueKind::Decimal => "a number",
            ValueKind::Boolean => "true or false",
            ValueKind::Date => "a date, YYYY-MM-DD",
            ValueKind::DateTime => "a date and time, RFC 3339",
            _ => "an email address",
        };
        Err(CliError::usage(format!(
            "--{long} must be {expected}, not `{value}`."
        )))
    }
}

fn is_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
        && time::parse(&format!("{value}T00:00:00Z")).is_some()
}

/// Send one page of the call, with `cursor` when given.
async fn send(
    client: &Client,
    operation: &Command,
    call: &Call,
    cursor: Option<&str>,
) -> Result<oscaraccountant::RawResponse> {
    let mut request = client.request(call.method, call.path.clone());
    for (name, value) in &call.query {
        if cursor.is_none() || name != "cursor" {
            request = request.query(name.clone(), value.clone());
        }
    }
    if let Some(cursor) = cursor {
        request = request.query("cursor", cursor);
    }
    if operation.response == Response::Binary {
        request = request.accept("*/*");
    }
    if let Some(body) = &call.body {
        request = request.json(body)?;
    }
    if operation.idempotent {
        request = match &call.idempotency_key {
            Some(key) => request.idempotency_key(key.clone()),
            None => request.idempotent(),
        };
    }
    Ok(request.send().await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli;
    use crate::generated::COMMANDS;
    use crate::session::tests::app;

    fn operation(resource: &str, method: &str) -> &'static Command {
        COMMANDS
            .iter()
            .find(|c| c.resource == resource && c.method == method)
            .unwrap()
    }

    fn leaf(args: &[&str]) -> ArgMatches {
        let matches = cli::command().try_get_matches_from(args).unwrap();
        super::super::leaf(&matches).clone()
    }

    /// A list page of `ids`, with a next page when `more`.
    fn page(ids: &[&str], more: bool) -> String {
        let items: Vec<Value> = ids.iter().map(|id| json!({"id": id})).collect();
        json!({"items": items, "next_cursor": if more { "next" } else { "" }}).to_string()
    }

    async fn resolve(answers: &[String]) -> Result<String> {
        let script = crate::oauth::tests::Script::new(
            &answers
                .iter()
                .map(|body| (200, body.as_str()))
                .collect::<Vec<_>>(),
        );
        let client = Client::builder()
            .base_url("https://oscar.test")
            .api_key("osk_test")
            .env(|_| None)
            .max_retries(0)
            .transport(script)
            .build()
            .unwrap();
        let mut call = Call {
            method: HttpMethod::Get,
            path: "/api/companies/c1/invoices/ABCD1234/".into(),
            query: Vec::new(),
            body: None,
            idempotency_key: None,
        };
        resolve_one(&client, operation("invoices", "list"), &mut call, 5).await?;
        Ok(call.path)
    }

    #[tokio::test]
    async fn a_prefix_beyond_the_scanned_pages_is_refused() {
        // The only match of the first pages is not final while the list goes on.
        let mut answers = vec![page(
            &["01J90000000000000000000001", "01J900000000000000ABCD1234"],
            true,
        )];
        answers.extend((1..PREFIX_PAGES).map(|_| page(&[], true)));
        let error = resolve(&answers).await.unwrap_err();
        assert_eq!(error.code, crate::error::EXIT_USAGE, "{}", error.message);
        assert!(
            error.message.contains("Give the whole id"),
            "{}",
            error.message
        );
        // No match in the scanned pages: the value goes to the server as it is, as before.
        let answers: Vec<String> = (0..PREFIX_PAGES).map(|_| page(&[], true)).collect();
        assert_eq!(
            resolve(&answers).await.unwrap(),
            "/api/companies/c1/invoices/ABCD1234/"
        );
        // The list ends within the pages: the one match is taken.
        let answers = vec![
            page(&["01J900000000000000ABCD1234"], true),
            page(&[], false),
        ];
        assert_eq!(
            resolve(&answers).await.unwrap(),
            "/api/companies/c1/invoices/01J900000000000000ABCD1234/"
        );
    }

    #[tokio::test]
    async fn a_title_beyond_the_scanned_pages_is_taken() {
        // The scan stops early, but the value is the whole title of one row: it is final.
        let row = json!({"items": [{"id": "01J900000000000000ZZZZ9999", "number": "ABCD1234"}], "next_cursor": "next"});
        let mut answers = vec![row.to_string()];
        answers.extend((1..PREFIX_PAGES).map(|_| page(&[], true)));
        assert_eq!(
            resolve(&answers).await.unwrap(),
            "/api/companies/c1/invoices/01J900000000000000ZZZZ9999/"
        );
    }

    #[test]
    fn every_page_keeps_the_fields_of_the_first_page() {
        let first = json!({"items": [{"id": "a"}], "next_cursor": "c2",
                           "coverage": {"accounts": [], "checked_from": null}});
        let page = every_page(vec![json!({"id": "a"}), json!({"id": "b"})], Some(&first));
        assert_eq!(
            page,
            json!({"items": [{"id": "a"}, {"id": "b"}], "next_cursor": null,
                   "coverage": {"accounts": [], "checked_from": null}})
        );
        // An older server: the page as before.
        let old = json!({"items": [], "next_cursor": null});
        assert_eq!(
            every_page(Vec::new(), Some(&old)).to_string(),
            r#"{"items":[],"next_cursor":null}"#
        );
        assert_eq!(
            every_page(Vec::new(), None).to_string(),
            r#"{"items":[],"next_cursor":null}"#
        );
    }

    #[test]
    fn builds_a_call() {
        let dir = tempfile::tempdir().unwrap();
        // A key: with no credential at all the error is "Not logged in", not "No company".
        let app = app(dir.path(), &[("OSCAR_API_KEY", "osk_test")]);
        let selection = app.select(&crate::config::Config::default()).unwrap();
        let args = leaf(&[
            "oscar",
            "invoices",
            "list",
            "--company",
            "c/1 x",
            "--state",
            "issued",
            "--limit",
            "5",
        ]);
        let call = build(&app, operation("invoices", "list"), &args, &selection).unwrap();
        assert_eq!(call.method, HttpMethod::Get);
        assert_eq!(call.path, "/api/companies/c%2F1%20x/invoices/");
        assert!(call.query.contains(&("state".into(), "issued".into())));
        assert!(call.query.contains(&("limit".into(), "5".into())));

        let args = leaf(&[
            "oscar",
            "invoices",
            "list",
            "--limit",
            "five",
            "--company",
            "c",
        ]);
        let error = build(&app, operation("invoices", "list"), &args, &selection).unwrap_err();
        assert_eq!(error.message, "--limit must be a whole number, not `five`.");

        let args = leaf(&["oscar", "invoices", "list"]);
        let error = build(&app, operation("invoices", "list"), &args, &selection).unwrap_err();
        assert!(error.message.starts_with("No company."));

        let args = leaf(&[
            "oscar",
            "invoices",
            "cancel",
            "inv1",
            "--company",
            "c",
            "--data",
            r#"{"reason": "x"}"#,
            "--idempotency-key",
            "k1",
        ]);
        let call = build(&app, operation("invoices", "cancel"), &args, &selection).unwrap();
        assert_eq!(call.path, "/api/companies/c/invoices/inv1/cancel");
        assert_eq!(call.body, Some(json!({"reason": "x"})));
        assert_eq!(call.idempotency_key.as_deref(), Some("k1"));

        let args = leaf(&["oscar", "companies", "list"]);
        let call = build(&app, operation("companies", "list"), &args, &selection).unwrap();
        assert_eq!(call.path, "/api/companies/");
    }

    #[test]
    fn a_list_asks_for_the_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path(), &[("OSCAR_API_KEY", "osk_test")]);
        let selection = app.select(&crate::config::Config::default()).unwrap();
        let ordering = |call: &Call| {
            call.query
                .iter()
                .filter(|(name, _)| name == "ordering")
                .map(|(_, value)| value.clone())
                .collect::<Vec<_>>()
        };
        // No --ordering: the descending value.
        let args = leaf(&["oscar", "invoices", "list", "-c", "c"]);
        let call = build(&app, operation("invoices", "list"), &args, &selection).unwrap();
        assert_eq!(ordering(&call), ["-invoice_date"]);
        let args = leaf(&["oscar", "documents", "list", "-c", "c"]);
        let call = build(&app, operation("documents", "list"), &args, &selection).unwrap();
        assert_eq!(ordering(&call), ["-created_at"]);
        let args = leaf(&["oscar", "bank-transactions", "list", "-c", "c"]);
        let call = build(
            &app,
            operation("bank-transactions", "list"),
            &args,
            &selection,
        )
        .unwrap();
        assert_eq!(ordering(&call), ["-booking_date"]);
        // An explicit --ordering wins, once.
        let args = leaf(&[
            "oscar",
            "invoices",
            "list",
            "-c",
            "c",
            "--ordering",
            "invoice_date",
        ]);
        let call = build(&app, operation("invoices", "list"), &args, &selection).unwrap();
        assert_eq!(ordering(&call), ["invoice_date"]);
        // An operation with no ordering sends none.
        let args = leaf(&["oscar", "accounts", "list", "-c", "c"]);
        let call = build(&app, operation("accounts", "list"), &args, &selection).unwrap();
        assert!(ordering(&call).is_empty());
        let args = leaf(&["oscar", "companies", "list"]);
        let call = build(&app, operation("companies", "list"), &args, &selection).unwrap();
        assert!(call.query.is_empty());
        assert_eq!(call.path, "/api/companies/");
        // The ordering does not make a list "filtered".
        let args = leaf(&["oscar", "invoices", "list", "-c", "c"]);
        let call = build(&app, operation("invoices", "list"), &args, &selection).unwrap();
        assert!(!list_info(operation("invoices", "list"), &call).filtered);
    }

    #[test]
    fn the_company_of_the_command_line() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(dir.path(), &[("OSCAR_API_KEY", "osk_test")]);
        let selection = app.select(&crate::config::Config::default()).unwrap();
        let list = operation("invoices", "list");
        // The root's -c (in the globals) when the command has none.
        app.globals.company = Some("root".into());
        let args = leaf(&["oscar", "invoices", "list"]);
        let call = build(&app, list, &args, &selection).unwrap();
        assert_eq!(call.path, "/api/companies/root/invoices/");
        // The command's -c wins.
        let args = leaf(&["oscar", "invoices", "list", "-c", "leaf"]);
        let call = build(&app, list, &args, &selection).unwrap();
        assert_eq!(call.path, "/api/companies/leaf/invoices/");
        app.globals.company = None;
        // `companies get <COMPANY>`.
        let get = operation("companies", "retrieve");
        let args = leaf(&["oscar", "companies", "get", "c7"]);
        assert_eq!(company_flag(&app, &args).unwrap().as_deref(), Some("c7"));
        let call = build(&app, get, &args, &selection).unwrap();
        assert_eq!(call.path, "/api/companies/c7/");
        // The same value twice is fine; two values are a usage error.
        let args = leaf(&["oscar", "companies", "get", "c7", "-c", "c7"]);
        assert_eq!(company_flag(&app, &args).unwrap().as_deref(), Some("c7"));
        let args = leaf(&["oscar", "companies", "get", "c7", "-c", "c8"]);
        let error = company_flag(&app, &args).unwrap_err();
        assert_eq!(error.code, crate::error::EXIT_USAGE);
        assert_eq!(
            error.message,
            "Give the company once: `c7` or -c `c8`, not both."
        );
        assert!(build(&app, get, &args, &selection).is_err());
        // Also against the root's -c.
        app.globals.company = Some("c9".into());
        let args = leaf(&["oscar", "companies", "get", "c7"]);
        assert!(company_flag(&app, &args).is_err());
        let args = leaf(&["oscar", "companies", "get"]);
        assert_eq!(company_flag(&app, &args).unwrap().as_deref(), Some("c9"));
    }

    #[test]
    fn companies_list_needs_no_company() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path(), &[]);
        let selection = app.select(&crate::config::Config::default()).unwrap();
        let list = operation("companies", "list");
        assert!(list.args.is_empty());
        let call = build(
            &app,
            list,
            &leaf(&["oscar", "companies", "list"]),
            &selection,
        )
        .unwrap();
        assert_eq!(call.path, "/api/companies/");
        // The list of every company has no page of the web app.
        assert_eq!(web_link("https://o.test", list, &call.path), None);
        // `--organization` is gone.
        assert!(
            crate::cli::command()
                .try_get_matches_from(["oscar", "companies", "list", "--organization", "o1"])
                .is_err()
        );
    }

    #[test]
    fn checks_values() {
        let ok = [
            (ValueKind::Integer, "-3"),
            (ValueKind::Number, "1.5"),
            (ValueKind::Decimal, "10.00"),
            (ValueKind::Boolean, "true"),
            (ValueKind::Date, "2026-09-27"),
            (ValueKind::DateTime, "2026-09-27T10:00:00Z"),
            (ValueKind::Email, "a@b.test"),
            (ValueKind::Enum(&["a"]), "new"),
            (ValueKind::String, "anything"),
        ];
        for (kind, value) in ok {
            check_value("x", kind, value).unwrap();
        }
        let bad = [
            (ValueKind::Integer, "1.5", "a whole number"),
            (ValueKind::Number, "NaN", "a number"),
            (ValueKind::Boolean, "yes", "true or false"),
            (ValueKind::Date, "2026-9-27", "a date"),
            (ValueKind::Date, "2026-02-30x", "a date"),
            (ValueKind::Date, "2026-13-01", "a date"),
            (ValueKind::Date, "2026a09-01", "a date"),
            (ValueKind::DateTime, "2026-09-27", "a date and time"),
            (ValueKind::Email, "nobody", "an email address"),
        ];
        for (kind, value, expected) in bad {
            let error = check_value("x", kind, value).unwrap_err();
            assert!(error.message.contains(expected), "{}", error.message);
            assert_eq!(error.code, crate::error::EXIT_USAGE);
        }
    }

    #[test]
    fn a_multipart_operation_needs_its_own_command() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path(), &[]);
        let selection = app.select(&crate::config::Config::default()).unwrap();
        let args = leaf(&["oscar", "files", "upload", "x.pdf", "--company", "c"]);
        let error = build(&app, operation("files", "upload"), &args, &selection).unwrap_err();
        assert!(error.message.contains("oscar api"));
    }

    #[test]
    fn success_lines_links_and_names() {
        let issue = operation("invoices", "issue");
        let link = web_link(
            "https://o.test/",
            issue,
            "/api/companies/c1/invoices/i1/issue",
        );
        assert_eq!(link.as_deref(), Some("https://o.test/c/c1/invoices"));
        assert_eq!(
            web_link("https://o.test", operation("me", "retrieve"), "/api/me/"),
            None
        );
        let tb = operation("trial-balance", "retrieve");
        assert_eq!(
            web_link("h", tb, "/api/companies/c/trial-balance/"),
            Some("h/c/c/trial-balance".into())
        );
        let value = json!({"command_id": "x", "replayed": true, "result": {"number": "2026-1"}});
        let message = success_message(issue, &value, None, None, None).unwrap();
        assert_eq!(
            message,
            "Issued invoice 2026-1 (already done: the server replayed the first answer)"
        );
        let value = json!({"result": {"party_id": "01J90000000000000000000179"}});
        let create = operation("parties", "create");
        assert_eq!(
            success_message(create, &value, None, None, Some("u")).unwrap(),
            "Created party 00000179 · u"
        );
        assert_eq!(
            success_message(operation("invoices", "list"), &value, None, None, None),
            None
        );
        assert_eq!(singular("bank-transactions"), "bank transaction");
    }

    #[test]
    fn a_success_line_names_the_object_and_its_amount() {
        // Regression: the short id even when the number or the name was known.
        let cancel = operation("invoices", "cancel");
        let invoice = json!({"id": "01J90000000000000000000116", "number": "EE-2026-006",
                             "gross_minor": 372_000, "currency": "EUR"});
        let view = operation("invoices", "retrieve").view.unwrap();
        let value = json!({"command_id": "x", "result": {}});
        assert_eq!(
            success_message(cancel, &value, Some(&(view, invoice)), None, None).unwrap(),
            "Cancelled invoice EE-2026-006 (3 720.00 EUR)"
        );
        let draft = json!({"id": "01J90000000000000000000116", "number": null,
                           "gross_minor": 124_000, "currency": "EUR"});
        assert_eq!(
            success_message(cancel, &value, Some(&(view, draft)), None, None).unwrap(),
            "Cancelled invoice 00000116 (1 240.00 EUR)"
        );
        let create = operation("parties", "create");
        let value = json!({"result": {"party_id": "01J90000000000000000000179"}});
        let body = json!({"name": "Echo Robotics OÜ"});
        assert_eq!(
            success_message(create, &value, None, Some(&body), None).unwrap(),
            "Created party Echo Robotics OÜ"
        );
    }

    #[test]
    fn the_object_of_a_write_is_read_back_from_its_get() {
        let issue = operation("invoices", "issue");
        let (get, path) =
            item_call(issue, "/api/companies/c1/invoices/01J9X/issue", &json!({})).unwrap();
        assert_eq!((get.resource, get.http_method), ("invoices", "GET"));
        assert_eq!(path, "/api/companies/c1/invoices/01J9X/");
        let record = operation("invoices", "record");
        let (get, path) = item_call(
            record,
            "/api/companies/c1/invoices/",
            &json!({"result": {"invoice_id": "01J9 Y"}}),
        )
        .unwrap();
        assert_eq!(get.resource, "invoices");
        assert_eq!(path, "/api/companies/c1/invoices/01J9%20Y/");
        assert!(item_call(record, "/api/companies/c1/invoices/", &json!({})).is_none());
        assert_eq!(singular("inbox"), "inbox");
    }

    #[test]
    fn a_path_value_never_walks_out_of_its_segment() {
        // Regression: `oscar invoices get ../../me` reached `/api/companies/me/`.
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path(), &[("OSCAR_API_KEY", "osk_test")]);
        let selection = app.select(&crate::config::Config::default()).unwrap();
        for bad in ["../../me", "..", ".", "a/b", r"a\b", "%2e%2e/x"] {
            let args = leaf(&["oscar", "invoices", "get", bad, "--company", "c"]);
            let error =
                build(&app, operation("invoices", "retrieve"), &args, &selection).unwrap_err();
            assert_eq!(error.code, crate::error::EXIT_USAGE, "{bad}");
            assert!(
                error.message.contains("is not a valid INVOICE_ID"),
                "{}",
                error.message
            );
        }
        let args = leaf(&["oscar", "invoices", "get", "a?b#c%d ..x", "--company", "c"]);
        let call = build(&app, operation("invoices", "retrieve"), &args, &selection).unwrap();
        assert_eq!(call.path, "/api/companies/c/invoices/a%3Fb%23c%25d%20..x/");
        assert!(check_path_value("x", "").is_err());
    }

    #[test]
    fn what_names_an_id() {
        // Regression: the start of the short form (`Z6AASA` of `...Z6AASAST`) failed.
        let id = "01M4P0QQSGEKXXXXXXZ6AASAST";
        for given in ["01M4P0", "Z6AASAST", "AASAST", "Z6AASA", "z6aasa"] {
            assert!(id_matches(id, &given.to_ascii_uppercase()), "{given}");
        }
        assert!(!id_matches(id, "XYZ123"));
        assert!(is_partial_id("Z6AASA"));
        assert!(!is_partial_id("EE-2026-007"));
        assert!(!is_partial_id("abc"));
        assert!(!is_partial_id(id));
    }
}
