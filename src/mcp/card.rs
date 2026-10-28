//! get_card: reference normalization, Markdown rendering, delivery files.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use reqwest::Method;
use serde_json::{Value, json};

use super::{
    Args, Error, Output, Server,
    board::{display_name, names_of},
    escape, msg, object_slice, object_string,
};
use crate::util;

const DEFAULT_FILE_THRESHOLD: usize = 16 * 1024;

#[derive(Default)]
pub(crate) struct GetCardOptions {
    pub comments_limit: i64,
    pub delivery: String,
    pub output_dir: Option<PathBuf>,
    pub file_threshold: usize,
}

impl GetCardOptions {
    fn normalized(mut self) -> Result<Self, Error> {
        if !(0..=100).contains(&self.comments_limit) {
            return Err(msg("commentsLimit must be between 0 and 100"));
        }
        self.delivery = self.delivery.trim().to_lowercase();
        if self.delivery.is_empty() {
            self.delivery = "auto".into();
        }
        if !["inline", "file", "auto"].contains(&self.delivery.as_str()) {
            return Err(msg("delivery must be inline, file, or auto"));
        }
        if self.file_threshold == 0 {
            self.file_threshold = DEFAULT_FILE_THRESHOLD;
        }
        Ok(self)
    }
}

impl Server {
    pub(crate) async fn get_card(
        &self,
        card_ref: &str,
        options: GetCardOptions,
    ) -> Result<Output, Error> {
        let card_id = normalize_card_ref(card_ref)?;
        let options = options.normalized()?;
        self.ensure_card_allowed(&card_id).await?;
        let card = self
            .client
            .call(
                Method::GET,
                &format!("/cards/{}", escape(&card_id)),
                card_request_params(&options),
                None,
            )
            .await?;
        let Value::Object(mut object) = card else {
            return Err(msg("Trello returned an invalid card response"));
        };
        // Checklist assignees need not be card members; name them from the board.
        let board_path = format!("/boards/{}", escape(&object_string(&object, &["idBoard"])));
        for checklist in object_slice(object.get("checklists").unwrap_or(&Value::Null)) {
            for item in object_slice(checklist.get("checkItems").unwrap_or(&Value::Null)) {
                let member_id = object_string(&item, &["idMember"]);
                if !member_id.is_empty()
                    && object.get("boardMembers").is_none_or(Value::is_null)
                    && names_of(&object, "members", &json!([member_id])).is_empty()
                {
                    let members = self
                        .client
                        .call(
                            Method::GET,
                            &format!("{board_path}/members"),
                            json!({"fields": "fullName"}),
                            None,
                        )
                        .await?;
                    object.insert("boardMembers".into(), members);
                }
            }
        }
        let mut fields = Vec::new();
        if !object_slice(object.get("customFieldItems").unwrap_or(&Value::Null)).is_empty() {
            let definitions = self
                .client
                .call(
                    Method::GET,
                    &format!("{board_path}/customFields"),
                    Value::Null,
                    None,
                )
                .await?;
            fields = object_slice(&definitions);
        }
        deliver_card(
            &card_id,
            &render_card_markdown(&object, &fields, options.comments_limit),
            &options,
        )
    }
}

fn card_request_params(options: &GetCardOptions) -> Value {
    let mut params = json!({
        "fields": "idBoard,name,desc,closed,due,dueComplete,dueReminder,start,shortUrl,labels",
        "list": true, "list_fields": "name", "board": true, "board_fields": "name",
        "members": true, "member_fields": "id,fullName,username",
        "attachments": true, "attachment_fields": "id,name,url",
        "checklists": "all", "checklist_fields": "name", "checkItem_fields": "id,name,state,due,idMember",
        "customFieldItems": true,
    });
    if options.comments_limit > 0 {
        params["actions"] = json!("commentCard");
        params["actions_limit"] = json!(options.comments_limit);
        params["action_fields"] = json!("id,data,date");
        params["action_memberCreator_fields"] = json!("fullName,username");
    }
    params
}

