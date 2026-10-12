//! Card creation/update, custom fields, attachments, boards, activity, checklists.

use std::time::Instant;

use base64::{
    Engine, alphabet,
    engine::{GeneralPurpose, GeneralPurposeConfig},
};
use reqwest::Method;
use serde_json::{Value, json};

use super::board::{match_item, names_of};
use super::{
    Args, Error, Server, boolean, copy_arg, copy_present, escape, integer, map_args, msg,
    nested_string, nested_value, object_slice, object_string, string_slice, text,
};
use crate::mcp::handlers::acked;

fn array<'a>(args: &'a Args, name: &str) -> &'a [Value] {
    args.get(name)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

impl Server {
    pub(crate) async fn create_cards(&self, args: &Args) -> Result<Value, Error> {
        let (list_id, mut refs) = self.resolve_list(args, None).await?;
        let cards = array(args, "cards");
        if cards.is_empty() {
            return Err(msg("cards must include at least one card"));
        }
        // Resolve every name first so a bad label or member creates nothing.
        let mut requests = Vec::with_capacity(cards.len());
        for (index, raw) in cards.iter().enumerate() {
            let card = raw.as_object().cloned().unwrap_or_default();
            let mut params = map_args(
                &card,
                &[
                    ("name", "name"),
                    ("description", "desc"),
                    ("due", "due"),
                    ("start", "start"),
                    ("position", "pos"),
                ],
            );
            params.insert("idList".into(), json!(list_id));
            for (input, kind, output) in [
                ("labels", "labels", "idLabels"),
                ("members", "members", "idMembers"),
            ] {
                let values = array(&card, input);
                if values.is_empty() {
                    continue;
                }
                let ids = refs
                    .ids(self, kind, values)
                    .await
                    .map_err(|err| msg(format!("cards[{index}].{input}: {err}")))?;
                params.insert(output.into(), json!(ids.join(",")));
            }
            requests.push(Value::Object(params));
        }
        let mut created = Vec::with_capacity(requests.len());
        let mut created_ids: Vec<String> = Vec::with_capacity(requests.len());
        for (index, params) in requests.into_iter().enumerate() {
            let result = self
                .write(Method::POST, "/cards", params, None)
                .await
                .map_err(|err| {
                    msg(format!(
                        "cards[{index}] failed; cards already created: [{}]: {err}",
                        created_ids.join(", ")
                    ))
                })?;
            created_ids.push(nested_string(&result, &["id"]));
            created.push(result);
        }
        Ok(Value::Array(created))
    }

    pub(crate) async fn update_card(&self, args: &Args) -> Result<Value, Error> {
        let card_id = self.allowed_card(args, "cardId").await?;
        let mut params = map_args(
            args,
            &[
                ("name", "name"),
                ("description", "desc"),
                ("due", "due"),
                ("start", "start"),
                ("dueComplete", "dueComplete"),
                ("dueReminder", "dueReminder"),
                ("position", "pos"),
                ("closed", "closed"),
                ("subscribed", "subscribed"),
            ],
        );
        let sets = [
            ("idLabels", "labels", "addLabels", "removeLabels"),
            ("idMembers", "members", "addMembers", "removeMembers"),
        ];
        let mut clears: Vec<String> = Vec::new(); // DELETE paths for removals that leave a set empty
        let list = text(args, "list");
        let needs_card = !list.is_empty()
            || sets.iter().any(|(_, _, add, remove)| {
                !array(args, add).is_empty() || !array(args, remove).is_empty()
            });
        if needs_card {
            let card = self
                .client
                .call(
                    Method::GET,
                    &format!("/cards/{}", escape(&card_id)),
                    json!({"fields": "idBoard,idLabels,idMembers"}),
                    None,
                )
                .await?;
            let mut refs = self.board_id_refs(&nested_string(&card, &["idBoard"]));
            if !list.is_empty() {
                let list_id = refs.id(self, "lists", &list).await?;
                self.ensure_list_allowed(&list_id).await?;
                params.insert("idList".into(), json!(list_id));
            }
            // The final label and member sets go in the one PUT, so the update is
            // all-or-nothing and safe to repeat.
            for (field, kind, add, remove) in sets {
                let (adds, removes) = (array(args, add), array(args, remove));
                if adds.len() + removes.len() == 0 {
                    continue;
                }
                let add_ids = refs
                    .ids(self, kind, adds)
                    .await
                    .map_err(|err| msg(format!("{add}: {err}")))?;
                let remove_ids = refs
                    .ids(self, kind, removes)
                    .await
                    .map_err(|err| msg(format!("{remove}: {err}")))?;
                let current = string_slice(card.get(field).unwrap_or(&Value::Null));
                let end = apply_set(&current, &add_ids, &remove_ids);
                if !end.is_empty() {
                    params.insert(field.into(), json!(end.join(",")));
                    continue;
                }
                // An empty set can't go in the PUT, so each removal uses Trello's
                // own DELETE; IDs already off the card need no call.
                for id in &current {
                    clears.push(format!(
                        "/cards/{}/{field}/{}",
                        escape(&card_id),
                        escape(id)
                    ));
                }
            }
        }
        if params.is_empty() && clears.is_empty() {
            return Err(msg("nothing to update"));
        }
        let mut result = json!({"id": card_id});
        if !params.is_empty() {
            result = self
                .write(
                    Method::PUT,
                    &format!("/cards/{}", escape(&card_id)),
                    Value::Object(params),
                    None,
                )
                .await?;
        }
        for path in clears {
            self.client
                .call(Method::DELETE, &path, Value::Null, None)
                .await?;
        }
        Ok(result)
    }

