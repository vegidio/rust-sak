use std::fmt;

/// The deployment environment reported with every record.
///
/// Becomes the `deployment.environment.name` resource attribute on the exported logs. [`Environment::Custom`] covers
/// anything beyond the two common cases (e.g. `"staging"`).
///
/// ```
/// use rust_sak::o11y::Environment;
///
/// assert_eq!(Environment::Development.to_string(), "development");
/// assert_eq!(Environment::Custom("staging".to_string()).to_string(), "staging");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Environment {
    /// A local or test deployment. The default.
    #[default]
    Development,
    /// A live deployment serving real users.
    Production,
    /// Any other environment name.
    Custom(String),
}

impl fmt::Display for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Environment::Development => f.write_str("development"),
            Environment::Production => f.write_str("production"),
            Environment::Custom(name) => f.write_str(name),
        }
    }
}
