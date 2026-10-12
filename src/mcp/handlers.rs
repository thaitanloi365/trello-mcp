//! Tool dispatch, write acknowledgements, and workspace allow-list checks.

use reqwest::Method;
use serde_json::{Value, json};

use super::{
    Args, Error, Output, Server, boolean,
    card::{GetCardOptions, normalize_card_ref},
    compact_objects, copy_arg, copy_present, escape,
    extras::link_attachments,
    integer, map_args, msg, nested_string, text, text_default,
};

impl Server {
    pub(crate) async fn handle(&self, name: &str, args: &Args) -> Result<Output, Error> {
        if name == "get_card" {
            let options = GetCardOptions {
                comments_limit: integer(args, "commentsLimit", 10),
                delivery: text_default(args, "delivery", "auto"),
                ..Default::default()
            };
            return self.get_card(&text(args, "cardId"), options).await;
        }
        self.handle_json(name, args).await.map(Output::Json)
    }

    async fn handle_json(&self, name: &str, args: &Args) -> Result<Value, Error> {
        match name {
            "list_cards" => self.list_cards(args).await,
            "get_activity" => self.activity(args).await,
            "get_board" => {
                let board_id = self.board(args).await?;
                let index = self.board_index(&board_id).await?;
                Ok(super::board::board_summary(&index))
            }
            "list_boards" => self.list_boards(&text(args, "workspaceId")).await,
            "list_workspaces" => {
                let result = self
                    .client
                    .call(
                        Method::GET,
                        "/members/me/organizations",
                        json!({"fields": "id,displayName"}),
                        None,
                    )
                    .await?;
                Ok(self.filter_objects_by_workspace(result, "id"))
            }
            "find_checklist_items" => self.find_checklist_items(args).await,
            "get_health" => self.health().await,
            "download_attachment" => {
                let card_id = self.allowed_card(args, "cardId").await?;
                Ok(self
                    .client
                    .download_attachment(
                        &card_id,
                        &text(args, "attachmentId"),
                        &text(args, "destinationPath"),
                    )
                    .await?)
            }
            "create_cards" => self.create_cards(args).await,
            "update_card" => self.update_card(args).await,
            "set_custom_field" => self.set_custom_field(args).await,
            "copy_card" => {
                let source_ref = self.allowed_card(args, "sourceCardId").await?;
                // idCardSource must be the full ID, and a list name means the source card's board.
                let source = self
                    .client
                    .call(
                        Method::GET,
                        &format!("/cards/{}", escape(&source_ref)),
                        json!({"fields": "id,idBoard"}),
                        None,
                    )
                    .await?;
                let refs = self.board_id_refs(&nested_string(&source, &["idBoard"]));
                let (list_id, _) = self.resolve_list(args, Some(refs)).await?;
                let mut params = map_args(args, &[("name", "name"), ("position", "pos")]);
                params.insert(
                    "idCardSource".into(),
                    nested_string(&source, &["id"]).into(),
                );
                params.insert("idList".into(), list_id.into());
                if let Some(Value::Array(values)) = args.get("keepFromSource") {
                    params.insert("keepFromSource".into(), join_values(values).into());
                }
                self.write(Method::POST, "/cards", Value::Object(params), None)
                    .await
            }
            "create_list" => {
                let board_id = self.board(args).await?;
                let mut params = Args::new();
                params.insert("idBoard".into(), board_id.into());
                params.insert("name".into(), text(args, "name").into());
                copy_arg(&mut params, "pos", args, "position");
                self.write(Method::POST, "/lists", Value::Object(params), None)
                    .await
            }
            "update_list" => {
                let (list_id, _) = self.resolve_list(args, None).await?;
                let params = map_args(
                    args,
                    &[
                        ("name", "name"),
                        ("position", "pos"),
                        ("closed", "closed"),
                        ("subscribed", "subscribed"),
                    ],
                );
                self.write(
                    Method::PUT,
                    &format!("/lists/{}", escape(&list_id)),
                    Value::Object(params),
                    None,
                )
                .await
            }
            "create_board" => {
                let workspace_id = self.workspace_id(args).await?;
                let mut params = Args::new();
                params.insert("name".into(), text(args, "name").into());
                params.insert("idOrganization".into(), workspace_id.into());
                params.insert(
                    "defaultLists".into(),
                    boolean(args, "defaultLists", true).into(),
                );
                copy_arg(&mut params, "desc", args, "description");
                self.write(Method::POST, "/boards", Value::Object(params), None)
                    .await
            }
            "set_active" => {
                self.set_active(&text(args, "boardId"), &text(args, "workspaceId"))
                    .await
            }
            "add_comment" => {
                let card_id = self.allowed_card(args, "cardId").await?;
                self.write(
                    Method::POST,
                    &format!("/cards/{}/actions/comments", escape(&card_id)),
                    json!({"text": link_attachments(&text(args, "text"))}),
                    None,
                )
                .await
            }
            "update_comment" | "delete_comment" => {
                let comment_id = text(args, "commentId");
                self.ensure_action_allowed(&comment_id).await?;
                let (method, params) = if name == "update_comment" {
                    (
                        Method::PUT,
                        json!({"text": link_attachments(&text(args, "text"))}),
                    )
                } else {
                    (Method::DELETE, Value::Null)
                };
                self.write(
                    method,
                    &format!("/actions/{}/comments", escape(&comment_id)),
                    params,
                    None,
                )
                .await
            }
            "create_checklist" => self.create_checklist(args).await,
            "add_checklist_item" => {
                let checklist_id = text(args, "checklistId");
                self.ensure_checklist_allowed(&checklist_id).await?;
                let mut params = map_args(
                    args,
                    &[
                        ("name", "name"),
                        ("checked", "checked"),
                        ("position", "pos"),
                        ("due", "due"),
                        ("dueReminder", "dueReminder"),
                    ],
                );
                let member = text(args, "member");
                if !member.is_empty() {
                    let member_id = self
                        .checklist_refs(&checklist_id)
                        .id(self, "members", &member)
                        .await?;
                    params.insert("idMember".into(), member_id.into());
                }
                self.write(
                    Method::POST,
                    &format!("/checklists/{}/checkItems", escape(&checklist_id)),
                    Value::Object(params),
                    None,
                )
                .await
            }
            "update_checklist_item" => {
                let card_id = self.allowed_card(args, "cardId").await?;
                let mut params = map_args(
                    args,
                    &[
                        ("name", "name"),
                        ("position", "pos"),
                        ("due", "due"),
                        ("dueReminder", "dueReminder"),
                    ],
                );
                if let Some(Value::Bool(checked)) = args.get("checked") {
                    let state = if *checked { "complete" } else { "incomplete" };
                    params.insert("state".into(), state.into());
                }
                if args.contains_key("member") {
                    let member = text(args, "member");
                    let member_id = if member.is_empty() {
                        String::new()
                    } else {
                        self.card_refs(&card_id)
                            .id(self, "members", &member)
                            .await?
                    };
                    params.insert("idMember".into(), member_id.into());
                }
                self.write(
                    Method::PUT,
                    &format!(
                        "/cards/{}/checkItem/{}",
                        escape(&card_id),
                        escape(&text(args, "checkItemId"))
                    ),
                    Value::Object(params),
                    None,
                )
                .await
            }
            "delete_checklist_item" => {
                let checklist_id = text(args, "checklistId");
                self.ensure_checklist_allowed(&checklist_id).await?;
                self.write(
                    Method::DELETE,
                    &format!(
                        "/checklists/{}/checkItems/{}",
                        escape(&checklist_id),
                        escape(&text(args, "checkItemId"))
                    ),
                    Value::Null,
                    None,
                )
                .await
            }
            "create_label" => {
                let board_id = self.board(args).await?;
                self.write(
                    Method::POST,
                    "/labels",
                    json!({"idBoard": board_id, "name": text(args, "name"), "color": text(args, "color")}),
                    None,
                )
                .await
            }
            "update_label" | "delete_label" => {
                let label_id = text(args, "labelId");
                self.ensure_label_allowed(&label_id).await?;
                let path = format!("/labels/{}", escape(&label_id));
                if name == "delete_label" {
                    return self.write(Method::DELETE, &path, Value::Null, None).await;
                }
                let params = map_args(args, &[("name", "name"), ("color", "color")]);
                self.write(Method::PUT, &path, Value::Object(params), None)
                    .await
            }
            "add_attachment" => self.add_attachment(args).await,
            _ => Err(msg(format!("tool {name:?} is not implemented"))),
        }
    }