pub(crate) fn deliver_card(
    card_id: &str,
    markdown: &str,
    options: &GetCardOptions,
) -> Result<Output, Error> {
    if options.delivery == "inline"
        || (options.delivery == "auto" && markdown.len() <= options.file_threshold)
    {
        return Ok(Output::Text(markdown.to_string()));
    }
    let path = write_card_output(card_id, markdown, options.output_dir.as_deref())?;
    Ok(Output::Json(json!({
        "path": path.to_string_lossy(), "bytes": markdown.len(),
        "instruction": "The card is too large to return inline; read the Markdown file at path.",
    })))
}

fn items(object: &Args, key: &str) -> Vec<Args> {
    object_slice(object.get(key).unwrap_or(&Value::Null))
}

/// Renders a card in the order a reader needs it: status, description,
/// checklists, attachments, then comments. IDs appear only where a write tool
/// needs them.
pub(crate) fn render_card_markdown(
    card: &Args,
    field_definitions: &[Args],
    comments_limit: i64,
) -> String {
    let mut out = String::new();
    let line = |out: &mut String, label: &str, value: &str| {
        if !value.is_empty() {
            out.push_str(&format!("- {label}: {value}\n"));
        }
    };
    out.push_str(&format!("# {}\n\n", object_string(card, &["name"])));
    let board = object_string(card, &["board", "name"]);
    if !board.is_empty() {
        line(
            &mut out,
            "Board",
            &format!("{board} (board {})", object_string(card, &["idBoard"])),
        );
    }
    line(&mut out, "List", &object_string(card, &["list", "name"]));
    if card.get("closed").and_then(Value::as_bool) == Some(true) {
        line(&mut out, "Status", "Archived");
    }
    line(&mut out, "Start", &object_string(card, &["start"]));
    let mut due = object_string(card, &["due"]);
    if !due.is_empty() {
        if card.get("dueComplete").and_then(Value::as_bool) == Some(true) {
            due.push_str(" (done)");
        }
        if let Some(reminder) = card
            .get("dueReminder")
            .and_then(Value::as_f64)
            .filter(|reminder| *reminder >= 0.0)
        {
            due.push_str(&format!(
                ", reminder {} min before",
                format_minutes(reminder)
            ));
        }
        line(&mut out, "Due", &due);
    }
    let labels: Vec<String> = items(card, "labels")
        .iter()
        .map(|label| display_name("labels", label))
        .collect();
    line(&mut out, "Labels", &labels.join(", "));
    let mut member_names: HashMap<String, String> = HashMap::new();
    for member in items(card, "boardMembers") {
        member_names.insert(
            object_string(&member, &["id"]),
            object_string(&member, &["fullName"]),
        );
    }
    let mut members = Vec::new();
    for member in items(card, "members") {
        members.push(format!(
            "{} (@{})",
            object_string(&member, &["fullName"]),
            object_string(&member, &["username"])
        ));
        member_names.insert(
            object_string(&member, &["id"]),
            object_string(&member, &["fullName"]),
        );
    }
    line(&mut out, "Members", &members.join(", "));
    let definitions: HashMap<String, &Args> = field_definitions
        .iter()
        .map(|definition| (object_string(definition, &["id"]), definition))
        .collect();
    for item in items(card, "customFieldItems") {
        let definition = definitions
            .get(&object_string(&item, &["idCustomField"]))
            .copied();
        let name = definition
            .map(|d| object_string(d, &["name"]))
            .unwrap_or_default();
        line(&mut out, &name, &custom_field_text(definition, &item));
    }
    line(&mut out, "Link", &object_string(card, &["shortUrl"]));
    let description = object_string(card, &["desc"]);
    if !description.is_empty() {
        out.push_str(&format!("\n## Description\n\n{description}\n"));
    }
    for checklist in items(card, "checklists") {
        out.push_str(&format!(
            "\n## {} (checklist {})\n\n",
            object_string(&checklist, &["name"]),
            object_string(&checklist, &["id"])
        ));
        for item in items(&checklist, "checkItems") {
            let mark = if object_string(&item, &["state"]) == "complete" {
                "x"
            } else {
                " "
            };
            let mut details = format!("item {}", object_string(&item, &["id"]));
            let due = object_string(&item, &["due"]);
            if !due.is_empty() {
                details.push_str(&format!(", due {due}"));
            }
            let member_id = object_string(&item, &["idMember"]);
            if !member_id.is_empty() {
                let name = match member_names.get(&member_id) {
                    Some(name) if !name.is_empty() => name.clone(),
                    _ => format!("member {member_id}"),
                };
                details.push_str(&format!(", assigned {name}"));
            }
            out.push_str(&format!(
                "- [{mark}] {} ({details})\n",
                object_string(&item, &["name"])
            ));
        }
    }
    let attachments = items(card, "attachments");
    if !attachments.is_empty() {
        out.push_str("\n## Attachments\n\n");
        for attachment in attachments {
            out.push_str(&format!(
                "- [{}]({}) (attachment {})\n",
                object_string(&attachment, &["name"]),
                object_string(&attachment, &["url"]),
                object_string(&attachment, &["id"])
            ));
        }
    }
    let comments = items(card, "actions");
    if !comments.is_empty() {
        out.push_str("\n## Latest comments\n");
        if i64::try_from(comments.len()).is_ok_and(|count| count == comments_limit) {
            out.push_str(&format!(
                "\nShowing the latest {comments_limit}; raise commentsLimit for more.\n"
            ));
        }
        for comment in comments {
            let mut author = object_string(&comment, &["memberCreator", "fullName"]);
            let username = object_string(&comment, &["memberCreator", "username"]);
            if !username.is_empty() {
                author.push_str(&format!(" (@{username})"));
            }
            out.push_str(&format!(
                "\n### {author}, {} (comment {})\n\n{}\n",
                object_string(&comment, &["date"]),
                object_string(&comment, &["id"]),
                object_string(&comment, &["data", "text"])
            ));
        }
    }
    out.trim().to_string()
}

