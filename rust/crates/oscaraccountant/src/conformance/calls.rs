//! The call of a case: a generated method with the arguments of the case, or a request spec
//! handed to the client directly. See "The call" in `clients/spec/conformance/README.md`.

use std::time::Duration;

use bytes::Bytes;
use futures::TryStreamExt;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{
    Client, Error, HttpMethod, Page, PageStream, RequestBody, RequestOptions, RequestSpec,
    ResponseKind, Upload,
};

/// What a call gave.
#[derive(Debug)]
pub(super) enum Outcome {
    /// A JSON result, turned back into wire JSON.
    Json(Value),
    /// Every item of the iterator, each turned back into wire JSON.
    Items(Vec<Value>),
    /// No content.
    None,
    /// A download.
    Binary {
        bytes: Bytes,
        content_type: Option<String>,
        file_name: Option<String>,
    },
}

/// The value of a query parameter, typed as its setter takes it.
macro_rules! typed {
    (bool, $value:expr) => {
        $value.as_bool().expect("a boolean query value")
    };
    (int, $value:expr) => {
        $value.as_i64().expect("an integer query value")
    };
    (str, $value:expr) => {
        $value.as_str().expect("a string query value")
    };
}

/// Call the setter of each query parameter of the case. `null` means not set.
macro_rules! query {
    ($request:expr, $call:expr, { $($name:ident: $kind:ident),* $(,)? }) => {{
        let mut request = $request;
        for (name, value) in query_values($call) {
            request = match name {
                $(name if name == stringify!($name).trim_start_matches("r#") => {
                    request.$name(typed!($kind, value))
                })*
                other => panic!("the case sets the unknown query parameter `{other}`"),
            };
        }
        request
    }};
}

/// The options of the call: the timeout, the retries and the extra headers.
macro_rules! options {
    ($request:expr, $call:expr) => {{
        let mut request = $request;
        let options = &$call["options"];
        if let Some(milliseconds) = options["timeout_ms"].as_u64() {
            request = request.timeout(Duration::from_millis(milliseconds));
        }
        if let Some(retries) = options["max_retries"].as_u64() {
            request = request.max_retries(u32::try_from(retries).expect("a small number"));
        }
        for (name, value) in options["headers"].as_object().into_iter().flatten() {
            request = request.header(name, value.as_str().expect("a string header value"));
        }
        request
    }};
}

/// The options of the call, and the `Idempotency-Key` the caller chose.
macro_rules! keyed {
    ($request:expr, $call:expr) => {{
        let request = options!($request, $call);
        match $call["options"]["idempotency_key"].as_str() {
            Some(key) => request.idempotency_key(key),
            None => request,
        }
    }};
}

/// A list call: one page (`mode: page`, the default), or every item (`mode: all`).
macro_rules! list {
    ($request:expr, $call:expr) => {{
        let request = $request;
        if $call["mode"] == "all" {
            items(request.stream()).await
        } else {
            json(request.send().await)
        }
    }};
}