    /// Normalizes a card reference argument and applies the workspace allow-list.
    pub(crate) async fn allowed_card(&self, args: &Args, key: &str) -> Result<String, Error> {
        let card_id = normalize_card_ref(&text(args, key))?;
        self.ensure_card_allowed(&card_id).await?;
        Ok(card_id)
    }

    pub(crate) async fn workspace_id(&self, args: &Args) -> Result<String, Error> {
        let mut workspace_id = text(args, "workspaceId");
        if workspace_id.is_empty() {
            workspace_id = self.client.manager().effective()?.workspace_id;
        }
        self.client.require_workspace_allowed(&workspace_id)?;
        Ok(workspace_id)
    }

    /// Calls a mutating endpoint and returns a short ack instead of the full
    /// Trello object, which can run to several KiB; get_card has the full view.
    pub(crate) async fn write(
        &self,
        method: Method,
        path: &str,
        params: Value,
        body: Option<Value>,
    ) -> Result<Value, Error> {
        Ok(acked(
            self.client
                .call(method, path, params, body.as_ref())
                .await?,
        ))
    }

    async fn ensure_allowed(
        &self,
        path: String,
        fields: &str,
        board_path: &[&str],
        missing: &str,
    ) -> Result<(), Error> {
        if !self.workspace_restrictions_enabled()? {
            return Ok(());
        }
        let object = self
            .client
            .call(Method::GET, &path, json!({"fields": fields}), None)
            .await?;
        let board_id = nested_string(&object, board_path);
        if board_id.is_empty() {
            return Err(msg(missing));
        }
        Ok(self.client.ensure_board_allowed(&board_id).await?)
    }

