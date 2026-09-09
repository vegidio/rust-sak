use opentelemetry::Key;
use opentelemetry::logs::{AnyValue, Severity};

use super::{Telemetry, Value};

/// A log record under construction, returned by [`Telemetry::event`].
///
/// Fields are attached with [`field`](Event::field) or [`fields`](Event::fields), then the record is emitted by one of
/// the severity methods — [`info`](Event::info), [`warn`](Event::warn), [`debug`](Event::debug) or
/// [`error`](Event::error). Nothing is sent until one of those is called.
///
/// A field with the same name as an enrichment attribute (`version`, `session.id`, …) **replaces** it for that one
/// record; the enrichment itself is untouched.
///
/// ```no_run
/// # fn run() -> Result<(), Box<dyn std::error::Error>> {
/// use rust_sak::o11y::Telemetry;
///
/// let telemetry = Telemetry::builder("https://collector.example.com", "my-app").build()?;
///
/// telemetry
///     .event("export.finished")
///     .field("format", "avif")
///     .field("bytes", 1_048_576u64)
///     .field("resized", true)
///     .info();
/// # Ok(())
/// # }
/// ```
#[must_use = "an Event is only emitted when a severity method (info/warn/debug/error) is called"]
#[derive(Debug)]
pub struct Event<'a> {
    /// The handle that will emit this record.
    telemetry: &'a Telemetry,
    /// The record body, i.e. the event name.
    name: String,
    /// This record's own fields, in insertion order.
    fields: Vec<(Key, AnyValue)>,
}

impl<'a> Event<'a> {
    /// Starts a record named `name` against `telemetry`.
    pub(super) fn new(telemetry: &'a Telemetry, name: impl Into<String>) -> Self {
        Self {
            telemetry,
            name: name.into(),
            fields: Vec::new(),
        }
    }

    /// Attaches a single field. Call repeatedly to add more.
    ///
    /// Accepts anything convertible into a [`Value`] — strings (borrowed or owned), any integer or float width,
    /// `bool`, byte slices, and `Vec`/`HashMap` of the same.
    pub fn field(mut self, key: impl Into<Key>, value: impl Into<Value>) -> Self {
        self.fields.push((key.into(), value.into().into_any_value()));
        self
    }

    /// Attaches many fields at once, for a set assembled at runtime.
    ///
    /// ```no_run
    /// # fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::collections::HashMap;
    /// use rust_sak::o11y::{Telemetry, Value};
    ///
    /// let telemetry = Telemetry::builder("https://collector.example.com", "my-app").build()?;
    ///
    /// let mut collected: HashMap<String, Value> = HashMap::new();
    /// collected.insert("codec".to_string(), "avif".into());
    /// collected.insert("threads".to_string(), 8u32.into());
    ///
    /// telemetry.event("encode.started").fields(collected).info();
    /// # Ok(())
    /// # }
    /// ```
    pub fn fields<I, K, V>(mut self, fields: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<Key>,
        V: Into<Value>,
    {
        self.fields.extend(
            fields
                .into_iter()
                .map(|(key, value)| (key.into(), value.into().into_any_value())),
        );
        self
    }

    /// Emits the record at `Debug` severity.
    pub fn debug(self) {
        self.emit(Severity::Debug);
    }

    /// Emits the record at `Info` severity.
    pub fn info(self) {
        self.emit(Severity::Info);
    }

    /// Emits the record at `Warn` severity.
    pub fn warn(self) {
        self.emit(Severity::Warn);
    }

    /// Emits the record at `Error` severity, attaching `err`.
    ///
    /// The error's `Display` form is attached as `error`, and — if it has a [`source`](std::error::Error::source) —
    /// the chain below it is joined into `error.source`, so the underlying cause is not lost.
    pub fn error<E: std::error::Error + ?Sized>(mut self, err: &E) {
        self.fields
            .push((Key::from_static_str("error"), AnyValue::from(err.to_string())));

        let mut causes = Vec::new();
        let mut source = err.source();
        while let Some(cause) = source {
            causes.push(cause.to_string());
            source = cause.source();
        }

        if !causes.is_empty() {
            self.fields
                .push((Key::from_static_str("error.source"), AnyValue::from(causes.join(": "))));
        }

        self.emit(Severity::Error);
    }

    /// Renders the enrichment plus this record's own fields and hands the result to the logger.
    ///
    /// An enrichment attribute is dropped when this record sets a field of the same name, so the record's own value
    /// wins without either list being mutated.
    fn emit(self, severity: Severity) {
        self.telemetry.emit(self.name, severity, self.fields);
    }
}
