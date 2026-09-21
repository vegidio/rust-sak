use std::fmt;

/// The severity of a log record, and the threshold the log macros gate on.
///
/// The discriminants are the OTLP severity numbers, so a level is both the comparison key for
/// [`min_level`](super::ConfigBuilder::min_level) and the value that goes on the wire. Ordering follows severity:
/// `Debug < Info < Warn < Error`.
///
/// ```
/// use rust_sak::o11y::Level;
///
/// assert!(Level::Debug < Level::Error);
/// assert_eq!(Level::Info.severity_text(), "INFO");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Level {
    /// Detail useful while diagnosing a problem, too noisy to keep on by default.
    Debug = 5,
    /// The default: something worth recording happened.
    Info = 9,
    /// Something unexpected that the application recovered from.
    Warn = 13,
    /// Something failed.
    Error = 17,
}

impl Level {
    /// The OTLP `severityNumber` for this level.
    pub fn severity_number(self) -> u8 {
        self as u8
    }

    /// The OTLP `severityText` for this level.
    pub fn severity_text(self) -> &'static str {
        match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.severity_text())
    }
}
