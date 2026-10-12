//! Board index, name-to-ID resolution, and list_cards.

use std::collections::{HashMap, hash_map::Entry};

use reqwest::Method;
use serde_json::{Value, json};

use super::{
    Args, Error, Server, compact_objects, copy_present, escape, integer, msg, nested_string,
    object_slice, string_slice, text,
};

/// String field of an object, or "" (Go's `nestedString(item, key)`).
fn field(item: &Args, key: &str) -> String {
    item.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn field_or_null(item: &Args, key: &str) -> Value {
    item.get(key).cloned().unwrap_or(Value::Null)
}

fn closed(item: &Args) -> bool {
    item.get("closed").and_then(Value::as_bool).unwrap_or(false)
}

/// Where a `Refs` finds the ID of the board its names live on.
pub(crate) enum BoardSource {
    Args(Args),
    Id(String),
    Parent(String),
}

/// Resolves list, label, member, and custom field names to IDs on one board.
/// The board is fetched once, and only when a value is not an ID.
pub(crate) struct Refs {
    source: BoardSource,
    pub(crate) index: Option<Args>,
}

impl Refs {
    fn new(source: BoardSource) -> Self {
        Self {
            source,
            index: None,
        }
    }

    async fn board_id(&self, server: &Server) -> Result<String, Error> {
        match &self.source {
            BoardSource::Args(args) => server.board(args).await,
            BoardSource::Id(id) => Ok(id.clone()),
            BoardSource::Parent(path) => {
                let object = server
                    .client
                    .call(Method::GET, path, json!({"fields": "idBoard"}), None)
                    .await?;
                match nested_string(&object, &["idBoard"]) {
                    id if id.is_empty() => Err(msg("Trello response did not include idBoard")),
                    id => Ok(id),
                }
            }
        }
    }

    pub(crate) async fn load(&mut self, server: &Server) -> Result<&Args, Error> {
        if self.index.is_none() {
            let board_id = self.board_id(server).await?;
            self.index = Some(server.board_index(&board_id).await?);
        }
        Ok(self.index.get_or_insert_with(Args::new))
    }

    /// Resolves one value of kind lists, labels, members, or customFields.
    pub(crate) async fn id(
        &mut self,
        server: &Server,
        kind: &str,
        value: &str,
    ) -> Result<String, Error> {
        let value = value.trim();
        if is_trello_id(value) {
            return Ok(value.to_string());
        }
        if kind == "members"
            && value
                .strip_prefix('@')
                .unwrap_or(value)
                .eq_ignore_ascii_case("me")
        {
            let me = server
                .client
                .call(Method::GET, "/members/me", json!({"fields": "id"}), None)
                .await?;
            return Ok(nested_string(&me, &["id"]));
        }
        let index = self.load(server).await?;
        let items = object_slice(index.get(kind).unwrap_or(&Value::Null));
        Ok(field(&match_item(&items, kind, value)?, "id"))
    }

    pub(crate) async fn ids(
        &mut self,
        server: &Server,
        kind: &str,
        values: &[Value],
    ) -> Result<Vec<String>, Error> {
        let mut result = Vec::with_capacity(values.len());
        for value in values {
            let text = match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            result.push(self.id(server, kind, &text).await?);
        }
        Ok(result)
    }
}

impl Server {
    /// Fetches everything needed to turn a board's IDs into names and back in
    /// one request: lists, labels, members, and custom fields.
    pub(crate) async fn board_index(&self, board_id: &str) -> Result<Args, Error> {
        self.client.ensure_board_allowed(board_id).await?;
        let board = self
            .client
            .call(
                Method::GET,
                &format!("/boards/{}", escape(board_id)),
                json!({
                    "fields": "name,url", "lists": "all", "list_fields": "name,closed",
                    "labels": "all", "label_fields": "name,color", "labels_limit": 1000,
                    "members": "all", "member_fields": "fullName,username", "customFields": true,
                }),
                None,
            )
            .await?;
        match board {
            Value::Object(index) => Ok(index),
            _ => Err(msg("Trello returned an invalid board response")),
        }
    }

    pub(crate) async fn board(&self, args: &Args) -> Result<String, Error> {
        let board_id = self.client.board_id(&text(args, "boardId"))?;
        self.client.ensure_board_allowed(&board_id).await?;
        Ok(board_id)
    }

    pub(crate) fn workspace_restrictions_enabled(&self) -> Result<bool, Error> {
        Ok(!self
            .client
            .manager()
            .effective()?
            .allowed_workspace_ids
            .is_empty())
    }

    /// Drops items whose board is outside the allowed workspaces.
    pub(crate) async fn filter_by_workspace(&self, value: Value) -> Result<Value, Error> {
        if !self.workspace_restrictions_enabled()? {
            return Ok(value);
        }
        let Value::Array(items) = value else {
            return Ok(value);
        };
        let mut allowed_boards: HashMap<String, bool> = HashMap::new();
        let mut filtered = Vec::with_capacity(items.len());
        for item in items {
            let board_id = nested_string(&item, &["idBoard"]);
            let allowed = match allowed_boards.get(&board_id) {
                Some(allowed) => *allowed,
                None => {
                    let allowed = self.client.ensure_board_allowed(&board_id).await.is_ok();
                    allowed_boards.insert(board_id, allowed);
                    allowed
                }
            };
            if allowed {
                filtered.push(item);
            }
        }
        Ok(Value::Array(filtered))
    }

    pub(crate) fn board_refs(&self, args: &Args) -> Refs {
        Refs::new(BoardSource::Args(args.clone()))
    }

    /// Resolves names on a board whose ID the caller already has.
    pub(crate) fn board_id_refs(&self, board_id: &str) -> Refs {
        Refs::new(BoardSource::Id(board_id.to_string()))
    }

    pub(crate) fn card_refs(&self, card_id: &str) -> Refs {
        parent_refs(format!("/cards/{}", escape(card_id)))
    }

    pub(crate) fn checklist_refs(&self, checklist_id: &str) -> Refs {
        parent_refs(format!("/checklists/{}", escape(checklist_id)))
    }

    /// Resolves the list argument and returns refs for its board, so labels and
    /// members on the same call resolve without another board fetch. A list name
    /// resolves on boardId, else on fallback when given, else on the active board.
    pub(crate) async fn resolve_list(
        &self,
        args: &Args,
        fallback: Option<Refs>,
    ) -> Result<(String, Refs), Error> {
        let value = text(args, "list");
        if value.is_empty() {
            return Err(msg("list is required"));
        }
        let mut refs = match fallback {
            _ if is_trello_id(&value) => parent_refs(format!("/lists/{}", escape(&value))),
            Some(fallback) if text(args, "boardId").is_empty() => fallback,
            _ => self.board_refs(args),
        };
        let list_id = refs.id(self, "lists", &value).await?;
        self.ensure_list_allowed(&list_id).await?;
        Ok((list_id, refs))
    }

    /// Open cards in a list, on a board, or assigned to me, with list, label,
    /// and member names in place of IDs.
    pub(crate) async fn list_cards(&self, args: &Args) -> Result<Value, Error> {
        let params = json!({
            "fields": "shortLink,name,due,start,dueComplete,idBoard,idList,idLabels,idMembers",
            "filter": "open", "limit": integer(args, "limit", 100),
        });
        let mut indexes: HashMap<String, Args> = HashMap::new();
        let list_arg = text(args, "list");
        let raw = if !list_arg.is_empty() {
            let (list_id, refs) = self.resolve_list(args, None).await?;
            if let Some(index) = refs.index {
                indexes.insert(field(&index, "id"), index);
            }
            self.client
                .call(
                    Method::GET,
                    &format!("/lists/{}/cards", escape(&list_id)),
                    params,
                    None,
                )
                .await?
        } else if !text(args, "boardId").is_empty() {
            let board_id = self.board(args).await?;
            self.client
                .call(
                    Method::GET,
                    &format!("/boards/{}/cards", escape(&board_id)),
                    params,
                    None,
                )
                .await?
        } else {
            let raw = self
                .client
                .call(Method::GET, "/members/me/cards", params, None)
                .await?;
            self.filter_by_workspace(raw).await?
        };
        let cards = object_slice(&raw);
        for card in &cards {
            let board_id = field(card, "idBoard");
            if let Entry::Vacant(slot) = indexes.entry(board_id) {
                let index = self.board_index(slot.key()).await?;
                slot.insert(index);
            }
        }
        let empty = Args::new();
        let rows = cards
            .iter()
            .map(|card| {
                let index = indexes.get(&field(card, "idBoard")).unwrap_or(&empty);
                let mut row = Args::new();
                row.insert("id".into(), field_or_null(card, "shortLink"));
                row.insert("name".into(), field_or_null(card, "name"));
                if indexes.len() > 1 {
                    row.insert("board".into(), field_or_null(index, "name"));
                }
                let id_list = Value::Array(vec![field_or_null(card, "idList")]);
                let lists = names_of(index, "lists", &id_list);
                if let [list] = lists.as_slice()
                    && list_arg.is_empty()
                {
                    row.insert("list".into(), json!(list));
                }
                copy_present(&mut row, card, &["due", "start"]);
                if card.get("dueComplete").and_then(Value::as_bool) == Some(true) {
                    row.insert("dueComplete".into(), json!(true));
                }
                let labels = names_of(index, "labels", &field_or_null(card, "idLabels"));
                if !labels.is_empty() {
                    row.insert("labels".into(), json!(labels));
                }
                let members = names_of(index, "members", &field_or_null(card, "idMembers"));
                if !members.is_empty() {
                    row.insert("members".into(), json!(members));
                }
                Value::Object(row)
            })
            .collect();
        Ok(Value::Array(rows))
    }
}

/// Resolves names on the board of the card, list, or checklist at `path`.
fn parent_refs(path: String) -> Refs {
    Refs::new(BoardSource::Parent(path))
}

/// The get_board result: open lists, labels, members, and custom fields with
/// the IDs other tools accept.
pub(crate) fn board_summary(index: &Args) -> Value {
    let null = Value::Null;
    let lists: Vec<Value> = object_slice(index.get("lists").unwrap_or(&null))
        .iter()
        .filter(|list| !closed(list))
        .map(|list| json!({"id": field_or_null(list, "id"), "name": field_or_null(list, "name")}))
        .collect();
    let fields: Vec<Value> = object_slice(index.get("customFields").unwrap_or(&null))
        .iter()
        .map(|custom| {
            let mut compact = Args::new();
            for key in ["id", "name", "type"] {
                compact.insert(key.into(), field_or_null(custom, key));
            }
            let options = object_slice(custom.get("options").unwrap_or(&null));
            if !options.is_empty() {
                let texts: Vec<String> = options
                    .iter()
                    .map(|option| {
                        option
                            .get("value")
                            .map(|value| nested_string(value, &["text"]))
                            .unwrap_or_default()
                    })
                    .collect();
                compact.insert("options".into(), json!(texts));
            }
            Value::Object(compact)
        })
        .collect();
    let mut result = Args::new();
    result.insert("lists".into(), Value::Array(lists));
    result.insert(
        "labels".into(),
        Value::Array(compact_objects(
            index.get("labels").unwrap_or(&null),
            &["id", "name", "color"],
        )),
    );
    result.insert(
        "members".into(),
        Value::Array(compact_objects(
            index.get("members").unwrap_or(&null),
            &["id", "fullName", "username"],
        )),
    );
    copy_present(&mut result, index, &["id", "name", "url"]);
    if !fields.is_empty() {
        result.insert("customFields".into(), Value::Array(fields));
    }
    Value::Object(result)
}

/// Finds the one item whose ID or name equals `value`, ignoring case. Open
/// items win over archived ones, and names win over the fallback key: username
/// for members, color for labels.
pub(crate) fn match_item(items: &[Args], kind: &str, value: &str) -> Result<Args, Error> {
    let value = value.trim();
    let mut want = value.to_lowercase();
    let (mut name_key, fallback_key) = match kind {
        "members" => ("name", "username"),
        "labels" => ("name", "color"),
        _ => ("name", ""),
    };
    if kind == "members" {
        name_key = "fullName";
        want = want.strip_prefix('@').unwrap_or(&want).to_string();
    }
    // Index 0 open, 1 archived.
    let mut by_name: [Vec<&Args>; 2] = Default::default();
    let mut by_fallback: [Vec<&Args>; 2] = Default::default();
    let mut options = Vec::with_capacity(items.len());
    for item in items {
        if field(item, "id") == value {
            return Ok(item.clone());
        }
        let state = if closed(item) {
            1
        } else {
            options.push(display_name(kind, item));
            0
        };
        if field(item, name_key).to_lowercase() == want {
            by_name[state].push(item);
        } else if !fallback_key.is_empty() && field(item, fallback_key).to_lowercase() == want {
            by_fallback[state].push(item);
        }
    }
    let singular = kind.strip_suffix('s').unwrap_or(kind);
    let [name_open, name_closed] = by_name;
    let [fallback_open, fallback_closed] = by_fallback;
    for found in [name_open, fallback_open, name_closed, fallback_closed] {
        match found.as_slice() {
            [] => continue,
            [one] => return Ok((*one).clone()),
            many => {
                return Err(msg(format!(
                    "{singular} {value:?} matches {} entries; use an ID from get_board",
                    many.len()
                )));
            }
        }
    }
    Err(msg(format!(
        "{singular} {value:?} not found; options: {}",
        options.join(", ")
    )))
}

pub(crate) fn display_name(kind: &str, item: &Args) -> String {
    match kind {
        "members" => format!("{} (@{})", field(item, "fullName"), field(item, "username")),
        "labels" => match field(item, "name") {
            name if name.is_empty() => field(item, "color"),
            name => name,
        },
        _ => field(item, "name"),
    }
}

/// Turns IDs into names using a `board_index` collection.
pub(crate) fn names_of(index: &Args, kind: &str, ids: &Value) -> Vec<String> {
    let by_id: HashMap<String, String> = object_slice(index.get(kind).unwrap_or(&Value::Null))
        .iter()
        .map(|item| {
            let name = if kind == "members" {
                field(item, "fullName")
            } else {
                display_name(kind, item)
            };
            (field(item, "id"), name)
        })
        .collect();
    string_slice(ids)
        .iter()
        .filter_map(|id| by_id.get(id).cloned())
        .collect()
}

pub(crate) fn is_trello_id(value: &str) -> bool {
    value.len() == 24
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::test_support::*;
    use serde_json::json;

    fn objects(value: Value) -> Vec<Args> {
        crate::mcp::object_slice(&value)
    }

    #[test]
    fn match_prefers_names_and_open_items() {
        let lists = objects(board_fixture()["lists"].clone());
        assert_eq!(
            match_item(&lists, "lists", "done").unwrap()["id"],
            "list-done"
        );
        let labels = objects(board_fixture()["labels"].clone());
        assert_eq!(
            match_item(&labels, "labels", "blue").unwrap()["id"],
            "label-blue"
        );
        let members = objects(board_fixture()["members"].clone());
        assert_eq!(
            match_item(&members, "members", "@JOE").unwrap()["id"],
            "member-joe"
        );
        assert_eq!(
            match_item(&lists, "lists", "Backlog")
                .unwrap_err()
                .to_string(),
            "list \"Backlog\" not found; options: To Do, Done"
        );
        let twins = objects(json!([{"id": "a", "name": "Same"}, {"id": "b", "name": "same"}]));
        assert_eq!(
            match_item(&twins, "lists", "same").unwrap_err().to_string(),
            "list \"same\" matches 2 entries; use an ID from get_board"
        );
    }

    #[test]
    fn match_prefers_names_and_open_items_go() {
        let labels = objects(json!([
            {"id": "label-named", "name": "Green", "color": "lime"},
            {"id": "label-unnamed", "name": "", "color": "green"}
        ]));
        assert_eq!(
            match_item(&labels, "labels", "green").unwrap()["id"],
            "label-named"
        );
        let lists = objects(json!([{"id": "list-q3", "name": "Q3", "closed": true}]));
        assert_eq!(match_item(&lists, "lists", "q3").unwrap()["id"], "list-q3");
        let inbox = objects(json!([{"id": "list-inbox", "name": "@inbox"}]));
        assert!(match_item(&inbox, "lists", "@inbox").is_ok());
    }

    #[test]
    fn trello_ids_are_24_lowercase_hex() {
        assert!(is_trello_id("0123456789abcdef01234567"));
        assert!(!is_trello_id("0123456789ABCDEF01234567"));
        assert!(!is_trello_id("list-todo"));
    }

    #[tokio::test]
    async fn list_cards_uses_names() {
        let h = test_server(&[], |request| match request.url.path() {
            "/boards/board-1" => json_response(board_fixture()),
            "/lists/list-todo/cards" => json_response(json!([{
                "shortLink": "abc12345", "name": "Card", "idBoard": "board-1", "idList": "list-todo",
                "idLabels": ["label-bug", "label-blue"], "idMembers": ["member-joe"], "dueComplete": true, "due": "2026-11-01"
            }])),
            _ => not_found(),
        })
        .await;
        let args = json!({"list": "To Do", "boardId": "board-1"})
            .as_object()
            .unwrap()
            .clone();
        let result = h.server.list_cards(&args).await.unwrap();
        assert_eq!(
            result,
            json!([{"id": "abc12345", "name": "Card", "due": "2026-11-01", "dueComplete": true, "labels": ["Bug", "blue"], "members": ["Joe Thai"]}])
        );
        assert_eq!(
            calls(&h.api).await,
            vec!["GET /boards/board-1", "GET /lists/list-todo/cards"]
        );
    }

    // Port of Go TestListCardsUsesNames: no list or board, so my cards.
    #[tokio::test]
    async fn list_cards_defaults_to_my_cards() {
        let h = test_server(&[], |request| match request.url.path() {
            "/members/me/cards" => json_response(json!([{
                "shortLink": "abc", "name": "Card", "idBoard": "board-1", "idList": "list-done",
                "idLabels": ["label-bug"], "idMembers": ["member-joe"], "dueComplete": false, "due": null
            }])),
            "/boards/board-1" => json_response(board_fixture()),
            _ => not_found(),
        })
        .await;
        let result = h.server.list_cards(&Args::new()).await.unwrap();
        assert_eq!(
            result,
            json!([{"id": "abc", "labels": ["Bug"], "list": "Done", "members": ["Joe Thai"], "name": "Card"}])
        );
    }
}