    pub(crate) async fn set_custom_field(&self, args: &Args) -> Result<Value, Error> {
        let mut value = text(args, "value");
        if boolean(args, "clear", false) {
            value.clear();
        } else if value.is_empty() {
            return Err(msg("give value, or clear: true"));
        }
        let card_id = self.allowed_card(args, "cardId").await?;
        let mut refs = self.card_refs(&card_id);
        let index = refs.load(self).await?;
        let fields = object_slice(index.get("customFields").unwrap_or(&Value::Null));
        let field = match_item(&fields, "customFields", &text(args, "field"))?;
        let body = custom_field_body(&field, &value)?;
        let path = format!(
            "/cards/{}/customField/{}/item",
            escape(&card_id),
            escape(&object_string(&field, &["id"]))
        );
        self.write(Method::PUT, &path, Value::Null, Some(body))
            .await
    }

    pub(crate) async fn add_attachment(&self, args: &Args) -> Result<Value, Error> {
        let card_id = self.allowed_card(args, "cardId").await?;
        let (link, file_path, data, name) = (
            text(args, "url"),
            text(args, "filePath"),
            text(args, "data"),
            text(args, "name"),
        );
        let sources = [&link, &file_path, &data]
            .iter()
            .filter(|source| !source.is_empty())
            .count();
        if sources != 1 {
            return Err(msg("give exactly one of url, filePath, or data"));
        }
        if !link.is_empty() {
            let mut params = Args::new();
            params.insert("url".into(), json!(link));
            copy_arg(&mut params, "name", args, "name");
            copy_arg(&mut params, "setCover", args, "setCover");
            return self
                .write(
                    Method::POST,
                    &format!("/cards/{}/attachments", escape(&card_id)),
                    Value::Object(params),
                    None,
                )
                .await;
        }
        if !file_path.is_empty() {
            return Ok(acked(
                self.client.upload_file(&card_id, &file_path, &name).await?,
            ));
        }
        if name.is_empty() {
            return Err(msg("name is required with data"));
        }
        let decoded = decode_base64(&data).map_err(|err| msg(format!("decode data: {err}")))?;
        Ok(acked(
            self.client.upload_data(&card_id, &name, &decoded).await?,
        ))
    }

    pub(crate) async fn set_active(
        &self,
        board_id: &str,
        workspace_id: &str,
    ) -> Result<Value, Error> {
        if board_id.is_empty() && workspace_id.is_empty() {
            return Err(msg("give boardId, workspaceId, or both"));
        }
        // Validate both before saving either.
        let mut result = Args::new();
        if !workspace_id.is_empty() {
            self.client.require_workspace_allowed(workspace_id)?;
            let workspace = self
                .client
                .call(
                    Method::GET,
                    &format!("/organizations/{}", escape(workspace_id)),
                    json!({"fields": "displayName"}),
                    None,
                )
                .await?;
            result.insert(
                "workspace".into(),
                json!(nested_string(&workspace, &["displayName"])),
            );
        }
        if !board_id.is_empty() {
            self.client.ensure_board_allowed(board_id).await?;
            let board = self
                .client
                .call(
                    Method::GET,
                    &format!("/boards/{}", escape(board_id)),
                    json!({"fields": "name"}),
                    None,
                )
                .await?;
            result.insert("board".into(), json!(nested_string(&board, &["name"])));
        }
        self.client.manager().update(|cfg| {
            if !workspace_id.is_empty() {
                cfg.workspace_id = workspace_id.to_string();
            }
            if !board_id.is_empty() {
                cfg.default_board_id = board_id.to_string();
            }
            Ok(())
        })?;
        Ok(Value::Object(result))
    }

    pub(crate) async fn list_boards(&self, workspace_id: &str) -> Result<Value, Error> {
        if !workspace_id.is_empty() {
            self.client.require_workspace_allowed(workspace_id)?;
            return Ok(self
                .client
                .call(
                    Method::GET,
                    &format!("/organizations/{}/boards", escape(workspace_id)),
                    json!({"filter": "open", "fields": "id,name"}),
                    None,
                )
                .await?);
        }
        let result = self
            .client
            .call(
                Method::GET,
                "/members/me/boards",
                json!({
                    "filter": "open", "fields": "id,name,idOrganization",
                    "organization": true, "organization_fields": "displayName"
                }),
                None,
            )
            .await?;
        let filtered = self.filter_objects_by_workspace(result, "idOrganization");
        let boards = object_slice(&filtered)
            .into_iter()
            .map(|board| {
                let mut row = Args::new();
                row.insert("id".into(), board.get("id").cloned().unwrap_or(Value::Null));
                row.insert(
                    "name".into(),
                    board.get("name").cloned().unwrap_or(Value::Null),
                );
                let workspace = nested_string(
                    &Value::Object(board.clone()),
                    &["organization", "displayName"],
                );
                if !workspace.is_empty() {
                    row.insert("workspace".into(), json!(workspace));
                }
                Value::Object(row)
            })
            .collect();
        Ok(Value::Array(boards))
    }

