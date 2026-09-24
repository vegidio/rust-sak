//! The `span!` macro.
//!
//! Defined here and re-exported from [`trace`](super) — see the note in [`log::macros`](super::super::log::macros)
//! about why `#[macro_export]` forces the mangled name plus a `pub use`.

/// Opens a span, returning a guard that closes it on drop.
///
/// Takes a name and the same field syntax as the log macros:
///
/// ```
/// use rust_sak::o11y::trace;
///
/// # let order_id = "ord_8812";
/// # let error = std::fmt::Error;
/// let _span = trace::span!("charge_card", order_id = order_id, attempt = 1, cause = %error);
/// ```
///
/// The guard must be bound: `trace::span!("x");` opens and immediately closes the span, which the `#[must_use]` on
/// [`Span`](crate::o11y::trace::Span) warns about.
#[doc(hidden)]
#[macro_export]
macro_rules! __rust_sak_o11y_span {
    // One arm for both the field and no-field forms: the fields macro types its own empty case, so
    // `__rust_sak_o11y_fields!()` is already a `Vec` with a concrete element type.
    ($name:expr $(, $($fields:tt)*)?) => {
        if $crate::o11y::trace::enabled() {
            $crate::o11y::trace::Span::__enter($name, $crate::__rust_sak_o11y_fields!($($($fields)*)?))
        } else {
            $crate::o11y::trace::Span::__disabled()
        }
    };
}