/// Play the call of a case.
pub(super) async fn call(client: &Client, call: &Value) -> Result<Outcome, Error> {
    if call.get("spec").is_some() {
        return spec_call(client, call).await;
    }
    let path = |name: &str| -> String {
        call["path"][name]
            .as_str()
            .unwrap_or_else(|| panic!("the case has no path parameter `{name}`"))
            .to_owned()
    };
    let operation = call["operation"].as_str().expect("an operation");
    match operation {
        "accounts.list" => list!(
            query!(options!(client.accounts().list(path("company_id")), call), call, {
                active: bool, control_kind: str, cursor: str, limit: int, search: str, r#type: str,
            }),
            call
        ),
        "bank_accounts.list" => list!(
            query!(options!(client.bank_accounts().list(path("company_id")), call), call, {
                active: bool, cursor: str, limit: int,
            }),
            call
        ),
        "bank_transactions.list" => list!(
            query!(options!(client.bank_transactions().list(path("company_id")), call), call, {
                bank_account_id: str, cursor: str, date_from: str, date_to: str, direction: str,
                has_document: bool, limit: int, ordering: str, party_id: str, statement_id: str,
                status: str, to_explain: bool,
            }),
            call
        ),
        "bank_transactions.retrieve" => json(
            options!(
                client
                    .bank_transactions()
                    .retrieve(path("company_id"), path("bank_transaction_id")),
                call
            )
            .send()
            .await,
        ),
        "companies.list" => list!(
            query!(options!(client.companies().list(path("organization_id")), call), call, {
                cursor: str, limit: int,
            }),
            call
        ),
        "companies.retrieve" => json(
            options!(client.companies().retrieve(path("company_id")), call)
                .send()
                .await,
        ),
        "dimensions.list" => list!(
            query!(options!(client.dimensions().list(path("company_id")), call), call, {
                active: bool, cursor: str, kind: str, limit: int,
            }),
            call
        ),
        "documents.list" => list!(
            query!(options!(client.documents().list(path("company_id")), call), call, {
                cursor: str, kind: str, limit: int, ordering: str, received_from: str,
                received_to: str, referenced: bool, sha256: str,
            }),
            call
        ),
        "documents.retrieve" => json(
            options!(
                client
                    .documents()
                    .retrieve(path("company_id"), path("document_id")),
                call
            )
            .send()
            .await,
        ),
        "files.download" => {
            let download = options!(
                client.files().download(path("company_id"), path("file_id")),
                call
            )
            .send()
            .await?;
            let content_type = download.content_type().map(str::to_owned);
            let file_name = download.file_name().map(str::to_owned);
            Ok(Outcome::Binary {
                bytes: download.bytes().await?,
                content_type,
                file_name,
            })
        }
        "files.list" => list!(
            query!(options!(client.files().list(path("company_id")), call), call, {
                cursor: str, kind: str, limit: int,
            }),
            call
        ),
        "files.retrieve" => json(
            options!(
                client.files().retrieve(path("company_id"), path("file_id")),
                call
            )
            .send()
            .await,
        ),
        "files.upload" => {
            let multipart = &call["multipart"];
            let file = &multipart["file"];
            assert_eq!(file["field"], "file", "the file field of files.upload");
            let mut upload = Upload::new(super::decode_base64(
                file["content_base64"].as_str().expect("the file content"),
            ));
            if let Some(name) = file["filename"].as_str() {
                upload = upload.file_name(name);
            }
            if let Some(content_type) = file["content_type"].as_str() {
                upload = upload.content_type(content_type);
            }
            let mut request = keyed!(client.files().upload(path("company_id"), upload), call);
            for (name, value) in multipart["fields"].as_object().into_iter().flatten() {
                let value = value.as_str().expect("a text field");
                request = match name.as_str() {
                    "kind" => request.kind(value),
                    "received_on" => request.received_on(value),
                    "bank_transaction_id" => request.bank_transaction_id(value),
                    other => panic!("the case sets the unknown upload field `{other}`"),
                };
            }
            json(request.send().await)
        }
        "general_ledger.retrieve" => json(
            query!(options!(client.general_ledger().retrieve(path("company_id")), call), call, {
                account_code: str, date_from: str, date_to: str,
            })
            .send()
            .await,
        ),
        "inbox.list_emails" => list!(
            query!(options!(client.inbox().list_emails(path("company_id")), call), call, {
                cursor: str, limit: int,
            }),
            call
        ),
        "inbox.retrieve" => json(
            options!(client.inbox().retrieve(path("company_id")), call)
                .send()
                .await,
        ),
        "inbox.rotate" => json(
            options!(client.inbox().rotate(path("company_id")), call)
                .send()
                .await,
        ),
        "invoices.cancel" => json(
            keyed!(
                client
                    .invoices()
                    .cancel(path("company_id"), path("invoice_id"), body(call)),
                call
            )
            .send()
            .await,
        ),
        "invoices.issue" => {
            let mut request = keyed!(
                client
                    .invoices()
                    .issue(path("company_id"), path("invoice_id")),
                call
            );
            if call.get("body").is_some() {
                request = request.body(body(call));
            }
            json(request.send().await)
        }
        "invoices.list" => list!(
            query!(options!(client.invoices().list(path("company_id")), call), call, {
                cursor: str, date_from: str, date_to: str, direction: str, kind: str, limit: int,
                needs_review: bool, number: str, ordering: str, party_id: str, settlement: str,
                state: str,
            }),
            call
        ),
        "invoices.record" => json(
            keyed!(
                client.invoices().record(path("company_id"), body(call)),
                call
            )
            .send()
            .await,
        ),
        "invoices.retrieve" => json(
            options!(
                client
                    .invoices()
                    .retrieve(path("company_id"), path("invoice_id")),
                call
            )
            .send()
            .await,
        ),
        "invoices.revise" => json(
            keyed!(
                client
                    .invoices()
                    .revise(path("company_id"), path("invoice_id"), body(call)),
                call
            )
            .send()
            .await,
        ),
        "me.retrieve" => json(options!(client.me().retrieve(), call).send().await),
        "open_items.retrieve" => json(
            query!(options!(client.open_items().retrieve(path("company_id")), call), call, {
                account_code: str, as_of: str, party_id: str,
            })
            .send()
            .await,
        ),
        "organizations.list" => list!(
            query!(options!(client.organizations().list(), call), call, {
                cursor: str, limit: int,
            }),
            call
        ),
        "organizations.retrieve" => json(
            options!(
                client.organizations().retrieve(path("organization_id")),
                call
            )
            .send()
            .await,
        ),
        "parties.create" => json(
            keyed!(
                client.parties().create(path("company_id"), body(call)),
                call
            )
            .send()
            .await,
        ),
        "parties.list" => list!(
            query!(options!(client.parties().list(path("company_id")), call), call, {
                cursor: str, include_merged: bool, kind: str, limit: int, reg_code: str,
                search: str, vat_number: str,
            }),
            call
        ),
        "parties.retrieve" => json(
            options!(
                client
                    .parties()
                    .retrieve(path("company_id"), path("party_id")),
                call
            )
            .send()
            .await,
        ),
        "periods.list" => list!(
            query!(options!(client.periods().list(path("company_id")), call), call, {
                cursor: str, date: str, fiscal_year: str, limit: int, state: str,
            }),
            call
        ),
        "reports.balance_sheet" => {
            let (call, date) = take_query(call, "date");
            json(
                query!(
                    options!(
                        client.reports().balance_sheet(path("company_id"), date),
                        &call
                    ),
                    &call,
                    { compare_date: str, schema: str, simplified: bool }
                )
                .send()
                .await,
            )
        }
        "reports.profit_and_loss" => {
            let (call, date_from) = take_query(call, "date_from");
            let (call, date_to) = take_query(&call, "date_to");
            json(
                query!(
                    options!(
                        client
                            .reports()
                            .profit_and_loss(path("company_id"), date_from, date_to),
                        &call
                    ),
                    &call,
                    { compare_from: str, compare_to: str, schema: str, simplified: bool }
                )
                .send()
                .await,
            )
        }
        "trial_balance.retrieve" => json(
            query!(options!(client.trial_balance().retrieve(path("company_id")), call), call, {
                date_from: str, date_to: str, period: str,
            })
            .send()
            .await,
        ),
        "vat_codes.list" => list!(
            query!(options!(client.vat_codes().list(path("company_id")), call), call, {
                active: bool, cursor: str, direction: str, limit: int, treatment: str,
            }),
            call
        ),
        "vat_preview.retrieve" => {
            let (call, period_from) = take_query(call, "period_from");
            let (call, period_to) = take_query(&call, "period_to");
            json(
                query!(
                    options!(
                        client.vat_preview().retrieve(
                            path("company_id"),
                            period_from,
                            period_to
                        ),
                        &call
                    ),
                    &call,
                    { form: str }
                )
                .send()
                .await,
            )
        }
        other => panic!("no runner call for the operation `{other}`"),
    }
}