    pub(crate) async fn activity(&self, args: &Args) -> Result<Value, Error> {
        let card_scoped = !text(args, "cardId").is_empty();
        let path = if card_scoped {
            let card_id = self.allowed_card(args, "cardId").await?;
            format!("/cards/{}/actions", escape(&card_id))
        } else {
            let board_id = self.board(args).await?;
            format!("/boards/{}/actions", escape(&board_id))
        };
        let actions = self
            .client
            .call(
                Method::GET,
                &path,
                json!({
                    "filter": "all", "limit": integer(args, "limit", 50),
                    "memberCreator_fields": "fullName"
                }),
                None,
            )
            .await?;
        let mut result = compact_actions(&actions);
        if card_scoped {
            for action in &mut result {
                if let Some(object) = action.as_object_mut() {
                    object.remove("card");
                    object.remove("cardId");
                }
            }
        }
        Ok(Value::Array(result))
    }

    async fn all_checklists(&self, card_ref: &str, board_id: &str) -> Result<Vec<Args>, Error> {
        if !card_ref.is_empty() {
            let card_id = super::card::normalize_card_ref(card_ref)?;
            self.ensure_card_allowed(&card_id).await?;
            let value = self
                .client
                .call(
                    Method::GET,
                    &format!("/cards/{}/checklists", escape(&card_id)),
                    json!({
                        "fields": "name", "checkItems": "all",
                        "checkItem_fields": "name,state,due,idMember"
                    }),
                    None,
                )
                .await?;
            return Ok(object_slice(&value));
        }
        let mut board_args = Args::new();
        board_args.insert("boardId".into(), json!(board_id));
        let resolved = self.board(&board_args).await?;
        let value = self
            .client
            .call(
                Method::GET,
                &format!("/boards/{}/cards", escape(&resolved)),
                json!({
                    "filter": "visible", "fields": "shortLink,name", "checklists": "all",
                    "checkItem_fields": "name,state,due,idMember", "limit": 1000
                }),
                None,
            )
            .await?;
        let mut result = Vec::new();
        for card in object_slice(&value) {
            for mut checklist in object_slice(card.get("checklists").unwrap_or(&Value::Null)) {
                checklist.insert(
                    "cardId".into(),
                    card.get("shortLink").cloned().unwrap_or(Value::Null),
                );
                checklist.insert(
                    "card".into(),
                    card.get("name").cloned().unwrap_or(Value::Null),
                );
                result.push(checklist);
            }
        }
        Ok(result)
    }

    pub(crate) async fn find_checklist_items(&self, args: &Args) -> Result<Value, Error> {
        let query = text(args, "query").to_lowercase();
        let checklist_name = text(args, "checklist");
        if query.is_empty() && checklist_name.is_empty() {
            return Err(msg("give query, checklist, or both"));
        }
        let checklists = self
            .all_checklists(&text(args, "cardId"), &text(args, "boardId"))
            .await?;
        let wanted = checklist_name.to_lowercase();
        let mut matches = Vec::new();
        let mut checklist_found = false;
        let mut index: Option<Args> = None;
        for checklist in &checklists {
            if !checklist_name.is_empty()
                && object_string(checklist, &["name"]).to_lowercase() != wanted
            {
                continue;
            }
            checklist_found = true;
            // Items are grouped under their checklist so card and checklist names
            // appear once, not on every item.
            let mut items = Vec::new();
            for item in object_slice(checklist.get("checkItems").unwrap_or(&Value::Null)) {
                if !query.is_empty()
                    && !object_string(&item, &["name"])
                        .to_lowercase()
                        .contains(&query)
                {
                    continue;
                }
                let mut row = Args::new();
                copy_present(&mut row, &item, &["id", "name", "state", "due"]);
                let member_id = object_string(&item, &["idMember"]);
                if !member_id.is_empty() {
                    if index.is_none() {
                        index = Some(self.checklist_board_refs(args).load(self).await?.clone());
                    }
                    row.insert("member".into(), json!(member_id));
                    if let Some(index) = &index {
                        let names = names_of(index, "members", &json!([member_id]));
                        if let [name] = names.as_slice() {
                            row.insert("member".into(), json!(name));
                        }
                    }
                }
                items.push(Value::Object(row));
            }
            if !items.is_empty() {
                let mut group = Args::new();
                group.insert(
                    "checklistId".into(),
                    checklist.get("id").cloned().unwrap_or(Value::Null),
                );
                group.insert(
                    "checklist".into(),
                    checklist.get("name").cloned().unwrap_or(Value::Null),
                );
                group.insert("items".into(), Value::Array(items));
                copy_present(&mut group, checklist, &["cardId", "card"]);
                matches.push(Value::Object(group));
            }
        }
        if !checklist_name.is_empty() && !checklist_found {
            let mut names: Vec<String> = Vec::with_capacity(checklists.len());
            for checklist in &checklists {
                let name = object_string(checklist, &["name"]);
                if !names.contains(&name) {
                    names.push(name);
                }
            }
            return Err(msg(format!(
                "checklist {checklist_name:?} not found; checklists: {}",
                names.join(", ")
            )));
        }
        Ok(Value::Array(matches))
    }

