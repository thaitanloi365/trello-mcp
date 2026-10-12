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
            return Err(msg(format!("unsupported Trello card URL host {host:?}")));
        }
        let parts: Vec<&str> = parsed.path().trim_matches('/').split('/').collect();
        if parts.len() < 2 || !parts[0].eq_ignore_ascii_case("c") {
            return Err(msg(
                "Trello card URL must have the form trello.com/c/{shortLink}",
            ));
        }
        value = crate::util::path_unescape(parts[1])
            .map_err(|err| msg(format!("decode Trello short link: {err}")))?;
    }
    if value.contains(['/', '?', '#']) {
        return Err(msg(
            "card reference must be a card ID, short link, or Trello card URL",
        ));
    }
    Ok(value)
}