    pub(crate) async fn ensure_card_allowed(&self, id: &str) -> Result<(), Error> {
        self.ensure_allowed(
            format!("/cards/{}", escape(id)),
            "id,idBoard",
            &["idBoard"],
            "Trello card response did not include idBoard",
        )
        .await
    }

    pub(crate) async fn ensure_list_allowed(&self, id: &str) -> Result<(), Error> {
        self.ensure_allowed(
            format!("/lists/{}", escape(id)),
            "id,idBoard",
            &["idBoard"],
            "Trello list response did not include idBoard",
        )
        .await
    }

    pub(crate) async fn ensure_checklist_allowed(&self, id: &str) -> Result<(), Error> {
        self.ensure_allowed(
            format!("/checklists/{}", escape(id)),
            "id,idBoard",
            &["idBoard"],
            "Trello checklist response did not include idBoard",
        )
        .await
    }

    pub(crate) async fn ensure_action_allowed(&self, id: &str) -> Result<(), Error> {
        self.ensure_allowed(
            format!("/actions/{}", escape(id)),
            "data",
            &["data", "board", "id"],
            "Trello action response did not include a board ID",
        )
        .await
    }

    pub(crate) async fn ensure_label_allowed(&self, id: &str) -> Result<(), Error> {
        self.ensure_allowed(
            format!("/labels/{}", escape(id)),
            "id,idBoard",
            &["idBoard"],
            "Trello label response did not include idBoard",
        )
        .await
    }
}

