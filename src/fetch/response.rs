//! A response returned with its metadata intact, rather than reduced to its body.

use reqwest::header::HeaderMap;

/// A response and the metadata that came with it: the body, the status it arrived with, and its headers.
///
/// [`Fetch::text`](super::Fetch::text) and [`Fetch::json`](super::Fetch::json) hand back the body alone, which is what
/// almost every caller wants. This is for the ones that cannot work from the body: an API whose pagination cursor is
/// in a `Link` header, an artifact whose checksum is in an `ETag`, a rate limit that has to be read before the next
/// request is sent.
#[derive(Debug, Clone)]
pub struct Response<T> {
    /// The body, as text or deserialized into `T`.
    pub body: T,
    /// The status the response arrived with. Always a success status: a `4xx` or `5xx` is an error rather than a
    /// response, exactly as it is for `text` and `json`.
    pub status: reqwest::StatusCode,
    /// Every response header, as received.
    pub headers: HeaderMap,
}

impl<T> Response<T> {
    /// The first value of the `name` header, or `None` where there is none or it is not valid text.
    ///
    /// The name is matched case-insensitively, as HTTP header names are.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name)?.to_str().ok()
    }

    /// The target of the `Link` header whose relation type is `rel`, or `None` where the response declares none.
    ///
    /// This is how a paginated API is followed to exhaustion: request a page, ask for `link("next")`, and stop when
    /// there is no answer. The alternative — a `limit` parameter chosen to be larger than the collection — is a bound
    /// that is eventually wrong, and silently returns a prefix when it is.
    ///
    /// ```
    /// # use rust_sak::fetch::Response;
    /// # use reqwest::header::HeaderMap;
    /// let mut headers = HeaderMap::new();
    /// headers.insert("link", r#"<https://example.com/p2>; rel="next""#.parse().unwrap());
    /// let response = Response { body: (), status: reqwest::StatusCode::OK, headers };
    ///
    /// assert_eq!(response.link("next"), Some("https://example.com/p2"));
    /// assert_eq!(response.link("prev"), None);
    /// ```
    ///
    /// Parsed per RFC 8288: every `Link` header on the response is considered, each may carry several
    /// comma-separated links, and a link's `rel` may be quoted or bare and may list several relation types separated
    /// by spaces. Relation types are matched case-insensitively, as the RFC specifies.
    pub fn link(&self, rel: &str) -> Option<&str> {
        self.headers
            .get_all("link")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(links)
            .find_map(|(target, parameters)| has_rel(parameters, rel).then_some(target))
    }
}

/// Splits one `Link` header value into its links, each as the target inside the angle brackets and the parameters
/// following it.
///
/// Not `split(',')`: a comma is legal inside a URI-Reference, and a cursor is exactly where one turns up. The angle
/// brackets are what actually delimit a target — RFC 8288 requires them and they cannot appear inside — so the scan
/// keys on those and takes everything up to the next one as the preceding link's parameters.
fn links(value: &str) -> impl Iterator<Item = (&str, &str)> {
    let mut rest = value;

    std::iter::from_fn(move || {
        loop {
            let open = rest.find('<')?;
            let after = &rest[open + 1..];

            // A `<` with no `>` after it is not a link at all, and nothing later in the value can be one either.
            let Some(close) = after.find('>') else {
                rest = "";
                return None;
            };

            let (target, tail) = (&after[..close], &after[close + 1..]);
            // Up to the next target, so a value carrying several links gives each its own parameters.
            let end = tail.find('<').unwrap_or(tail.len());
            let parameters = &tail[..end];
            rest = &tail[end..];

            let target = target.trim();
            if !target.is_empty() {
                return Some((target, parameters));
            }
        }
    })
}

