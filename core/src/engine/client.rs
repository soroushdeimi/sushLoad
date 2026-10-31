//! Building the shared HTTP client from the proxy settings, and describing network errors without
//! ever leaking proxy credentials.

use crate::model::{ProxyMode, Settings, validate_proxy_url};
use reqwest::header::{COOKIE, HeaderValue, REFERER, USER_AGENT};
use reqwest::{Client, RequestBuilder, Url};
use std::ops::Deref;
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) \
     Version/17.0 Safari/605.1.15 sushLoad/0.1";

/// Browser context for one download: sent on every request to the download's own origin
/// (`Referer`, `User-Agent`) and, for `Cookie`, only to that origin.
#[derive(Clone, Default)]
pub(crate) struct RequestContext {
    pub referer: Option<String>,
    pub cookies: Option<String>,
    pub user_agent: Option<String>,
    /// `scheme://host:port` of the URL the download was added with.
    pub origin: Option<String>,
}

impl RequestContext {
    pub fn new(
        url: &str,
        referer: Option<String>,
        cookies: Option<String>,
        user_agent: Option<String>,
    ) -> RequestContext {
        RequestContext {
            referer,
            cookies,
            user_agent,
            origin: Url::parse(url).ok().map(|url| origin_of(&url)),
        }
    }
}

/// `scheme://host:port` (default ports filled in) for same-origin checks.
pub(crate) fn origin_of(url: &Url) -> String {
    format!(
        "{}://{}:{}",
        url.scheme(),
        url.host_str().unwrap_or_default().to_ascii_lowercase(),
        url.port_or_known_default().unwrap_or(0)
    )
}

/// A ready client plus what is needed to explain errors that came through the proxy.
#[derive(Clone)]
pub(crate) struct Net {
    client: Client,
    /// `scheme://host:port` of the custom proxy (never any userinfo).
    proxy_label: Option<String>,
    /// Raw and decoded passwords to scrub from any message.
    secrets: Vec<String>,
    context: Option<Arc<RequestContext>>,
}

impl Deref for Net {
    type Target = Client;
    fn deref(&self) -> &Client {
        &self.client
    }
}

impl Net {
    /// Client using reqwest's defaults (system proxy settings).
    pub fn direct() -> Result<Net, String> {
        let client = base_builder()
            .build()
            .map_err(|e| format!("cannot create HTTP client: {e}"))?;
        Ok(Net {
            client,
            proxy_label: None,
            secrets: Vec::new(),
            context: None,
        })
    }

    /// A copy of this client that sends the browser context of one download.
    pub fn with_context(&self, context: RequestContext) -> Net {
        let mut net = self.clone();
        if let Some(cookies) = context.cookies.as_deref().filter(|c| !c.is_empty()) {
            net.secrets.push(cookies.to_string());
        }
        net.context = Some(Arc::new(context));
        net
    }

    /// GET `url` with the default User-Agent, or the download's browser context when set.
    /// `Cookie` is added only when `url` has the download's own origin; redirects to other
    /// origins are stripped of it by the client's redirect policy.
    pub fn get(&self, url: &str) -> RequestBuilder {
        let mut request = self.client.get(url);
        let mut user_agent = HeaderValue::from_static(DEFAULT_USER_AGENT);
        if let Some(context) = &self.context {
            if let Some(value) = context
                .user_agent
                .as_deref()
                .and_then(|v| HeaderValue::from_str(v).ok())
            {
                user_agent = value;
            }
            if let Some(value) = context
                .referer
                .as_deref()
                .and_then(|v| HeaderValue::from_str(v).ok())
            {
                request = request.header(REFERER, value);
            }
            let same_origin = Url::parse(url)
                .ok()
                .is_some_and(|url| context.origin.as_deref() == Some(origin_of(&url).as_str()));
            if same_origin
                && let Some(mut value) = context
                    .cookies
                    .as_deref()
                    .filter(|v| !v.is_empty())
                    .and_then(|v| HeaderValue::from_str(v).ok())
            {
                value.set_sensitive(true);
                request = request.header(COOKIE, value);
            }
        }
        request.header(USER_AGENT, user_agent)
    }

    /// True when a custom proxy is in use and `error` is a failure to connect.
    pub fn is_proxy_unreachable(&self, error: &reqwest::Error) -> bool {
        self.proxy_label.is_some() && error.is_connect()
    }

