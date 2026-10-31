//! Small parsing helpers used by the engine: file names, Content-Disposition and Content-Range.

#![allow(dead_code)]

use percent_encoding::percent_decode_str;
use reqwest::Url;

/// Replace path separators, colons and control characters; never return an empty file name.
pub fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == ':' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        "download".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Last non-empty path segment of `url`, percent-decoded and sanitized. Falls back to `download`.
pub fn file_name_from_url(url: &Url) -> String {
    let raw = url
        .path_segments()
        .and_then(|mut segments| segments.rfind(|s| !s.is_empty()))
        .map(|s| percent_decode_str(s).decode_utf8_lossy().into_owned())
        .unwrap_or_default();
    sanitize_file_name(&raw)
}

/// Filename from a `Content-Disposition` header, preferring RFC 5987 `filename*`.
pub fn parse_content_disposition(value: &str) -> Option<String> {
    let mut plain = None;
    let mut extended = None;

    for part in split_unquoted(value, ';') {
        let Some((key, val)) = part.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let val = unquote(val.trim());
        match key.as_str() {
            "filename*" => {
                if let Some(name) = decode_rfc5987(&val) {
                    extended = Some(name);
                }
            }
            "filename" => plain = Some(val),
            _ => {}
        }
    }

    extended
        .or(plain)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// A parsed satisfied byte range from a `Content-Range` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentRange {
    pub start: u64,
    pub end: u64,
    /// `None` when the server sent `/*` instead of a complete length.
    pub total: Option<u64>,
}

/// Parse `bytes <start>-<end>/<total|*>`; returns `None` for malformed or `bytes */N` headers.
pub fn parse_content_range(value: &str) -> Option<ContentRange> {
    let rest = value.trim().strip_prefix("bytes")?.trim_start();
    if rest.starts_with("*/") {
        return None;
    }
    let (range, total) = rest.split_once('/')?;
    let (start, end) = range.trim().split_once('-')?;
    let start: u64 = start.trim().parse().ok()?;
    let end: u64 = end.trim().parse().ok()?;
    if end < start {
        return None;
    }
    let total = match total.trim() {
        "*" => None,
        other => Some(other.parse().ok()?),
    };
    Some(ContentRange { start, end, total })
}

fn split_unquoted(s: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for (idx, c) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            _ if c == sep && !quoted => {
                parts.push(&s[start..idx]);
                start = idx + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"' {
        let inner = &value[1..value.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut escaped = false;
        for c in inner.chars() {
            if escaped {
                out.push(c);
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else {
                out.push(c);
            }
        }
        out
    } else {
        value.to_string()
    }
}

fn decode_rfc5987(raw: &str) -> Option<String> {
    let mut parts = raw.splitn(3, '\'');
    let charset = parts.next()?;
    let has_charset = parts.next().is_some();
    let has_lang = parts.next().is_some();

    let encoded = if has_charset && has_lang {
        raw.splitn(3, '\'').nth(2).unwrap_or_default()
    } else {
        raw
    };
    if encoded.is_empty() {
        return None;
    }

    let bytes: Vec<u8> = percent_decode_str(encoded).collect();
    let decoded = match charset.to_ascii_lowercase().as_str() {
        "iso-8859-1" | "latin1" if has_charset && has_lang => {
            bytes.iter().map(|&b| b as char).collect()
        }
        _ => String::from_utf8_lossy(&bytes).into_owned(),
    };
    Some(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_separators_controls_and_empty_names() {
        assert_eq!(sanitize_file_name("a/b:c.txt"), "a_b_c.txt");
        assert_eq!(sanitize_file_name("line\nfeed"), "line_feed");
        assert_eq!(sanitize_file_name(""), "download");
        assert_eq!(sanitize_file_name("   "), "download");
        assert_eq!(sanitize_file_name(".."), "download");
        assert_eq!(sanitize_file_name("  report.pdf  "), "report.pdf");
    }

    #[test]
    fn extracts_and_sanitizes_name_from_url() {
        let url = Url::parse("https://example.com/files/My%20Report.pdf?x=1").expect("url");
        assert_eq!(file_name_from_url(&url), "My Report.pdf");

        let url = Url::parse("https://example.com/files/a%2Fb.txt").expect("url");
        assert_eq!(file_name_from_url(&url), "a_b.txt");

        let url = Url::parse("https://example.com/").expect("url");
        assert_eq!(file_name_from_url(&url), "download");

        let url = Url::parse("https://example.com/dir/").expect("url");
        assert_eq!(file_name_from_url(&url), "dir");
    }

    #[test]
    fn parses_plain_content_disposition_filename() {
        assert_eq!(
            parse_content_disposition("attachment; filename=\"report.pdf\""),
            Some("report.pdf".to_string())
        );
        assert_eq!(
            parse_content_disposition("attachment; filename=no-quotes.zip"),
            Some("no-quotes.zip".to_string())
        );
        assert_eq!(
            parse_content_disposition("attachment; filename=\"semi;colon.txt\""),
            Some("semi;colon.txt".to_string())
        );
    }

    #[test]
    fn extended_filename_wins_and_is_percent_decoded() {
        let header =
            "attachment; filename=\"fallback.zip\"; filename*=UTF-8''%D9%81%D8%A7%DB%8C%D9%84.zip";
        assert_eq!(
            parse_content_disposition(header),
            Some("فایل.zip".to_string())
        );

        let header = "attachment; FileName*=utf-8'en'a%20b.txt";
        assert_eq!(
            parse_content_disposition(header),
            Some("a b.txt".to_string())
        );
    }

    #[test]
    fn content_disposition_without_filename_is_none() {
        assert_eq!(parse_content_disposition("attachment"), None);
        assert_eq!(parse_content_disposition("inline; size=12"), None);
        assert_eq!(parse_content_disposition("attachment; filename=\"\""), None);
    }

    #[test]
    fn parses_content_range() {
        assert_eq!(
            parse_content_range("bytes 0-0/1234"),
            Some(ContentRange {
                start: 0,
                end: 0,
                total: Some(1234)
            })
        );
        assert_eq!(
            parse_content_range("bytes 100-199/*"),
            Some(ContentRange {
                start: 100,
                end: 199,
                total: None
            })
        );
        assert_eq!(
            parse_content_range("bytes  5-9 / 42 "),
            Some(ContentRange {
                start: 5,
                end: 9,
                total: Some(42)
            })
        );
    }

    #[test]
    fn rejects_malformed_content_range() {
        assert_eq!(parse_content_range("bytes */42"), None);
        assert_eq!(parse_content_range("bytes 9-5/42"), None);
        assert_eq!(parse_content_range("bytes x-y/42"), None);
        assert_eq!(parse_content_range("items 0-1/2"), None);
        assert_eq!(parse_content_range("bytes 0-1"), None);
    }
}