/// The query parameters of the call that are set, by wire name.
fn query_values(call: &Value) -> Vec<(&str, &Value)> {
    call["query"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, value)| !value.is_null())
        .map(|(name, value)| (name.as_str(), value))
        .collect()
}

/// A required query parameter, which the generated method takes as an argument: its value, and
/// the call without it.
fn take_query(call: &Value, name: &str) -> (Value, String) {
    let mut call = call.clone();
    let value = call["query"]
        .as_object_mut()
        .and_then(|query| query.remove(name))
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| panic!("the case has no required query parameter `{name}`"));
    (call, value)
}

/// The JSON body of the case, read into the request type of the operation.
fn body<T: DeserializeOwned>(call: &Value) -> T {
    serde_json::from_value(call["body"].clone()).expect("a body of the request type")
}

fn json<T: Serialize>(result: Result<T, Error>) -> Result<Outcome, Error> {
    Ok(Outcome::Json(
        serde_json::to_value(result?).expect("a result that serializes"),
    ))
}

async fn items<P>(stream: PageStream<P>) -> Result<Outcome, Error>
where
    P: Page + Send + 'static,
    P::Item: Serialize + Send + 'static,
{
    let items: Vec<P::Item> = stream.try_collect().await?;
    Ok(Outcome::Items(
        items
            .iter()
            .map(|item| serde_json::to_value(item).expect("an item that serializes"))
            .collect(),
    ))
}