    /// Message for a request error; mentions the proxy when one is configured and the
    /// connection failed, and never contains the proxy password.
    pub fn describe(&self, error: &reqwest::Error) -> String {
        let mut message = error.to_string();
        let mut source = std::error::Error::source(error);
        while let Some(cause) = source {
            message.push_str(": ");
            message.push_str(&cause.to_string());
            source = cause.source();
        }
        if let Some(label) = &self.proxy_label
            && error.is_connect()
        {
            message = format!("Proxy {label}: cannot connect ({message})");
        }
        self.scrub(message)
    }

    pub fn scrub(&self, message: String) -> String {
        scrub_all(&message, &self.secrets)
    }
}

fn base_builder() -> reqwest::ClientBuilder {
    // `referer(false)`: keep the Referer the browser sent across redirects instead of
    // replacing it with the previous hop. reqwest itself drops `Cookie` (and `Authorization`)
    // on a redirect to another scheme/host/port (see `remove_sensitive_headers`).
    Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .referer(false)
}

/// Builds the client for `settings`: system proxy, no proxy, or the custom proxy URL.
pub(crate) fn build_client(settings: &Settings) -> Result<Net, String> {
    match settings.proxy_mode {
        ProxyMode::System => Net::direct(),
        ProxyMode::Off => {
            let client = base_builder()
                .no_proxy()
                .build()
                .map_err(|e| format!("cannot create HTTP client: {e}"))?;
            Ok(Net {
                client,
                proxy_label: None,
                secrets: Vec::new(),
                context: None,
            })
        }
        ProxyMode::Custom => {
            let url = settings.proxy_url.trim();
            validate_proxy_url(url)?;
            let (label, secrets) = split_credentials(url);
            let proxy =
                reqwest::Proxy::all(url).map_err(|e| scrub_all(&e.to_string(), &secrets))?;
            let client = base_builder()
                .no_proxy()
                .proxy(proxy)
                .build()
                .map_err(|e| scrub_all(&format!("cannot create HTTP client: {e}"), &secrets))?;
            Ok(Net {
                client,
                proxy_label: Some(label),
                secrets,
                context: None,
            })
        }
    }
}

fn scrub_all(message: &str, secrets: &[String]) -> String {
    let mut message = message.to_string();
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        message = message.replace(secret.as_str(), "***");
    }
    message
}

/// Returns `scheme://host:port` without userinfo, and the password in raw and decoded form.
fn split_credentials(url: &str) -> (String, Vec<String>) {
    let Some((scheme, rest)) = url.split_once("://") else {
        return (url.to_string(), Vec::new());
    };
    let (userinfo, host_port) = match rest.rsplit_once('@') {
        Some((userinfo, host_port)) => (Some(userinfo), host_port),
        None => (None, rest),
    };
    let mut secrets = Vec::new();
    if let Some(userinfo) = userinfo
        && let Some((_, password)) = userinfo.split_once(':')
    {
        secrets.push(password.to_string());
        let decoded = percent_encoding::percent_decode_str(password)
            .decode_utf8_lossy()
            .into_owned();
        if decoded != password {
            secrets.push(decoded);
        }
    }
    let label = format!(
        "{}://{}",
        scheme.to_ascii_lowercase(),
        host_port.trim_end_matches('/')
    );
    (label, secrets)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom(url: &str) -> Settings {
        Settings {
            proxy_mode: ProxyMode::Custom,
            proxy_url: url.to_string(),
            ..Settings::default()
        }
    }

    #[test]
    fn credentials_are_split_from_the_label() {
        let (label, secrets) = split_credentials("socks5h://bob:p%40ss@10.0.0.1:1080/");
        assert_eq!(label, "socks5h://10.0.0.1:1080");
        assert!(secrets.contains(&"p%40ss".to_string()));
        assert!(secrets.contains(&"p@ss".to_string()));
    }

    #[test]
    fn bad_custom_url_is_an_error_without_secrets() {
        let error = build_client(&custom("ftp://bob:hunter2@h:21"))
            .err()
            .unwrap();
        assert!(!error.contains("hunter2"));
        assert!(build_client(&custom("socks5://u:pw@127.0.0.1:1080")).is_ok());
        assert!(build_client(&Settings::default()).is_ok());
    }

    #[test]
    fn scrub_removes_passwords() {
        let net = build_client(&custom("http://u:hunter2@127.0.0.1:8080")).unwrap();
        assert_eq!(net.scrub("failed hunter2 here".into()), "failed *** here");
    }
}
