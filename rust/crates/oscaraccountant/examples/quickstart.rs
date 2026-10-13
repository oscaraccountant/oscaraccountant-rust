//! The examples of the README, compiled by `cargo clippy --all-targets`.
//!
//! Run it against a development stack:
//! `OSCAR_BASE_URL=http://localhost:3500 OSCAR_API_KEY=osk_... cargo run --example quickstart`.

use futures::TryStreamExt;
use oscaraccountant::{BoxError, Client, Credential, HttpMethod, Upload};

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let oscar = Client::from_env()?; // reads OSCAR_API_KEY and OSCAR_BASE_URL

    let me = oscar.me().retrieve().send().await?;
    let Some(company) = me.companies.first() else {
        println!("no company");
        return Ok(());
    };
    let company_id = &company.id;

    // One page.
    let page = oscar
        .invoices()
        .list(company_id)
        .state("issued")
        .limit(50)
        .send()
        .await?;
    println!("{} invoices on the first page", page.items.len());

    // Every invoice of every page.
    let mut invoices = oscar.invoices().list(company_id).state("issued").stream();
    while let Some(invoice) = invoices.try_next().await? {
        println!("{} {}", invoice.id, invoice.state);
    }

    // An error by its status.
    match oscar
        .invoices()
        .retrieve(company_id, "no-such-invoice")
        .send()
        .await
    {
        Ok(invoice) => println!("{}", invoice.id),
        Err(error) if error.status() == Some(404) => println!("no such invoice"),
        Err(error) => return Err(error.into()),
    }

    // Files.
    if let Some(path) = std::env::args().nth(1) {
        let upload = Upload::from_path(&path)
            .await?
            .content_type("application/pdf");
        let document = oscar
            .files()
            .upload(company_id, upload)
            .kind("invoice")
            .send()
            .await?;
        let download = oscar
            .files()
            .download(company_id, &document.id)
            .send()
            .await?;
        println!("{:?} {:?}", download.file_name(), download.content_type());
        let bytes = download.bytes().await?;
        println!("{} bytes", bytes.len());
    }

    // A raw call.
    let response = oscar
        .request(HttpMethod::Get, "/api/me/")
        .query("expand", "companies")
        .send()
        .await?;
    let value: serde_json::Value = response.json()?;
    println!("{}", value["credential"]);

    // A token provider, and the login of the CLI.
    let _with_provider = Client::builder()
        .token_provider(|| async { Ok::<_, BoxError>(std::env::var("OSCAR_ACCESS_TOKEN")?) })
        .build()?;
    let _with_cli = Client::builder().credential(Credential::Cli).build()?;
    Ok(())
}
