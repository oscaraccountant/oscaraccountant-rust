//! The Rust SDK of the Oscar accounting API.
//!
//! ```no_run
//! # async fn run() -> Result<(), oscaraccountant::Error> {
//! use futures::TryStreamExt;
//! use oscaraccountant::Client;
//!
//! let oscar = Client::from_env()?; // OSCAR_API_KEY, OSCAR_BASE_URL
//! let mut invoices = oscar.invoices().list("company-id").state("issued").stream();
//! while let Some(invoice) = invoices.try_next().await? {
//!     println!("{} {}", invoice.id, invoice.state);
//! }
//! # Ok(()) }
//! ```
//!
//! The behaviour (credentials, retries, idempotency, pagination, errors, files) follows
//! the shared rules of every Oscar client (`spec/README.md` of the repository
//! `oscaraccountant-rust`). The resources and the types are generated from the OpenAPI document
//! of the public API by `oscar-codegen`; they are in the module `generated` and re-exported here.

#[rustfmt::skip]
#[allow(missing_docs)]
mod generated;

mod cli_login;
mod client;
#[cfg(test)]
mod conformance;
mod credentials;
mod error;
mod files;
mod hooks;
mod pagination;
mod raw;
mod retry;
mod rfc3339;
mod transport;
mod urls;
mod user_agent;

pub use client::{Client, ClientBuilder, DEFAULT_MAX_RETRIES, DEFAULT_TIMEOUT};
pub use credentials::{BoxError, Credential, TokenProvider};
pub use error::{ApiError, AuthenticationError, Error};
pub use files::{Download, Upload};
pub use generated::*;
pub use pagination::{PageStream, Pages};
pub use raw::{RawRequest, RawResponse};
pub use transport::{
    ByteStream, HttpRequest, HttpResponse, HttpTransport, ReqwestTransport, TlsRoots,
    TransportError, reqwest_client, reqwest_client_builder,
};
pub use urls::{DEFAULT_BASE_URL, normalize_base_url};
pub use user_agent::{VERSION, user_agent};

/// The `reqwest` this crate is built on, for [`ReqwestTransport::from_client`].
pub use reqwest;