    /// Resolves member names on the board find_checklist_items searched: the
    /// card's board, or boardId, or the active board.
    fn checklist_board_refs(&self, args: &Args) -> super::board::Refs {
        match super::card::normalize_card_ref(&text(args, "cardId")) {
            Ok(card_id) => self.card_refs(&card_id),
            Err(_) => self.board_refs(args),
        }
    }

    pub(crate) async fn create_checklist(&self, args: &Args) -> Result<Value, Error> {
        let items = array(args, "items");
        for (index, item) in items.iter().enumerate() {
            if item.as_str().is_none_or(|name| name.trim().is_empty()) {
                return Err(msg(format!("items[{index}] must be non-empty text")));
            }
        }
        let card_id = self.allowed_card(args, "cardId").await?;
        let mut params = Args::new();
        params.insert("name".into(), json!(text(args, "name")));
        copy_arg(&mut params, "pos", args, "position");
        let mut checklist = self
            .write(
                Method::POST,
                &format!("/cards/{}/checklists", escape(&card_id)),
                Value::Object(params),
                None,
            )
            .await?;
        if items.is_empty() {
            return Ok(checklist);
        }
        let checklist_id = nested_string(&checklist, &["id"]);
        if checklist_id.is_empty() {
            return Err(msg("Trello checklist response did not include an id"));
        }
        let mut created = Vec::with_capacity(items.len());
        for (index, item) in items.iter().enumerate() {
            let result = self
                .write(
                    Method::POST,
                    &format!("/checklists/{}/checkItems", escape(&checklist_id)),
                    json!({"name": item}),
                    None,
                )
                .await
                .map_err(|err| {
                    msg(format!(
                        "checklist {checklist_id} was created with items[0:{index}]; add the rest with add_checklist_item, not create_checklist: {err}"
                    ))
                })?;
            created.push(result);
        }
        if let Some(object) = checklist.as_object_mut() {
            object.insert("checkItems".into(), Value::Array(created));
        }
        Ok(checklist)
    }

    pub(crate) async fn health(&self) -> Result<Value, Error> {
        let start = Instant::now();
        let api = self
            .client
            .call(Method::GET, "/members/me", json!({"fields": "id"}), None)
            .await;
        let mut result = Args::new();
        result.insert("ok".into(), json!(api.is_ok()));
        result.insert("version".into(), json!(crate::version::CURRENT));
        result.insert(
            "latency_ms".into(),
            json!(start.elapsed().as_millis() as u64),
        );
        result.insert(
            "config_path".into(),
            json!(self.client.manager().path().display().to_string()),
        );
        if let Err(err) = api {
            result.insert("error".into(), json!(err.to_string()));
        }
        if let Ok(cfg) = self.client.manager().effective() {
            result.insert("active_board_id".into(), json!(cfg.default_board_id));
            result.insert("active_workspace_id".into(), json!(cfg.workspace_id));
            result.insert(
                "allowed_workspace_count".into(),
                json!(cfg.allowed_workspace_ids.len()),
            );
            result.insert(
                "credentials_configured".into(),
                json!(!cfg.api_key.is_empty() && !cfg.token.is_empty()),
            );
            result.insert("api_base_url".into(), json!(cfg.api_base_url));
            result.insert(
                "request_timeout_seconds".into(),
                json!(cfg.request_timeout_seconds),
            );
            result.insert("max_upload_bytes".into(), json!(cfg.max_upload_bytes));
        }
        Ok(Value::Object(result))
    }

    pub(crate) fn filter_objects_by_workspace(&self, value: Value, field: &str) -> Value {
        match self.workspace_restrictions_enabled() {
            Ok(true) => {}
            _ => return value,
        }
        let Value::Array(items) = value else {
            return value;
        };
        Value::Array(
            items
                .into_iter()
                .filter(|item| {
                    let workspace_id = nested_string(item, &[field]);
                    !workspace_id.is_empty() && self.client.workspace_allowed(&workspace_id)
                })
                .collect(),
        )
    }
}

/// Go's StdEncoding.DecodeString: skips \r and \n, ignores non-zero trailing bits.
fn decode_base64(data: &str) -> Result<Vec<u8>, base64::DecodeError> {
    let engine = GeneralPurpose::new(
        &alphabet::STANDARD,
        GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true),
    );
    engine.decode(data.replace(['\r', '\n'], ""))
}

