//! get_card: reference normalization, Markdown rendering, delivery files.

use std::path::PathBuf;

use super::{Error, Output, Server, msg};

#[derive(Default)]
pub(crate) struct GetCardOptions {
    pub comments_limit: i64,
    pub delivery: String,
    pub output_dir: Option<PathBuf>,
    pub file_threshold: usize,
}

impl Server {
    pub(crate) async fn get_card(
        &self,
        _card_ref: &str,
        _options: GetCardOptions,
    ) -> Result<Output, Error> {
        todo!("task 8")
    }
}

/// Host as written in the input (Go's `Hostname()`): no userinfo, port or IPv6 brackets.
fn raw_hostname(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if let Some(inner) = host.strip_prefix('[') {
        return inner.split(']').next().unwrap_or("").to_string();
    }
    host.rsplit_once(':')
        .map_or(host, |(name, _)| name)
        .to_string()
}

pub(crate) fn normalize_card_ref(value: &str) -> Result<String, Error> {
    let mut value = value.trim().to_string();
    if value.is_empty() {
        return Err(msg("card reference is required"));
    }
    let lower = value.to_lowercase();
    if lower.starts_with("trello.com/") || lower.starts_with("www.trello.com/") {
        value = format!("https://{value}");
    }
    if let Ok(parsed) = reqwest::Url::parse(&value)
        && let Some(host) = parsed.host_str().filter(|host| !host.is_empty())
    {
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err(msg("Trello card URL must use http or https"));
        }
        let host = host.to_lowercase();
        if host != "trello.com" && host != "www.trello.com" {
            return Err(msg(format!(
                "unsupported Trello card URL host {:?}",
                raw_hostname(&value)
            )));
        }
        let parts: Vec<&str> = parsed.path().trim_matches('/').split('/').collect();
        if parts.len() < 2 || !parts[0].eq_ignore_ascii_case("c") {
            return Err(msg(
                "Trello card URL must have the form trello.com/c/{shortLink}",
            ));
        }
        // Go's url.Parse rejects bad escapes, so it reaches the generic message below.
        value = crate::util::path_unescape(parts[1])
            .map_err(|_| msg("card reference must be a card ID, short link, or Trello card URL"))?;
    }
    if value.contains(['/', '?', '#']) {
        return Err(msg(
            "card reference must be a card ID, short link, or Trello card URL",
        ));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_host_keeps_input_case() {
        for (input, host) in [
            ("https://GitHub.com/c/abc", "GitHub.com"),
            ("https://u:p@GitHub.com:8080/c/abc", "GitHub.com"),
        ] {
            assert_eq!(
                normalize_card_ref(input).unwrap_err().to_string(),
                format!("unsupported Trello card URL host {host:?}")
            );
        }
    }

    #[test]
    fn bad_escape_in_short_link_gets_generic_message() {
        assert_eq!(
            normalize_card_ref("https://trello.com/c/%zz")
                .unwrap_err()
                .to_string(),
            "card reference must be a card ID, short link, or Trello card URL"
        );
    }
}
