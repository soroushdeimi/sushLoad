//! Building the shared HTTP client from the proxy settings, and describing network errors without
//! ever leaking proxy credentials.

use crate::model::{ProxyMode, Settings, validate_proxy_url};
use reqwest::Client;
use std::ops::Deref;
use std::time::Duration;

/// A ready client plus what is needed to explain errors that came through the proxy.
#[derive(Clone)]
pub(crate) struct Net {
    client: Client,
    /// `scheme://host:port` of the custom proxy (never any userinfo).
    proxy_label: Option<String>,
    /// Raw and decoded passwords to scrub from any message.
    secrets: Vec<String>,
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
        })
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
    Client::builder().connect_timeout(Duration::from_secs(15))
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
