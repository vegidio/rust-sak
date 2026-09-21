//! The field syntax shared by the log macros and [`span!`](crate::o11y::trace::span).

/// Parses `key = value` pairs into a `Vec<(Cow<'static, str>, Value)>`.
///
/// Four forms are accepted, and they can be mixed freely:
///
/// | Written as        | Captured as                                     |
/// |-------------------|-------------------------------------------------|
/// | `key = value`     | `Value::from(value)` — anything `Into<Value>`   |
/// | `key = %value`    | the `Display` form, as a string                 |
/// | `key = ?value`    | the `Debug` form, as a string                   |
/// | `key`             | shorthand for `key = key`                       |
///
/// A key is normally an identifier. A string literal works too, for the dotted keys the OpenTelemetry semantic
/// conventions use: `"http.response.status_code" = 200`.
///
/// The rule order below is load-bearing. The `%` and `?` rules must precede the plain `= $value:expr` rule, because
/// `%error` is not a valid expression — reaching the plain rule first would make the `expr` fragment parser fail
/// outright instead of falling through to the next rule.
#[doc(hidden)]
#[macro_export]
macro_rules! __rust_sak_o11y_fields {
    // Done: build the vector from everything accumulated.
    //
    // Spelled out rather than `vec![..]` so that the no-field case still has a type. An empty `vec![]` would leave
    // the element type to be inferred from a caller that, by definition, supplied nothing to infer it from.
    (@acc [$($done:expr),*]) => {
        <::std::vec::Vec<(::std::borrow::Cow<'static, str>, $crate::o11y::Value)>>::from([$($done),*])
    };

    // key = %value — the Display form.
    (@acc [$($done:expr),*] $key:ident = % $value:expr $(, $($rest:tt)*)?) => {
        $crate::__rust_sak_o11y_fields!(@acc
            [$($done,)* (
                ::std::borrow::Cow::Borrowed(::core::stringify!($key)),
                $crate::o11y::Value::String(::std::format!("{}", $value)),
            )]
            $($($rest)*)?)
    };
    (@acc [$($done:expr),*] $key:literal = % $value:expr $(, $($rest:tt)*)?) => {
        $crate::__rust_sak_o11y_fields!(@acc
            [$($done,)* (
                ::std::borrow::Cow::Borrowed($key),
                $crate::o11y::Value::String(::std::format!("{}", $value)),
            )]
            $($($rest)*)?)
    };

    // key = ?value — the Debug form.
    (@acc [$($done:expr),*] $key:ident = ? $value:expr $(, $($rest:tt)*)?) => {
        $crate::__rust_sak_o11y_fields!(@acc
            [$($done,)* (
                ::std::borrow::Cow::Borrowed(::core::stringify!($key)),
                $crate::o11y::Value::String(::std::format!("{:?}", $value)),
            )]
            $($($rest)*)?)
    };
    (@acc [$($done:expr),*] $key:literal = ? $value:expr $(, $($rest:tt)*)?) => {
        $crate::__rust_sak_o11y_fields!(@acc
            [$($done,)* (
                ::std::borrow::Cow::Borrowed($key),
                $crate::o11y::Value::String(::std::format!("{:?}", $value)),
            )]
            $($($rest)*)?)
    };

    // key = value — anything convertible into a `Value`.
    (@acc [$($done:expr),*] $key:ident = $value:expr $(, $($rest:tt)*)?) => {
        $crate::__rust_sak_o11y_fields!(@acc
            [$($done,)* (
                ::std::borrow::Cow::Borrowed(::core::stringify!($key)),
                $crate::o11y::Value::from($value),
            )]
            $($($rest)*)?)
    };
    (@acc [$($done:expr),*] $key:literal = $value:expr $(, $($rest:tt)*)?) => {
        $crate::__rust_sak_o11y_fields!(@acc
            [$($done,)* (
                ::std::borrow::Cow::Borrowed($key),
                $crate::o11y::Value::from($value),
            )]
            $($($rest)*)?)
    };

    // key — shorthand for `key = key`.
    (@acc [$($done:expr),*] $key:ident $(, $($rest:tt)*)?) => {
        $crate::__rust_sak_o11y_fields!(@acc
            [$($done,)* (
                ::std::borrow::Cow::Borrowed(::core::stringify!($key)),
                $crate::o11y::Value::from($key),
            )]
            $($($rest)*)?)
    };

    // Entry point.
    ($($fields:tt)*) => {
        $crate::__rust_sak_o11y_fields!(@acc [] $($fields)*)
    };
}
