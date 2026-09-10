//! Proxy configuration for [`Fetch`](super::Fetch).

use std::fmt;

/// An explicit proxy for [`Fetch::proxy`](super::Fetch::proxy) to route requests through.
///
/// Set one only to *override* what the environment says. With no proxy configured at all,
/// `reqwest`'s own detection still applies — the `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` variables on
/// every platform, and the system proxy settings on macOS and Windows. Use
/// [`Fetch::no_proxy`](super::Fetch::no_proxy) to opt out of that detection instead of overriding it.
///
/// ```
/// use rust_sak::fetch::ProxySettings;
///
/// let plain = ProxySettings::new("http://proxy.example.com:3128");
/// let authenticated = ProxySettings::new("http://proxy.example.com:3128")
///     .basic_auth("user", "secret")
///     .no_proxy("localhost,127.0.0.1,.internal");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxySettings {
    /// The proxy URL, applied to both HTTP and HTTPS requests.
    pub(super) url: String,
    /// Optional `Proxy-Authorization` credentials as `(username, password)`.
    pub(super) basic_auth: Option<(String, String)>,
    /// Optional bypass list in `NO_PROXY` syntax — a comma-separated set of hosts, domain suffixes
    /// and CIDR blocks that go direct.
    pub(super) no_proxy: Option<String>,
}

impl ProxySettings {
    /// A proxy at `url`, with no credentials and no bypass list.
    ///
    /// The URL is not parsed here: it is validated when the client is built, so an invalid one
    /// surfaces as the [`reqwest::Error`] from the request rather than from this call.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            basic_auth: None,
            no_proxy: None,
        }
    }

    /// Sets the `Proxy-Authorization` credentials sent to the proxy.
    #[must_use]
    pub fn basic_auth(mut self, username: impl Into<String>, password: impl Into<String>) -> Self {
        self.basic_auth = Some((username.into(), password.into()));
        self
    }

    /// Sets the bypass list, in `NO_PROXY` syntax: a comma-separated set of hosts, domain suffixes
    /// and CIDR blocks reached directly rather than through the proxy.
    #[must_use]
    pub fn no_proxy(mut self, no_proxy: impl Into<String>) -> Self {
        self.no_proxy = Some(no_proxy.into());
        self
    }

    /// Builds the `reqwest` proxy this describes.
    ///
    /// # Errors
    ///
    /// Returns a [`reqwest::Error`] if [`url`](ProxySettings::url) is not a valid proxy URL.
    pub(super) fn to_reqwest(&self) -> Result<reqwest::Proxy, reqwest::Error> {
        let mut proxy = reqwest::Proxy::all(&self.url)?;

        if let Some((username, password)) = &self.basic_auth {
            proxy = proxy.basic_auth(username, password);
        }

        // `from_string` returns `None` for a list that parses to no usable rule, which is the same
        // as having set none — so it is passed straight through rather than treated as an error.
        if let Some(list) = &self.no_proxy {
            proxy = proxy.no_proxy(reqwest::NoProxy::from_string(list));
        }

        Ok(proxy)
    }
}

impl fmt::Display for ProxySettings {
    /// Renders the proxy URL alone. Credentials are deliberately never shown, so a `ProxySettings`
    /// can be named in an error message or a log line without leaking a password.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.url)
    }
}

/// How a [`Fetch`](super::Fetch) resolves the proxy for its requests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) enum ProxyMode {
    /// Leave `reqwest`'s own detection in place: the proxy environment variables everywhere, plus
    /// the system proxy settings on macOS and Windows. The default.
    #[default]
    Detect,
    /// Ignore all proxy configuration, including the environment, and connect directly.
    Disabled,
    /// Route through this proxy, overriding anything the environment says.
    Explicit(ProxySettings),
}
