//! Procedural macros for the `o11y` feature of [`rust-sak`](https://github.com/vegidio/rust-sak).
//!
//! This crate exists only because a proc-macro crate cannot be a module of a normal one. Nothing here is meant to be
//! depended on directly — use `rust_sak::o11y::trace::instrument`, which re-exports it.

use proc_macro::TokenStream;

mod instrument;

/// Wraps a function body in a span named after the function, capturing its arguments as span fields.
///
/// The span closes when the function returns, including on an early return through `?` and on a panic unwinding
/// through the frame. Applied to an `async fn`, the body is wrapped in a future that enters the span around each
/// poll, so the context survives `.await` points and thread migration.
///
/// # Attributes
///
/// - `#[instrument(name = "...")]` overrides the span name, which defaults to the function's own name.
/// - `#[instrument(skip(a, b))]` leaves those arguments out of the captured fields.
/// - `#[instrument(skip_all)]` captures no arguments at all.
///
/// An argument that converts into an `o11y::Value` on its own — every integer and float, `bool`, `&str`, `String` —
/// is captured in that variant directly, so a `u64` arrives as an integer rather than as a decimal string and costs
/// no formatting. Anything else is captured with its `Debug` representation, so an argument that is neither must
/// implement [`Debug`]. Skip the ones that do not, or that are too large or too sensitive to record.
///
/// # Panics
///
/// Does not panic. Malformed input is reported as a compile error at the offending span.
///
/// ```ignore
/// use rust_sak::o11y::trace;
///
/// #[trace::instrument(skip(password))]
/// fn sign_in(user: &str, password: &str) -> bool {
///     true
/// }
/// ```
#[proc_macro_attribute]
pub fn instrument(args: TokenStream, item: TokenStream) -> TokenStream {
    instrument::expand(args.into(), item.into()).into()
}
