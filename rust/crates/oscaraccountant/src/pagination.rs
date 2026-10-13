//! The auto-paging stream of a list call (`clients/spec/README.md`, section 3.6).
//!
//! The stream sends the first request with the caller's parameters, then the same parameters
//! with `cursor` set to the last `next_cursor`, until `next_cursor` is null. It fetches a page
//! only when the caller needs its first item, and each page is a `GET` with its own retries.

use std::fmt;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::stream::{self, BoxStream, Stream, StreamExt, TryStreamExt};

use crate::client::Prepared;
use crate::{Client, Error, Page};

/// Every item of every page of a list call. A [`Stream`] of `Result<Item, Error>`.
///
/// ```no_run
/// # async fn run(oscar: oscaraccountant::Client) -> Result<(), oscaraccountant::Error> {
/// use futures::TryStreamExt;
///
/// let mut invoices = oscar.invoices().list("company-id").state("issued").stream();
/// while let Some(invoice) = invoices.try_next().await? {
///     println!("{}", invoice.id);
/// }
/// # Ok(()) }
/// ```
#[must_use = "a stream does nothing until it is polled"]
pub struct PageStream<P: Page> {
    /// The first request, or the error of its preparation, until the first poll.
    start: Option<Box<Result<(Client, Prepared), Error>>>,
    items: Option<BoxStream<'static, Result<P::Item, Error>>>,
}

impl<P: Page> fmt::Debug for PageStream<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PageStream").finish_non_exhaustive()
    }
}

impl<P> PageStream<P>
where
    P: Page + Send + 'static,
    P::Item: Send + 'static,
{
    pub(crate) fn new(client: Client, first: Result<Prepared, Error>) -> Self {
        Self {
            start: Some(Box::new(first.map(|prepared| (client, prepared)))),
            items: None,
        }
    }

    /// The pages instead of the items: each item of this stream is one whole page.
    pub fn pages(self) -> Pages<P> {
        let inner = match self.start {
            Some(start) => page_stream::<P>(*start),
            // Already polled: the items stream owns the walk.
            None => stream::empty().boxed(),
        };
        Pages {
            inner,
            _page: PhantomData,
        }
    }
}

/// The pages of a list call, from [`PageStream::pages`]. A [`Stream`] of `Result<P, Error>`.
#[must_use = "a stream does nothing until it is polled"]
pub struct Pages<P> {
    inner: BoxStream<'static, Result<P, Error>>,
    _page: PhantomData<fn() -> P>,
}

impl<P> fmt::Debug for Pages<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pages").finish_non_exhaustive()
    }
}

impl<P> Stream for Pages<P> {
    type Item = Result<P, Error>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.poll_next_unpin(context)
    }
}

fn page_stream<P>(start: Result<(Client, Prepared), Error>) -> BoxStream<'static, Result<P, Error>>
where
    P: Page + Send + 'static,
{
    match start {
        Ok((client, prepared)) => pages::<P>(client, prepared),
        Err(error) => stream::once(async move { Err(error) }).fuse().boxed(),
    }
}

/// The stream of pages: fetch one, read its `next_cursor`, repeat.
fn pages<P>(client: Client, first: Prepared) -> BoxStream<'static, Result<P, Error>>
where
    P: Page + Send + 'static,
{
    stream::unfold(Some((client, first)), |state| async move {
        let (client, prepared) = state?;
        let next = prepared.clone();
        let result = async {
            let body = client.send(prepared, false).await?.into_bytes();
            let value: serde_json::Value = serde_json::from_slice(&body)?;
            let cursor = value
                .get("next_cursor")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            let page: P = serde_json::from_value(value)?;
            Ok::<_, Error>((page, cursor))
        }
        .await;
        match result {
            // An empty cursor ends the walk too: sending it would ask for the first page again.
            Ok((page, Some(cursor))) if !cursor.is_empty() => {
                Some((Ok(page), Some((client, with_cursor(next, cursor)))))
            }
            Ok((page, _)) => Some((Ok(page), None)),
            Err(error) => Some((Err(error), None)),
        }
    })
    // Polling after the end gives `None` again instead of a panic.
    .fuse()
    .boxed()
}

/// `prepared` with the query parameter `cursor` set to `cursor`.
fn with_cursor(mut prepared: Prepared, cursor: String) -> Prepared {
    prepared.query.retain(|(name, _)| name != "cursor");
    prepared.query.push(("cursor".to_owned(), cursor));
    prepared
}

impl<P> Stream for PageStream<P>
where
    P: Page + Send + 'static,
    P::Item: Send + 'static,
{
    type Item = Result<P::Item, Error>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(start) = self.start.take() {
            let items = page_stream::<P>(*start)
                .map_ok(|page| stream::iter(page.into_parts().0.into_iter().map(Ok)))
                .try_flatten()
                .fuse()
                .boxed();
            self.items = Some(items);
        }
        match self.items.as_mut() {
            Some(items) => items.poll_next_unpin(context),
            None => Poll::Ready(None),
        }
    }
}
