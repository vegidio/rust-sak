//! Carrying a span across `.await` points.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use super::Span;
use super::span::PendingSpan;

pin_project_lite::pin_project! {
    /// A future that runs inside a span.
    ///
    /// The span is pushed onto whichever thread is polling, and taken back off when the poll returns. That is what
    /// makes it correct where a bare [`Span`] guard is not: a task may be polled on a different worker thread after
    /// every `.await`, and the span follows it instead of staying behind on the thread that started it.
    ///
    /// Created by [`Instrument::instrument`] or by `#[instrument]` on an `async fn`.
    #[derive(Debug)]
    pub struct Instrumented<F> {
        #[pin]
        inner: F,
        span: PendingSpan,
    }
}

impl<F: Future> Future for Instrumented<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        let this = self.project();

        this.span.enter();
        let polled = this.inner.poll(cx);
        this.span.exit();

        if polled.is_ready() {
            this.span.finish();
        }

        polled
    }
}

/// Runs a future inside a span.
///
/// Implemented for every [`Future`], so it applies to any `async` block or `async fn` call:
///
/// ```
/// use rust_sak::o11y::trace::{self, Instrument};
///
/// # async fn example() {
/// async { /* ... */ }
///     .instrument(trace::span!("charge_card", order_id = "ord_8812"))
///     .await;
/// # }
/// ```
pub trait Instrument: Future + Sized {
    /// Records this future's work under `span`, entering it around each poll and closing it when the future resolves
    /// — or when the future is dropped without resolving.
    fn instrument(self, span: Span) -> Instrumented<Self> {
        Instrumented {
            inner: self,
            span: span.detach(),
        }
    }
}

impl<F: Future> Instrument for F {}