/// Returns current plus add, minus remove, without duplicates.
pub(crate) fn apply_set(current: &[String], add: &[String], remove: &[String]) -> Vec<String> {
    let mut drop: std::collections::HashSet<&String> = remove.iter().collect();
    let mut result = Vec::with_capacity(current.len() + add.len());
    for id in current.iter().chain(add) {
        if drop.insert(id) {
            result.push(id.clone());
        }
    }
    result
}

/// Builds Trello's request body for a field definition. An empty value clears
/// the field; Trello has no DELETE for field items.
pub(crate) fn custom_field_body(field: &Args, value: &str) -> Result<Value, Error> {
    if value.is_empty() {
        return Ok(json!({"idValue": "", "value": ""}));
    }
    let name = object_string(field, &["name"]);
    match object_string(field, &["type"]).as_str() {
        kind @ ("text" | "date") => Ok(json!({"value": {kind: value}})),
        "number" => {
            if let Err(err) = value.parse::<f64>() {
                return Err(msg(format!("{name} needs a number: {err}")));
            }
            Ok(json!({"value": {"number": value}}))
        }
        "checkbox" => match value {
            "1" | "t" | "T" | "TRUE" | "true" | "True" => Ok(json!({"value": {"checked": "true"}})),
            "0" | "f" | "F" | "FALSE" | "false" | "False" => {
                Ok(json!({"value": {"checked": "false"}}))
            }
            _ => Err(msg(format!("{name} needs true or false"))),
        },
        "list" => {
            let mut options = Vec::new();
            for option in object_slice(field.get("options").unwrap_or(&Value::Null)) {
                let label = object_string(&option, &["value", "text"]);
                let id = object_string(&option, &["id"]);
                if id == value || label.to_lowercase() == value.to_lowercase() {
                    return Ok(json!({"idValue": id}));
                }
                options.push(label);
            }
            Err(msg(format!(
                "{name} option {value:?} not found; options: {}",
                options.join(", ")
            )))
        }
        kind => Err(msg(format!("unsupported custom field type {kind:?}"))),
    }
}

/// Flattens Trello actions, about 3 KiB each, to who did what to which card,
/// with each changed field's old and new value.
pub(crate) fn compact_actions(value: &Value) -> Vec<Value> {
    const PATHS: [(&str, &[&str]); 15] = [
        ("by", &["memberCreator", "fullName"]),
        ("member", &["member", "fullName"]),
        ("card", &["data", "card", "name"]),
        ("cardId", &["data", "card", "shortLink"]),
        ("list", &["data", "list", "name"]),
        ("from", &["data", "listBefore", "name"]),
        ("to", &["data", "listAfter", "name"]),
        ("text", &["data", "text"]),
        ("checklist", &["data", "checklist", "name"]),
        ("checkItem", &["data", "checkItem", "name"]),
        ("state", &["data", "checkItem", "state"]),
        ("attachment", &["data", "attachment", "name"]),
        ("attachmentId", &["data", "attachment", "id"]),
        ("label", &["data", "label", "name"]),
        ("field", &["data", "customField", "name"]),
    ];
    object_slice(value)
        .into_iter()
        .map(|action| {
            let mut compact = Args::new();
            copy_present(&mut compact, &action, &["id", "type", "date"]);
            let action = Value::Object(action);
            for (key, path) in PATHS.iter() {
                let value = nested_string(&action, path);
                if !value.is_empty() {
                    compact.insert((*key).into(), json!(value));
                }
            }
            if let Some(old) = nested_value(&action, &["data", "old"])
                .and_then(Value::as_object)
                .filter(|old| !old.is_empty())
            {
                // updateCheckItem changes data.checkItem, updateList data.list, and so on.
                let kind = nested_string(&action, &["type"]);
                let mut entity = kind.strip_prefix("update").unwrap_or(&kind).to_string();
                if let Some(first) = entity.get(..1) {
                    entity = first.to_lowercase() + &entity[1..];
                }
                let changed: Args = old
                    .iter()
                    .map(|(key, before)| {
                        let after = nested_value(&action, &["data", &entity, key])
                            .cloned()
                            .unwrap_or(Value::Null);
                        (
                            key.clone(),
                            json!({"from": shorten(before), "to": shorten(&after)}),
                        )
                    })
                    .collect();
                compact.insert("changed".into(), Value::Object(changed));
            }
            Value::Object(compact)
        })
        .collect()
}

/// Keeps long text, such as an edited description, from flooding an activity
/// entry.
pub(crate) fn shorten(value: &Value) -> Value {
    match value.as_str() {
        Some(text) if text.chars().count() > 120 => {
            json!(format!("{}…", text.chars().take(120).collect::<String>()))
        }
        _ => value.clone(),
    }
}

