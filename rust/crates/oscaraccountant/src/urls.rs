//! The base URL and the URL of a call (`clients/spec/README.md`, sections 3.1 and 3.2).

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

use crate::Error;

/// The base URL when neither the client nor `OSCAR_BASE_URL` sets one: the production origin.
///
/// The API is under `/api/` and OAuth under `/oauth/`.
pub const DEFAULT_BASE_URL: &str = "https://oscaraccountant.com";

/// Every byte except the RFC 3986 unreserved characters is percent-encoded.
const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Percent-encode `value` for a path segment or a query component.
pub(crate) fn encode(value: &str) -> String {
    utf8_percent_encode(value, UNRESERVED).to_string()
}

/// A base URL in its normal form: no trailing `/`, the scheme and the host in lower case.
///
/// The scheme must be `https`, or `http` for a loopback host (`localhost`, `127.0.0.1`, `[::1]`)
/// or a host that ends in `.personal`. Any other value is an [`Error::Config`].
pub fn normalize_base_url(value: &str) -> Result<String, Error> {
    let invalid = |why: &str| Error::Config(format!("the base URL {value:?} {why}"));
    let url = url::Url::parse(value.trim()).map_err(|_| invalid("is not a URL"))?;
    if url.query().is_some() || url.fragment().is_some() {
        return Err(invalid("must not have a query or a fragment"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid("must not hold a user name or a password"));
    }
    let host = url.host_str().ok_or_else(|| invalid("has no host"))?;
    let local = matches!(host, "localhost" | "127.0.0.1" | "[::1]") || host.ends_with(".personal");
    match url.scheme() {
        "https" => {}
        "http" if local => {}
        "http" => return Err(invalid("must use https (http is only for a loopback host)")),
        _ => return Err(invalid("must use https")),
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

/// The path of `template` with each `{name}` replaced by its percent-encoded value. A value that
/// is empty, `.` or `..` is an [`Error::Config`]: a URL parser removes or merges such a segment
/// (also `%2E%2E`), so the call would reach another path.
pub(crate) fn render_path(template: &str, params: &[(&str, String)]) -> Result<String, Error> {
    let mut path = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let end = rest[start..]
            .find('}')
            .map(|end| start + end)
            .ok_or_else(|| Error::Config(format!("the path {template} has an unclosed {{")))?;
        let name = &rest[start + 1..end];
        let value = params
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value)
            .ok_or_else(|| Error::Config(format!("the path parameter {name} has no value")))?;
        if matches!(value.as_str(), "" | "." | "..") {
            return Err(Error::Config(format!(
                "the path parameter {name} must not be empty, `.` or `..`"
            )));
        }
        path.push_str(&rest[..start]);
        path.push_str(&encode(value));
        rest = &rest[end + 1..];
    }
    path.push_str(rest);
    Ok(path)
}

/// The full URL of a call: the base URL, the path, and the query parameters in their order.
///
/// `path` is already encoded and may hold a query of its own; `query` is appended to it.
pub(crate) fn build_url(base_url: &str, path: &str, query: &[(String, String)]) -> String {
    let mut url = String::with_capacity(base_url.len() + path.len() + 16 * query.len());
    url.push_str(base_url);
    if !path.starts_with('/') {
        url.push('/');
    }
    url.push_str(path);
    let mut separator = if path.contains('?') { '&' } else { '?' };
    for (name, value) in query {
        url.push(separator);
        url.push_str(&encode(name));
        url.push('=');
        url.push_str(&encode(value));
        separator = '&';
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_a_base_url() {
        let ok = [
            ("https://API.Example.com/", "https://api.example.com"),
            ("HTTPS://api.example.com", "https://api.example.com"),
            (
                "https://api.example.com:8443/base/",
                "https://api.example.com:8443/base",
            ),
            ("http://localhost:3500", "http://localhost:3500"),
            ("http://127.0.0.1:3500/", "http://127.0.0.1:3500"),
            ("http://[::1]:3500", "http://[::1]:3500"),
            (
                "http://omarchy.personal:3500",
                "http://omarchy.personal:3500",
            ),
            (" https://api.example.com ", "https://api.example.com"),
        ];
        for (value, expected) in ok {
            assert_eq!(normalize_base_url(value).unwrap(), expected, "{value}");
        }
    }

    #[test]
    fn refuses_a_wrong_base_url() {
        let wrong = [
            ("not a url", "is not a URL"),
            ("http://api.example.com", "must use https"),
            ("ftp://api.example.com", "must use https"),
            ("https://api.example.com/?a=1", "query"),
            ("https://api.example.com/#top", "query"),
            ("https://user:secret@api.example.com", "user name"),
            ("file:///etc/passwd", "has no host"),
        ];
        for (value, why) in wrong {
            let error = normalize_base_url(value).unwrap_err();
            assert!(matches!(error, Error::Config(_)), "{value}");
            assert!(error.to_string().contains(why), "{value}: {error}");
            assert!(!error.to_string().contains("secret") || value.contains("secret"));
        }
    }

    #[test]
    fn encodes_every_reserved_byte() {
        assert_eq!(encode("aZ09-._~"), "aZ09-._~");
        assert_eq!(encode("a/b c?d#e%f"), "a%2Fb%20c%3Fd%23e%25f");
        assert_eq!(encode("Ö"), "%C3%96");
    }

    #[test]
    fn renders_a_path() {
        let params = [
            ("company_id", "c 1".to_owned()),
            ("file_id", "f/2".to_owned()),
        ];
        assert_eq!(
            render_path("/api/companies/{company_id}/files/{file_id}/", &params).unwrap(),
            "/api/companies/c%201/files/f%2F2/"
        );
        assert_eq!(render_path("/api/me/", &[]).unwrap(), "/api/me/");
        for bad in ["", ".", ".."] {
            let params = [("company_id", bad.to_owned())];
            let error = render_path("/api/companies/{company_id}/", &params).unwrap_err();
            assert!(matches!(error, Error::Config(_)), "{bad:?}");
            assert!(error.to_string().contains("`..`"), "{error}");
        }
        let params = [("company_id", "...".to_owned())];
        assert_eq!(
            render_path("/api/companies/{company_id}/", &params).unwrap(),
            "/api/companies/.../"
        );
        assert!(
            render_path("/api/{missing}/", &[])
                .unwrap_err()
                .to_string()
                .contains("missing")
        );
        assert!(
            render_path("/api/{open", &[])
                .unwrap_err()
                .to_string()
                .contains("unclosed")
        );
    }

    #[test]
    fn builds_a_url() {
        let query = vec![
            ("state".to_owned(), "issued".to_owned()),
            ("q".to_owned(), "a&b".to_owned()),
        ];
        assert_eq!(
            build_url("https://x.test", "/api/invoices/", &query),
            "https://x.test/api/invoices/?state=issued&q=a%26b"
        );
        assert_eq!(
            build_url("https://x.test", "api/me/", &[]),
            "https://x.test/api/me/"
        );
        assert_eq!(
            build_url(
                "https://x.test",
                "/api/me/?a=1",
                &[("b".into(), "2".into())]
            ),
            "https://x.test/api/me/?a=1&b=2"
        );
    }
}
