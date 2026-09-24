//! The four log macros.
//!
//! All four share one body, [`__rust_sak_o11y_log`], and differ only in the [`Level`](crate::o11y::Level) they pass
//! it; each public name is a one-line forwarder carrying its own documentation. Writing the gate and the [`emit`](super::emit)
//! call once is what keeps a change to the emit shape from being a four-place edit.
//!
//! # Why the names are mangled
//!
//! `#[macro_export]` places a `macro_rules!` macro at the **crate root**, whatever module it was written in, and
//! does not make it reachable at that module's path. A `macro_rules! info` here would therefore become
//! `rust_sak::info!` — not `rust_sak::o11y::log::info!`, and a collision besides.
//!
//! The way out, which is what `tracing` and `serde_json` do, is to export under a mangled `#[doc(hidden)]` name and
//! re-export it at the intended path with `pub use`. Two constraints follow: every path inside a macro body must be
//! `$crate`-rooted or absolute (`::std::format!`, not `format!`, which the caller may have shadowed), and the
//! functions the bodies call must be `pub`, because expansion happens in the caller's crate.

/// The shared body of the four log macros. Macro plumbing; not API.
///
/// The fields macro types its own empty case (see the `@acc []` rule), so the no-field call needs no separate arm
/// here — `__rust_sak_o11y_fields!()` is already a `Vec` with a concrete element type.
#[doc(hidden)]
#[macro_export]
macro_rules! __rust_sak_o11y_log {
    ($level:expr, $message:expr $(, $($fields:tt)*)?) => {
        if $crate::o11y::log::enabled($level) {
            $crate::o11y::log::emit(
                $level,
                $message,
                $crate::__rust_sak_o11y_fields!($($($fields)*)?),
            );
        }
    };
}

/// Records a message at [`Level::Debug`](crate::o11y::Level).
///
/// ```
/// use rust_sak::o11y::log;
///
/// # let attempt = 2;
/// log::debug!("retrying", attempt = attempt);
/// ```
#[doc(hidden)]
#[macro_export]
macro_rules! __rust_sak_o11y_debug {
    ($($args:tt)*) => { $crate::__rust_sak_o11y_log!($crate::o11y::Level::Debug, $($args)*) };
}

/// Records a message at [`Level::Info`](crate::o11y::Level).
///
/// ```
/// use rust_sak::o11y::log;
///
/// # let order_id = "ord_8812";
/// # let amount = 129.5;
/// log::info!("order received", order_id = order_id, amount = amount);
/// ```
#[doc(hidden)]
#[macro_export]
macro_rules! __rust_sak_o11y_info {
    ($($args:tt)*) => { $crate::__rust_sak_o11y_log!($crate::o11y::Level::Info, $($args)*) };
}

/// Records a message at [`Level::Warn`](crate::o11y::Level).
///
/// ```
/// use rust_sak::o11y::log;
///
/// # let order_id = "ord_8812";
/// log::warn!("large order flagged", order_id = order_id, threshold = 10_000.0);
/// ```
#[doc(hidden)]
#[macro_export]
macro_rules! __rust_sak_o11y_warn {
    ($($args:tt)*) => { $crate::__rust_sak_o11y_log!($crate::o11y::Level::Warn, $($args)*) };
}

/// Records a message at [`Level::Error`](crate::o11y::Level).
///
/// ```
/// use rust_sak::o11y::log;
///
/// # let order_id = "ord_8812";
/// # let error = std::fmt::Error;
/// log::error!("payment failed", order_id = order_id, error = %error);
/// ```
#[doc(hidden)]
#[macro_export]
macro_rules! __rust_sak_o11y_error {
    ($($args:tt)*) => { $crate::__rust_sak_o11y_log!($crate::o11y::Level::Error, $($args)*) };
}