/// Turns bare attachment URLs into [file name](url) links. Only whole words
/// outside code spans change, so URLs already wrapped in Markdown stay as written.
pub(crate) fn link_attachments(text: &str) -> String {
    text.split('`')
        .enumerate()
        .map(|(index, part)| {
            if index % 2 == 0 {
                link_words(part)
            } else {
                part.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("`")
}

/// Applies link_attachment to each run of non-space characters. RE2's \s is
/// exactly [\t\n\f\r ].
fn link_words(part: &str) -> String {
    let mut out = String::with_capacity(part.len());
    let mut word = String::new();
    for ch in part.chars() {
        if matches!(ch, ' ' | '\t' | '\n' | '\x0C' | '\r') {
            if !word.is_empty() {
                out.push_str(&link_attachment(&word));
                word.clear();
            }
            out.push(ch);
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        out.push_str(&link_attachment(&word));
    }
    out
}

fn link_attachment(word: &str) -> String {
    let link = word.trim_end_matches(['.', ',', ';', ':', '!', '?', ')']);
    let Some(file) = attachment_file(link) else {
        return word.to_string();
    };
    if file.contains("://") {
        return word.to_string();
    }
    let name = crate::util::path_unescape(file).unwrap_or_else(|_| file.to_string());
    let target = link.replace('(', "%28").replace(')', "%29");
    format!("[{name}]({target}){}", &word[link.len()..])
}

/// Matches ^https://trello\.com/1/cards/\w+/attachments/\w+/download/(\S+)$
/// and returns the captured file part.
fn attachment_file(link: &str) -> Option<&str> {
    let rest = link.strip_prefix("https://trello.com/1/cards/")?;
    let (card, rest) = rest.split_once("/attachments/")?;
    let (attachment, file) = rest.split_once("/download/")?;
    let word = |part: &str| {
        !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    (word(card) && word(attachment) && !file.is_empty()).then_some(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::test_support::*;
    use serde_json::json;

    fn args(value: Value) -> Args {
        value.as_object().unwrap().clone()
    }

    fn encode(value: &Value) -> String {
        crate::mcp::server::encode_result(value)
    }

    #[tokio::test]
    async fn create_cards_resolves_names() {
        let h = test_server(&[], |request| {
            match (request.method.as_str(), request.url.path()) {
                ("GET", "/boards/board-1") => json_response(board_fixture()),
                ("POST", "/cards") => {
                    json_response(json!({"id": "card-1", "name": "New card", "badges": {}}))
                }
                _ => not_found(),
            }
        })
        .await;
        let result = h
            .server
            .create_cards(&args(json!({
                "list": "to do", "boardId": "board-1",
                "cards": [{"name": "New card", "labels": ["bug", "blue"], "members": ["@joe"]}]
            })))
            .await
            .unwrap();
        assert_eq!(encode(&result), r#"[{"id":"card-1","name":"New card"}]"#);
        let requests = h.api.received_requests().await.unwrap();
        let q = query(
            requests
                .iter()
                .find(|r| r.method.as_str() == "POST")
                .unwrap(),
        );
        assert_eq!(
            (
                q["idList"].as_str(),
                q["idLabels"].as_str(),
                q["idMembers"].as_str()
            ),
            ("list-todo", "label-bug,label-blue", "member-joe")
        );
    }

    #[tokio::test]
    async fn update_card_moves_labels_and_sets_reminder() {
        let h = test_server(&[], |request| match (request.method.as_str(), request.url.path()) {
            ("GET", "/cards/card-1") => json_response(
                json!({"idBoard": "board-1", "idLabels": ["label-blue"], "idMembers": []}),
            ),
            ("GET", "/boards/board-1") => json_response(board_fixture()),
            ("PUT", "/cards/card-1") => json_response(json!({
                "id": "card-1", "name": "Card", "desc": "long description", "due": "2026-11-01T00:00:00.000Z",
                "dueReminder": 1440, "idList": "list-done", "shortUrl": "https://trello.com/c/abc", "url": "https://trello.com/c/abc/1-card"
            })),
            _ => not_found(),
        })
        .await;
        let result = h
            .server
            .update_card(&args(json!({
                "cardId": "card-1", "list": "Done", "due": "2026-11-01", "dueReminder": 1440.0, "addLabels": ["Bug", "blue"]
            })))
            .await
            .unwrap();
        assert_eq!(
            encode(&result),
            r#"{"due":"2026-11-01T00:00:00.000Z","dueReminder":1440,"id":"card-1","idList":"list-done","name":"Card","shortUrl":"https://trello.com/c/abc"}"#
        );
        assert_eq!(
            calls(&h.api).await,
            vec![
                "GET /cards/card-1",
                "GET /boards/board-1",
                "PUT /cards/card-1"
            ]
        );
        let requests = h.api.received_requests().await.unwrap();
        let q = query(&requests[2]);
        assert_eq!(q["idList"], "list-done");
        assert_eq!(q["dueReminder"], "1440");
        assert_eq!(q["due"], "2026-11-01");
        assert_eq!(q["idLabels"], "label-blue,label-bug");
    }

    #[test]
    fn decode_base64_matches_go() {
        assert_eq!(decode_base64("aGVs\nbG8=\r\n").unwrap(), b"hello");
        assert_eq!(decode_base64("QR==").unwrap(), vec![0x41]);
        assert!(decode_base64("!!!").is_err());
    }

    #[test]
    fn shorten_counts_characters() {
        let long = "é".repeat(130);
        let short = shorten(&json!(long));
        assert_eq!(short.as_str().unwrap().chars().count(), 121);
        assert!(short.as_str().unwrap().ends_with('…'));
        assert_eq!(shorten(&json!(5)), json!(5));
    }

    #[tokio::test]
    async fn update_card_unknown_name_writes_nothing() {
        let h = test_server(&[], |request| {
            match (request.method.as_str(), request.url.path()) {
                ("GET", "/cards/card-1") => json_response(json!({"idBoard": "board-1"})),
                ("GET", "/boards/board-1") => json_response(board_fixture()),
                _ => not_found(),
            }
        })
        .await;
        let err = h
            .server
            .update_card(&args(
                json!({"cardId": "card-1", "name": "Renamed", "addLabels": ["Feature"]}),
            ))
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains(r#"label "Feature" not found; options: Bug, blue"#),
            "{err}"
        );
        assert!(
            calls(&h.api)
                .await
                .iter()
                .all(|call| call.starts_with("GET"))
        );
    }

    #[test]
    fn custom_field_body_cases() {
        let list = args(json!({"name": "Priority", "type": "list", "options": [
            {"id": "opt-high", "value": {"text": "High"}}
        ]}));
        let cases = [
            (&list, "high", r#"{"idValue":"opt-high"}"#),
            (&list, "", r#"{"idValue":"","value":""}"#),
            (
                &args(json!({"type": "checkbox"})),
                "TRUE",
                r#"{"value":{"checked":"true"}}"#,
            ),
            (
                &args(json!({"type": "number"})),
                "3.5",
                r#"{"value":{"number":"3.5"}}"#,
            ),
            (
                &args(json!({"type": "date"})),
                "2026-11-01",
                r#"{"value":{"date":"2026-11-01"}}"#,
            ),
        ];
        for (field, value, want) in cases {
            assert_eq!(encode(&custom_field_body(field, value).unwrap()), want);
        }
        let err = custom_field_body(&list, "Low").unwrap_err().to_string();
        assert!(err.contains("options: High"), "{err}");
    }

    #[tokio::test]
    async fn set_active_board_persists_selection() {
        let h = test_server(&[], |request| match request.url.path() {
            "/boards/board-1" => json_response(json!({"id": "board-1", "name": "Board"})),
            _ => not_found(),
        })
        .await;
        h.server.set_active("board-1", "").await.unwrap();
        assert_eq!(h.manager.file().default_board_id, "board-1");
    }

    #[tokio::test]
    async fn workspace_restriction_blocks_update_card() {
        let h = test_server(&["allowed-workspace"], |request| match request.url.path() {
            "/cards/card-1" => json_response(json!({"id": "card-1", "idBoard": "board-1"})),
            "/boards/board-1" => {
                json_response(json!({"id": "board-1", "idOrganization": "blocked-workspace"}))
            }
            _ => not_found(),
        })
        .await;
        let result = h
            .server
            .update_card(&args(json!({"cardId": "card-1", "closed": true})))
            .await;
        assert!(result.is_err());
        assert!(
            !calls(&h.api)
                .await
                .iter()
                .any(|call| call.starts_with("PUT"))
        );
    }

    #[tokio::test]
    async fn create_checklist_adds_items_and_returns_ack() {
        let h = test_server(&[], |request| match (request.method.as_str(), request.url.path()) {
            ("POST", "/cards/card-1/checklists") => json_response(
                json!({"id": "cl-1", "name": "Action Items", "idBoard": "board-1", "limits": {}}),
            ),
            ("POST", "/checklists/cl-1/checkItems") => {
                let name = query(request)["name"].clone();
                json_response(json!({
                    "id": format!("item-{name}"), "name": name, "state": "incomplete",
                    "idChecklist": "cl-1", "limits": {}
                }))
            }
            _ => not_found(),
        })
        .await;
        let result = h
            .server
            .create_checklist(&args(
                json!({"cardId": "card-1", "name": "Action Items", "items": ["a", "b"]}),
            ))
            .await
            .unwrap();
        let requests = h.api.received_requests().await.unwrap();
        let added: Vec<String> = requests
            .iter()
            .filter(|r| r.url.path() == "/checklists/cl-1/checkItems")
            .map(|r| query(r)["name"].clone())
            .collect();
        assert_eq!(added, vec!["a", "b"]);
        assert_eq!(
            encode(&result),
            r#"{"checkItems":[{"id":"item-a","name":"a","state":"incomplete"},{"id":"item-b","name":"b","state":"incomplete"}],"id":"cl-1","name":"Action Items"}"#
        );
    }

    #[test]
    fn link_attachments_cases() {
        const LINK: &str =
            "https://trello.com/1/cards/abc123/attachments/def456/download/RDS%20report.pdf";
        const PARENS: &str =
            "https://trello.com/1/cards/abc123/attachments/def456/download/Invoice%20(1).pdf";
        let cases = [
            (format!("- See {LINK}."), format!("- See [RDS report.pdf]({LINK}).")),
            (
                format!("{LINK} {LINK}"),
                format!("[RDS report.pdf]({LINK}) [RDS report.pdf]({LINK})"),
            ),
            (format!("(see {LINK})"), format!("(see [RDS report.pdf]({LINK}))")),
            (
                PARENS.to_string(),
                "[Invoice (1).pdf](https://trello.com/1/cards/abc123/attachments/def456/download/Invoice%20%281%29.pdf)".to_string(),
            ),
            (format!("[report]({LINK})"), format!("[report]({LINK})")),
            (format!("[see {LINK}]({LINK})"), format!("[see {LINK}]({LINK})")),
            (format!("<{LINK}>"), format!("<{LINK}>")),
            (format!("**{LINK}**"), format!("**{LINK}**")),
            (format!("\"{LINK}\""), format!("\"{LINK}\"")),
            (format!("`run {LINK}`"), format!("`run {LINK}`")),
            (format!("{LINK},{LINK}"), format!("{LINK},{LINK}")),
            (
                "https://github.com/x/y/1".to_string(),
                "https://github.com/x/y/1".to_string(),
            ),
        ];
        for (input, want) in cases {
            assert_eq!(link_attachments(&input), want, "input {input:?}");
        }
    }

    #[test]
    fn compact_actions_cases() {
        let got = compact_actions(&json!([{
            "id": "a-1", "type": "updateCard", "date": "2026-10-03T00:00:00Z", "limits": {},
            "memberCreator": {"fullName": "Joe", "avatarUrl": "https://example.com/a.png"},
            "data": {
                "card": {"id": "c-1", "name": "Card", "shortLink": "abc", "due": "2026-11-01"},
                "board": {"id": "b-1", "name": "Board"},
                "old": {"due": null, "dueReminder": null}
            }
        }]));
        assert_eq!(
            encode(&Value::Array(got)),
            r#"[{"by":"Joe","card":"Card","cardId":"abc","changed":{"due":{"from":null,"to":"2026-11-01"},"dueReminder":{"from":null,"to":null}},"date":"2026-10-03T00:00:00Z","id":"a-1","type":"updateCard"}]"#
        );
    }

    #[tokio::test]
    async fn set_custom_field_needs_value_or_clear() {
        let h = test_server(&[], |_| not_found()).await;
        let err = h
            .server
            .set_custom_field(&args(json!({"cardId": "card-1", "field": "points"})))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("clear: true"), "{err}");
        assert!(calls(&h.api).await.is_empty());
    }

    #[tokio::test]
    async fn update_card_removing_last_label_uses_delete() {
        let h = test_server(&[], |request| {
            match (request.method.as_str(), request.url.path()) {
                ("GET", "/cards/card-1") => {
                    json_response(json!({"idBoard": "board-1", "idLabels": ["label-bug"]}))
                }
                ("GET", "/boards/board-1") => json_response(board_fixture()),
                ("DELETE", _) => json_response(json!({"_value": null})),
                _ => not_found(),
            }
        })
        .await;
        let result = h
            .server
            .update_card(&args(
                json!({"cardId": "card-1", "removeLabels": ["Bug", "blue"]}),
            ))
            .await
            .unwrap();
        assert_eq!(
            calls(&h.api).await,
            vec![
                "GET /cards/card-1",
                "GET /boards/board-1",
                "DELETE /cards/card-1/idLabels/label-bug"
            ]
        );
        assert_eq!(nested_string(&result, &["id"]), "card-1");
    }

    #[test]
    fn compact_actions_reads_the_updated_entity() {
        let got = compact_actions(&json!([{
            "type": "updateCheckItem",
            "data": {
                "card": {"name": "Card"},
                "checkItem": {"name": "New text", "state": "incomplete"},
                "old": {"name": "Old text"}
            }
        }]));
        assert_eq!(
            encode(&Value::Array(got)),
            r#"[{"card":"Card","changed":{"name":{"from":"Old text","to":"New text"}},"checkItem":"New text","state":"incomplete","type":"updateCheckItem"}]"#
        );
    }

    #[tokio::test]
    async fn set_active_rejects_board_without_saving_workspace() {
        let h = test_server(&["workspace-1"], |request| match request.url.path() {
            "/organizations/workspace-1" => json_response(json!({"displayName": "Team"})),
            "/boards/board-x" => {
                json_response(json!({"id": "board-x", "idOrganization": "workspace-other"}))
            }
            _ => not_found(),
        })
        .await;
        let result = h.server.set_active("board-x", "workspace-1").await;
        assert!(result.is_err());
        assert_eq!(h.manager.file().workspace_id, "");
    }
}