/// A spec call: the request spec of the case, handed to the client.
async fn spec_call(client: &Client, call: &Value) -> Result<Outcome, Error> {
    let spec = &call["spec"];
    let options = &call["options"];
    let method = match spec["method"].as_str().expect("a method") {
        "GET" => HttpMethod::Get,
        "POST" => HttpMethod::Post,
        "PUT" => HttpMethod::Put,
        "PATCH" => HttpMethod::Patch,
        "DELETE" => HttpMethod::Delete,
        other => panic!("the case has the unknown method `{other}`"),
    };
    let response = match spec["response"].as_str().expect("a response kind") {
        "json" => ResponseKind::Json,
        "binary" => ResponseKind::Binary,
        "none" => ResponseKind::Empty,
        other => panic!("the case has the unknown response kind `{other}`"),
    };
    let request = RequestSpec {
        operation_id: leak(spec["operation_id"].as_str().expect("an operation id")),
        method,
        path: leak(spec["path"].as_str().expect("a path")),
        path_params: spec["path_params"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(name, value)| (leak(name), wire(value)))
            .collect(),
        query: query_values(spec)
            .into_iter()
            .map(|(name, value)| (leak(name), wire(value)))
            .collect(),
        body: spec.get("body").cloned().map(RequestBody::Json),
        idempotent: spec["idempotent"].as_bool().expect("idempotent"),
        idempotency_key: options["idempotency_key"].as_str().map(str::to_owned),
        response,
        options: RequestOptions {
            timeout: options["timeout_ms"].as_u64().map(Duration::from_millis),
            max_retries: options["max_retries"]
                .as_u64()
                .map(|retries| u32::try_from(retries).expect("a small number")),
            headers: options["headers"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(name, value)| (name.clone(), wire(value)))
                .collect(),
        },
    };
    match response {
        ResponseKind::Json => Ok(Outcome::Json(client.execute::<Value>(request).await?)),
        ResponseKind::Empty => client.execute_empty(request).await.map(|()| Outcome::None),
        ResponseKind::Binary => {
            let download = client.download(request).await?;
            let content_type = download.content_type().map(str::to_owned);
            let file_name = download.file_name().map(str::to_owned);
            Ok(Outcome::Binary {
                bytes: download.bytes().await?,
                content_type,
                file_name,
            })
        }
    }
}

/// A value as it goes on the wire: a string as it is, anything else as JSON text.
fn wire(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// A `&'static str` for the request spec, which holds the names of a generated operation.
fn leak(text: &str) -> &'static str {
    Box::leak(text.to_owned().into_boxed_str())
}