fn custom_field_text(definition: Option<&Args>, item: &Args) -> String {
    let option_id = object_string(item, &["idValue"]);
    if !option_id.is_empty() {
        for option in definition.map(|d| items(d, "options")).unwrap_or_default() {
            if object_string(&option, &["id"]) == option_id {
                return object_string(&option, &["value", "text"]);
            }
        }
    }
    for key in ["text", "number", "date", "checked"] {
        let text = object_string(item, &["value", key]);
        if !text.is_empty() {
            return text;
        }
    }
    String::new()
}

/// Go's %g for the reminder minutes: plain digits below 1e6, else Go's
/// exponent form. shortcut: exotic fractions may differ in the last digit from
/// Go; reminders are whole minutes, so this has not mattered.
fn format_minutes(value: f64) -> String {
    if value.abs() < 1e6 {
        return value.to_string();
    }
    let formatted = format!("{value:e}"); // "1e6"
    match formatted.split_once('e') {
        Some((mantissa, exponent)) => {
            let exponent: i32 = exponent.parse().unwrap_or(0);
            format!(
                "{mantissa}e{}{:02}",
                if exponent < 0 { '-' } else { '+' },
                exponent.abs()
            )
        }
        None => formatted,
    }
}

fn write_card_output(
    card_id: &str,
    markdown: &str,
    output_dir: Option<&Path>,
) -> Result<PathBuf, Error> {
    let dir = match output_dir {
        Some(dir) => dir.to_path_buf(),
        None => dirs::cache_dir()
            .ok_or_else(|| msg("resolve Trello MCP cache directory"))?
            .join("trello-mcp")
            .join("outputs"),
    };
    let wrap = |what: &str| {
        let what = what.to_string();
        move |err: std::io::Error| msg(format!("{what}: {err}"))
    };
    std::fs::create_dir_all(&dir).map_err(wrap("create Trello MCP output directory"))?;
    util::set_mode(&dir, 0o700).map_err(wrap("secure Trello MCP output directory"))?;
    // One file per card, replaced on each read, so the folder never holds more than one a card.
    let path = dir.join(format!("card-{}.md", safe_file_part(card_id)));
    util::write_atomic(&path, format!("{markdown}\n").as_bytes(), 0o600, ".card-")
        .map_err(wrap("save Trello MCP output"))?;
    util::absolute(&path).map_err(wrap("resolve Trello MCP output path"))
}