/// Reports whether a link's parameters declare the relation type `rel`.
///
/// `rel` may be quoted or bare and may list several types separated by spaces, so `rel="next last"` answers to both.
fn has_rel(parameters: &str, rel: &str) -> bool {
    parameters
        .split(';')
        .filter_map(|parameter| parameter.split_once('='))
        .filter(|(name, _)| name.trim().eq_ignore_ascii_case("rel"))
        .any(|(_, value)| {
            // The comma comes off before the quotes: a link followed by another carries the separator into its own
            // last parameter, so `rel="prev", ` would otherwise compare as `prev",` and match nothing.
            value
                .trim()
                .trim_end_matches(',')
                .trim()
                .trim_matches('"')
                .split_ascii_whitespace()
                .any(|declared| declared.eq_ignore_ascii_case(rel))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A response carrying `values` as its `Link` headers and nothing else.
    fn linked(values: &[&str]) -> Response<()> {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append("link", value.parse().unwrap());
        }

        Response {
            body: (),
            status: reqwest::StatusCode::OK,
            headers,
        }
    }

    #[test]
    fn a_header_is_read_back_case_insensitively() {
        let mut headers = HeaderMap::new();
        headers.insert("etag", "\"a1b2\"".parse().unwrap());
        let response = Response {
            body: (),
            status: reqwest::StatusCode::OK,
            headers,
        };

        assert_eq!(response.header("ETag"), Some("\"a1b2\""));
        assert_eq!(response.header("etag"), Some("\"a1b2\""));
        assert_eq!(response.header("x-absent"), None);
    }

    #[test]
    fn the_next_page_is_taken_from_a_quoted_relation() {
        let response = linked(&[r#"<https://example.com/page/2>; rel="next""#]);

        assert_eq!(response.link("next"), Some("https://example.com/page/2"));
    }

    #[test]
    fn a_bare_relation_is_read_the_same_as_a_quoted_one() {
        // RFC 8288 allows either; an API that quotes today may not tomorrow.
        let response = linked(&["<https://example.com/page/2>; rel=next"]);

        assert_eq!(response.link("next"), Some("https://example.com/page/2"));
    }

    #[test]
    fn one_value_carrying_several_links_gives_each_its_own_relation() {
        let response = linked(&[concat!(
            r#"<https://example.com/page/1>; rel="prev", "#,
            r#"<https://example.com/page/3>; rel="next", "#,
            r#"<https://example.com/page/9>; rel="last""#
        )]);

        assert_eq!(response.link("prev"), Some("https://example.com/page/1"));
        assert_eq!(response.link("next"), Some("https://example.com/page/3"));
        assert_eq!(response.link("last"), Some("https://example.com/page/9"));
    }

    #[test]
    fn several_link_headers_are_all_considered() {
        let response = linked(&[
            r#"<https://example.com/page/1>; rel="prev""#,
            r#"<https://example.com/page/3>; rel="next""#,
        ]);

        assert_eq!(response.link("next"), Some("https://example.com/page/3"));
    }

    #[test]
    fn a_comma_inside_a_target_does_not_split_the_link() {
        // The failure a `split(',')` parse has, and a cursor is exactly where a comma turns up: the target would be
        // truncated at the comma and the relation parsed out of the remainder.
        let response = linked(&[r#"<https://example.com/t?cursor=a,b,c>; rel="next""#]);

        assert_eq!(response.link("next"), Some("https://example.com/t?cursor=a,b,c"));
    }

    #[test]
    fn a_relation_listing_several_types_answers_to_each_of_them() {
        let response = linked(&[r#"<https://example.com/page/9>; rel="next last""#]);

        assert_eq!(response.link("next"), Some("https://example.com/page/9"));
        assert_eq!(response.link("last"), Some("https://example.com/page/9"));
    }

    #[test]
    fn a_relation_type_is_matched_ignoring_case() {
        let response = linked(&[r#"<https://example.com/page/2>; REL="NEXT""#]);

        assert_eq!(response.link("next"), Some("https://example.com/page/2"));
    }

    #[test]
    fn other_parameters_beside_the_relation_are_ignored() {
        let response = linked(&[r#"<https://example.com/page/2>; type="application/json"; rel="next"; title="Next""#]);

        assert_eq!(response.link("next"), Some("https://example.com/page/2"));
    }

    #[test]
    fn a_response_with_no_link_header_at_all_declares_nothing() {
        let response = Response {
            body: (),
            status: reqwest::StatusCode::OK,
            headers: HeaderMap::new(),
        };

        assert_eq!(response.link("next"), None);
    }

    #[test]
    fn a_malformed_value_yields_no_link_rather_than_a_wrong_one() {
        // A truncated or bracketless value is the case where guessing would hand back a target that was never
        // published, and a pagination loop would follow it.
        for value in [
            "",
            "rel=\"next\"",
            "<https://example.com/page/2; rel=\"next\"",
            "<>; rel=\"next\"",
        ] {
            assert_eq!(linked(&[value]).link("next"), None, "{value:?}");
        }
    }

    #[test]
    fn a_relation_that_is_a_prefix_of_another_is_not_mistaken_for_it() {
        let response = linked(&[r#"<https://example.com/page/2>; rel="nextish""#]);

        assert_eq!(response.link("next"), None);
    }
}