/// Strings as-is, anything else in its JSON form (Go's fmt.Sprint).
pub(crate) fn join_values(values: &[Value]) -> String {
    values
        .iter()
        .map(|value| match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// A short ack instead of the full Trello object, which can run to several
/// KiB; get_card has the full view.
pub(crate) fn acked(value: Value) -> Value {
    let Value::Object(object) = &value else {
        return value;
    };
    let mut result = Args::new();
    copy_present(
        &mut result,
        object,
        &[
            "id",
            "name",
            "shortUrl",
            "idList",
            "due",
            "start",
            "dueComplete",
            "dueReminder",
            "closed",
            "state",
            "color",
            "value",
            "date",
        ],
    );
    if !result.contains_key("shortUrl") {
        copy_present(&mut result, object, &["url"]);
    }
    if let Some(items) = object.get("checkItems") {
        result.insert(
            "checkItems".into(),
            Value::Array(compact_objects(items, &["id", "name", "state"])),
        );
    }
    if result.is_empty() {
        return json!({"ok": true});
    }
    Value::Object(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::test_support::*;
    use serde_json::json;

    fn args(value: Value) -> Args {
        value.as_object().unwrap().clone()
    }

    #[tokio::test]
    async fn workspace_restriction_blocks_card_mutation() {
        let h = test_server(&["allowed-workspace"], |request| match request.url.path() {
            "/cards/card-1" => json_response(json!({"id": "card-1", "idBoard": "board-1"})),
            "/boards/board-1" => json_response(
                json!({"id": "board-1", "idOrganization": "other-workspace", "name": "B"}),
            ),
            _ => not_found(),
        })
        .await;
        let err = h
            .server
            .handle(
                "add_comment",
                &args(json!({"cardId": "card-1", "text": "hi"})),
            )
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "board \"board-1\" belongs to workspace \"other-workspace\", which is not allowed"
        );
        assert!(
            !calls(&h.api)
                .await
                .iter()
                .any(|call| call.starts_with("POST"))
        );
    }

    #[tokio::test]
    async fn copy_card_uses_full_source_id_and_source_board() {
        let h = test_server(&[], |request| {
            match (request.method.as_str(), request.url.path()) {
                ("GET", "/cards/abc12345") => {
                    json_response(json!({"id": "0123456789abcdef01234567", "idBoard": "board-1"}))
                }
                ("GET", "/boards/board-1") => json_response(board_fixture()),
                ("POST", "/cards") => json_response(json!({"id": "copy-1", "name": "Copy"})),
                _ => not_found(),
            }
        })
        .await;
        let result = h
            .server
            .handle(
                "copy_card",
                &args(json!({"sourceCardId": "abc12345", "list": "Done", "keepFromSource": ["labels", "due"]})),
            )
            .await
            .unwrap();
        assert!(
            matches!(result, Output::Json(ref v) if *v == json!({"id": "copy-1", "name": "Copy"}))
        );
        let requests = h.api.received_requests().await.unwrap();
        let post = requests
            .iter()
            .find(|r| r.method.as_str() == "POST")
            .unwrap();
        let q = query(post);
        assert_eq!(q["idCardSource"], "0123456789abcdef01234567");
        assert_eq!(q["idList"], "list-done");
        assert_eq!(q["keepFromSource"], "labels,due");
    }

    #[test]
    fn acked_keeps_short_fields() {
        assert_eq!(
            acked(json!({"id": "x", "url": "u", "badges": {}})),
            json!({"id": "x", "url": "u"})
        );
        assert_eq!(acked(json!({"badges": {}})), json!({"ok": true}));
        assert_eq!(acked(json!([1])), json!([1]));
    }

    #[tokio::test]
    async fn unknown_tool_name_reports_not_implemented() {
        let h = test_server(&[], |_| not_found()).await;
        assert_eq!(
            h.server
                .handle("nope", &Args::new())
                .await
                .unwrap_err()
                .to_string(),
            "tool \"nope\" is not implemented"
        );
    }
}