// Go stops after 48 output bytes; every output char is one ASCII byte, so take 48 chars.
fn safe_file_part(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .take(48)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-');
    if trimmed.is_empty() {
        "card".into()
    } else {
        trimmed.into()
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

    use crate::mcp::test_support::{calls, json_response, not_found, query, test_server};
    use serde_json::{Value, json};

    fn args(value: Value) -> Args {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn normalize_card_ref_accepts_ids_links_and_urls() {
        for (input, want) in [
            ("6a595e613293292bd6ef5a4c", "6a595e613293292bd6ef5a4c"),
            ("7yNeeFiE", "7yNeeFiE"),
            (
                "https://trello.com/c/7yNeeFiE/430-example?menu=filter#comment-1",
                "7yNeeFiE",
            ),
            ("trello.com/c/7yNeeFiE/430-example", "7yNeeFiE"),
        ] {
            assert_eq!(normalize_card_ref(input).unwrap(), want, "{input}");
        }
    }

    #[test]
    fn normalize_card_ref_rejects_non_card_urls() {
        for value in [
            "https://example.com/c/7yNeeFiE",
            "https://trello.com/b/board-id",
            "card/id",
        ] {
            assert!(normalize_card_ref(value).is_err(), "{value}");
        }
    }

    #[tokio::test]
    async fn get_card_compact_uses_one_direct_request() {
        let h = test_server(&[], |request| {
            if request.url.path() == "/boards/board-1/customFields" {
                return json_response(
                    json!([{"id": "field-1", "name": "Priority", "type": "text"}]),
                );
            }
            if request.method.as_str() != "GET" || request.url.path() != "/cards/7yNeeFiE" {
                return not_found();
            }
            json_response(json!({
                "id": "card-1", "idBoard": "board-1", "idList": "list-1", "shortLink": "7yNeeFiE",
                "name": "Card name", "desc": "Card description", "closed": false,
                "list": {"id": "list-1", "name": "Doing"},
                "url": "https://trello.com/c/7yNeeFiE/card-name",
                "cover": {"scaled": ["large preview data"]},
                "labels": [{"id": "label-1", "name": "Urgent", "color": "red", "uses": 99}],
                "attachments": [{
                    "id": "attachment-1", "name": "spec.pdf", "url": "https://example.test/spec.pdf",
                    "mimeType": "application/pdf", "previews": ["large preview data"]
                }],
                "members": [{"id": "member-1", "fullName": "Test User", "username": "test", "avatarUrl": "large"}],
                "checklists": [{
                    "id": "checklist-1", "name": "Acceptance Criteria", "idBoard": "board-1",
                    "checkItems": [{"id": "item-1", "name": "Works", "state": "incomplete", "idChecklist": "checklist-1"}]
                }],
                "customFieldItems": [{"id": "custom-1", "idCustomField": "field-1", "value": {"text": "High"}, "extra": "drop"}],
                "actions": [{
                    "id": "action-1", "idMemberCreator": "member-1", "date": "2026-07-23T00:00:00.000Z",
                    "data": {
                        "text": "A useful comment",
                        "card": {"id": "card-1", "name": "duplicated card"},
                        "board": {"id": "board-1", "name": "duplicated board"}
                    },
                    "memberCreator": {"id": "member-1", "fullName": "Test User", "username": "test", "avatarUrl": "large"},
                    "display": {"translationKey": "large duplicated display metadata"}
                }]
            }))
        })
        .await;
        let result = h
            .server
            .handle(
                "get_card",
                &args(json!({"cardId": "https://trello.com/c/7yNeeFiE/430-card-name"})),
            )
            .await
            .unwrap();
        let requests = h.api.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2, "want 2 (card, custom field definitions)");
        let card_request = requests
            .iter()
            .find(|r| r.url.path() == "/cards/7yNeeFiE")
            .unwrap();
        let params = query(card_request);
        let fields = &params["fields"];
        assert!(!fields.is_empty() && fields != "all", "fields = {fields:?}");
        assert_eq!(params["actions"], "commentCard");
        assert_eq!(params["actions_limit"], "10");
        assert_ne!(params["attachment_fields"], "all");
        let Output::Text(markdown) = result else {
            panic!("want text output")
        };
        for want in [
            "# Card name",
            "- List: Doing",
            "- Labels: Urgent",
            "- Members: Test User (@test)",
            "- Priority: High",
            "## Acceptance Criteria (checklist checklist-1)",
            "- [ ] Works (item item-1)",
            "- [spec.pdf](https://example.test/spec.pdf) (attachment attachment-1)",
            "### Test User (@test), 2026-07-23T00:00:00.000Z (comment action-1)",
            "A useful comment",
        ] {
            assert!(markdown.contains(want), "missing {want:?}:\n{markdown}");
        }
        for dropped in ["large preview data", "duplicated", "avatar"] {
            assert!(!markdown.contains(dropped), "contains {dropped:?}");
        }
    }

    #[tokio::test]
    async fn get_card_with_workspace_restriction_keeps_authorization_preflight() {
        let h = test_server(&["workspace-1"], |request| {
            let fields = query(request).get("fields").cloned().unwrap_or_default();
            match request.url.path() {
                "/cards/card-1" if fields == "id,idBoard" => {
                    json_response(json!({"id": "card-1", "idBoard": "board-1"}))
                }
                "/boards/board-1" => {
                    json_response(json!({"id": "board-1", "idOrganization": "workspace-1"}))
                }
                "/cards/card-1" => json_response(json!({
                    "id": "card-1", "idBoard": "board-1", "name": "Allowed card", "actions": []
                })),
                _ => not_found(),
            }
        })
        .await;
        h.server
            .handle("get_card", &args(json!({"cardId": "card-1"})))
            .await
            .unwrap();
        let requests = calls(&h.api).await;
        assert_eq!(requests.len(), 3, "{requests:?}");
        assert!(
            !requests.join("\n").contains("/actions"),
            "comments were fetched separately: {requests:?}"
        );
    }

    #[tokio::test]
    async fn get_card_defaults_to_markdown_auto_inline() {
        let h = test_server(&[], |_| {
            json_response(json!({
                "id": "card-1", "name": "Markdown card", "desc": "Useful description",
                "url": "https://trello.com/c/markdown", "actions": []
            }))
        })
        .await;
        let result = h
            .server
            .handle("get_card", &args(json!({"cardId": "card-1"})))
            .await
            .unwrap();
        let Output::Text(markdown) = result else {
            panic!("want text output")
        };
        assert!(markdown.contains("# Markdown card") && !markdown.contains("\"name\""));
    }

    #[test]
    fn deliver_card_modes() {
        let card = "# Delivery card\n\nUseful details";
        let options = |delivery: &str, file_threshold, output_dir| GetCardOptions {
            delivery: delivery.into(),
            file_threshold,
            output_dir,
            comments_limit: 10,
        };
        assert!(matches!(
            deliver_card("card-1", card, &options("inline", 1, None)).unwrap(),
            Output::Text(_)
        ));
        assert!(matches!(
            deliver_card("card-1", card, &options("auto", 1 << 20, None)).unwrap(),
            Output::Text(_)
        ));

        let dir = tempfile::tempdir().unwrap();
        let output_dir = dir.path().join("outputs");
        let Output::Json(result) = deliver_card(
            "card-1",
            card,
            &options("auto", 1, Some(output_dir.clone())),
        )
        .unwrap() else {
            panic!("want file result")
        };
        let path = Path::new(result["path"].as_str().unwrap());
        assert_eq!(path.parent().unwrap(), util::absolute(&output_dir).unwrap());
        assert_eq!(path.extension().unwrap(), "md");
        assert!(
            std::fs::read_to_string(path)
                .unwrap()
                .contains("# Delivery card")
        );
        assert_eq!(result["bytes"], card.len());
        assert_eq!(
            result["instruction"],
            "The card is too large to return inline; read the Markdown file at path."
        );
    }

    #[tokio::test]
    async fn get_card_rejects_invalid_options_before_calling_trello() {
        let h = test_server(&[], |_| {
            wiremock::ResponseTemplate::new(500).set_body_string("unexpected request")
        })
        .await;
        for bad in [
            json!({"cardId": "card-1", "commentsLimit": 101}),
            json!({"cardId": "card-1", "delivery": "elsewhere"}),
        ] {
            assert!(h.server.handle("get_card", &args(bad)).await.is_err());
            assert!(calls(&h.api).await.is_empty());
        }
    }

    #[tokio::test]
    async fn get_card_names_checklist_assignees_from_the_board() {
        let h = test_server(&[], |request| match request.url.path() {
            "/cards/card-1" => json_response(json!({
                "idBoard": "board-1", "name": "Card",
                "checklists": [{"id": "cl-1", "name": "Tasks", "checkItems": [
                    {"id": "item-1", "name": "Ship", "state": "incomplete", "idMember": "member-anna"},
                    {"id": "item-2", "name": "Test", "state": "incomplete", "idMember": "member-gone"}
                ]}]
            })),
            "/boards/board-1/members" => {
                json_response(json!([{"id": "member-anna", "fullName": "Anna"}]))
            }
            _ => not_found(),
        })
        .await;
        let Output::Text(markdown) = h
            .server
            .handle("get_card", &args(json!({"cardId": "card-1"})))
            .await
            .unwrap()
        else {
            panic!("want text output")
        };
        for want in [
            "- [ ] Ship (item item-1, assigned Anna)",
            "- [ ] Test (item item-2, assigned member member-gone)",
        ] {
            assert!(markdown.contains(want), "missing {want:?}:\n{markdown}");
        }
    }

    #[test]
    fn format_minutes_matches_go_percent_g() {
        assert_eq!(format_minutes(1440.0), "1440");
        assert_eq!(format_minutes(0.0), "0");
        assert_eq!(format_minutes(90.5), "90.5");
        assert_eq!(format_minutes(1_000_000.0), "1e+06");
    }

    #[test]
    fn safe_file_part_replaces_unsafe_characters() {
        assert_eq!(safe_file_part("abc/../x y"), "abc----x-y");
        assert_eq!(safe_file_part("///"), "card");
        assert_eq!(safe_file_part(&"a".repeat(60)).len(), 48);
        assert_eq!(safe_file_part(&("a".repeat(47) + "/b")), "a".repeat(47));
    }

    #[test]
    fn delivered_card_keeps_one_file_per_card() {
        let dir = tempfile::tempdir().unwrap();
        let options = GetCardOptions {
            delivery: "file".into(),
            output_dir: Some(dir.path().to_path_buf()),
            file_threshold: 1,
            comments_limit: 10,
        };
        let path = |out: Output| match out {
            Output::Json(v) => v["path"].as_str().unwrap().to_string(),
            Output::Text(_) => panic!("want a file result"),
        };
        let first = path(deliver_card("card-1", "# Old", &options).unwrap());
        let second = path(deliver_card("card-1", "# New", &options).unwrap());
        assert_eq!(first, second);
        assert!(first.ends_with("/card-card-1.md"), "{first}");
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "# New\n");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn delivered_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let options = GetCardOptions {
            delivery: "file".into(),
            output_dir: Some(dir.path().join("outputs")),
            file_threshold: 1,
            comments_limit: 10,
        };
        let Output::Json(result) = deliver_card("card-1", "# Card", &options).unwrap() else {
            panic!("expected a file result")
        };
        let path = Path::new(result["path"].as_str().unwrap());
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "# Card\n");
    }
}
