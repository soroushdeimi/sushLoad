//! Helpers that keep secrets out of log lines: URLs lose userinfo, query and fragment; proxy URLs
//! lose their credentials. Cookies and tokens are never passed to the log at all.

/// `https://user:pw@host/path/file.zip?token=x#y` becomes `https://host/path/file.zip?…`.
/// Without a query or fragment the URL is returned (minus userinfo) unchanged.
pub fn redact_url(url: &str) -> String {
    let url = url.trim();
    let Some((scheme, rest)) = url.split_once("://") else {
        return match url.find(['?', '#']) {
            Some(i) => format!("{}?…", &url[..i]),
            None => url.to_string(),
        };
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    match tail.find(['?', '#']) {
        Some(i) => format!("{scheme}://{host}{}?…", &tail[..i]),
        None => format!("{scheme}://{host}{tail}"),
    }
}

/// Redacts every `http(s)://…` URL inside free text (error messages quote the URL they failed on).
pub fn redact_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let next = ["https://", "http://"]
            .iter()
            .filter_map(|p| rest.find(p))
            .min();
        let Some(start) = next else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let len = tail
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '"' | '\'' | '<' | '>'))
            .unwrap_or(tail.len());
        out.push_str(&redact_url(&tail[..len]));
        rest = &tail[len..];
    }
}

/// `socks5h://user:pass@host:1080/` becomes `socks5h://host:1080`.
pub fn redact_proxy(url: &str) -> String {
    let url = url.trim();
    let Some((scheme, rest)) = url.split_once("://") else {
        return "(invalid)".to_string();
    };
    let host_port = rest.rsplit_once('@').map_or(rest, |(_, hp)| hp);
    format!(
        "{}://{}",
        scheme.to_ascii_lowercase(),
        host_port.trim_end_matches('/')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_loses_userinfo_query_and_fragment() {
        let got = redact_url("https://user:pw@host.example:8443/a/b.zip?token=SECRET&x=1#frag");
        assert_eq!(got, "https://host.example:8443/a/b.zip?…");
        assert!(!got.contains("SECRET") && !got.contains("pw") && !got.contains("frag"));
        assert_eq!(redact_url("http://h/x.bin"), "http://h/x.bin");
        assert_eq!(redact_url("http://h?sig=abc"), "http://h?…");
        assert_eq!(redact_url("https://h/p#only-fragment"), "https://h/p?…");
        assert_eq!(redact_url("not a url?secret=1"), "not a url?…");
    }

    #[test]
    fn text_with_urls_is_redacted_in_place() {
        let got = redact_text(
            "error sending request for url (https://cdn.test/f.zip?sig=TOPSECRET): timed out; also http://a.test/b?k=v",
        );
        assert_eq!(
            got,
            "error sending request for url (https://cdn.test/f.zip?…): timed out; also http://a.test/b?…"
        );
        assert_eq!(redact_text("no urls here"), "no urls here");
    }

    #[test]
    fn proxy_credentials_are_stripped() {
        assert_eq!(
            redact_proxy("socks5h://bob:hunter2@10.0.0.1:1080/"),
            "socks5h://10.0.0.1:1080"
        );
        assert_eq!(redact_proxy("HTTP://h:8080"), "http://h:8080");
        assert!(!redact_proxy("http://u:p%40ss@h:1").contains("ss"));
        assert_eq!(redact_proxy("garbage"), "(invalid)");
    }
}
