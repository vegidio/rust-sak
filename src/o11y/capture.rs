//! How `#[instrument]` turns a function argument into a [`Value`].
//!
//! The attribute cannot know an argument's type, so it has to pick a conversion that works for *any* of them. The
//! obvious choice — `format!("{:?}")` on everything — costs a heap allocation and a full `Debug` walk per argument
//! per call, and turns a `u64` into a decimal string when [`Value::Int`] would hold it exactly.
//!
//! The two traits below recover the cheap path without giving up the general one, through what is usually called
//! *autoref specialization*. Both declare the same method name, so `(&argument).o11y_value()` resolves to whichever
//! applies:
//!
//! - Rust first looks for a method whose receiver is `&T`, which is [`ValueViaInto`] — taken only when the argument
//!   converts into a `Value` on its own, so integers, floats, bools and strings land in their proper variant with no
//!   formatting at all.
//! - Only if that fails does it try one autoref further, at `&&T`, which is [`ValueViaDebug`]. Every `Debug` type
//!   reaches this, so nothing that used to compile stops compiling.
//!
//! The order is what makes it work, and it is the reason the narrow trait is implemented for `T` while the broad one
//! is implemented for `&T` rather than the other way round. Both are `#[doc(hidden)]` plumbing: the attribute brings
//! them into scope itself, and no caller names them.

use std::fmt::Debug;

use super::Value;

/// The cheap path: an argument that already converts into a [`Value`]. See the [module docs](self).
#[doc(hidden)]
pub trait ValueViaInto {
    /// Converts the argument into a [`Value`] through its `Into` impl.
    fn o11y_value(&self) -> Value;
}

impl<T: Into<Value> + Clone> ValueViaInto for T {
    fn o11y_value(&self) -> Value {
        self.clone().into()
    }
}

/// The fallback: any [`Debug`] argument, rendered as a string. See the [module docs](self).
#[doc(hidden)]
pub trait ValueViaDebug {
    /// Converts the argument into a [`Value`] through its `Debug` impl.
    fn o11y_value(&self) -> Value;
}

impl<T: Debug> ValueViaDebug for &T {
    fn o11y_value(&self) -> Value {
        Value::String(format!("{self:?}"))
    }
}
