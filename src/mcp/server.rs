//! MCP protocol glue: JSON Schemas from the tool table, argument validation,
//! and the rmcp ServerHandler.

use std::sync::Arc;

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
        GetPromptRequestParams, GetPromptResponse, GetPromptResult, Implementation,
        ListPromptsResult, ListToolsResult, PaginatedRequestParams, Prompt, PromptArgument,
        PromptMessage, Role, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
    },
    service::RequestContext,
};
use serde_json::{Map, Value, json};

use super::{
    Args, Output, Server,
    tools::{FieldDefinition, Kind, REPLY_STYLE, TOOLS},
};
use crate::version;

pub(crate) const INSTRUCTIONS: &str = "Trello tools. Cards take an ID, short link, or URL; lists, labels, members, and custom fields take a name or ID. Board tools default to the active board. Before any change other people see (comments, new or updated cards, checklists), show the user a preview and wait for approval. Keep text short: main point first.";

pub(crate) fn input_schema(fields: &[FieldDefinition]) -> Map<String, Value> {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for field in fields {
        let mut property = Map::new();
        property.insert("description".into(), json!(field.description));
        match field.kind {
            Kind::StrArray => {
                property.insert("type".into(), json!("array"));
                property.insert("items".into(), json!({"type": "string"}));
            }
            Kind::ObjectArray => {
                property.insert("type".into(), json!("array"));
                let items = if field.items.is_empty() {
                    json!({"type": "object"})
                } else {
                    Value::Object(input_schema(&field.items))
                };
                property.insert("items".into(), items);
            }
            Kind::Object => {
                property.insert("type".into(), json!("object"));
                property.insert("additionalProperties".into(), json!(true));
            }
            kind => {
                property.insert("type".into(), json!(kind.as_str()));
            }
        }
        if !field.enum_values.is_empty() {
            property.insert("enum".into(), json!(field.enum_values));
        }
        if let Some(default) = &field.default {
            property.insert("default".into(), default.clone());
        }
        properties.insert(field.name.into(), Value::Object(property));
        if field.required {
            required.push(field.name);
        }
    }
    let mut schema = Map::new();
    schema.insert("type".into(), json!("object"));
    schema.insert("properties".into(), Value::Object(properties));
    schema.insert("additionalProperties".into(), json!(false));
    if !required.is_empty() {
        schema.insert("required".into(), json!(required));
    }
    schema
}

pub(crate) fn validate_arguments(fields: &[FieldDefinition], args: &Args) -> Result<(), String> {
    for field in fields {
        let missing = match args.get(field.name) {
            None | Some(Value::Null) => true,
            Some(Value::String(text)) => text.trim().is_empty(),
            Some(_) => false,
        };
        if field.required && missing {
            return Err(format!("argument {:?} is required", field.name));
        }
    }
    for (name, value) in args {
        let Some(field) = fields.iter().find(|field| field.name == name) else {
            return Err(format!("unknown argument {name:?}"));
        };
        if value.is_null() {
            continue;
        }
        validate_kind(field, value)?;
        if !field.items.is_empty()
            && let Value::Array(items) = value
        {
            for (index, item) in items.iter().enumerate() {
                let object = item.as_object().cloned().unwrap_or_default();
                validate_arguments(&field.items, &object)
                    .map_err(|err| format!("{}[{index}]: {err}", field.name))?;
            }
        }
    }
    Ok(())
}

fn validate_kind(field: &FieldDefinition, value: &Value) -> Result<(), String> {
    let valid = match field.kind {
        Kind::Str => value.is_string(),
        Kind::Bool => value.is_boolean(),
        Kind::Number | Kind::Integer => match value.as_f64() {
            Some(number) if field.kind == Kind::Integer && number.trunc() != number => {
                return Err(format!("argument {:?} must be an integer", field.name));
            }
            Some(_) => true,
            None => false,
        },
        Kind::StrArray | Kind::ObjectArray => value.is_array(),
        Kind::Object => value.is_object(),
    };
    if !valid {
        return Err(format!(
            "argument {:?} must be {}",
            field.name,
            field.kind.as_str()
        ));
    }
    if !field.enum_values.is_empty() && !field.enum_values.contains(&value.as_str().unwrap_or("")) {
        return Err(format!(
            "argument {:?} must be one of {}",
            field.name,
            field.enum_values.join(", ")
        ));
    }
    Ok(())
}

/// Compact JSON. serde_json never HTML-escapes, so `&` stays `&` for the model.
pub(crate) fn encode_result(value: &Value) -> String {
    value.to_string()
}

/// Sorted by name: go-sdk lists tools that way.
pub(crate) fn tool_list() -> Vec<Tool> {
    let mut tools: Vec<Tool> = TOOLS
        .iter()
        .map(|definition| {
            let tool = Tool::new(
                definition.name,
                definition.description.clone(),
                Arc::new(input_schema(&definition.fields)),
            );
            if definition.read_only {
                tool.with_annotations(ToolAnnotations::new().read_only(true))
            } else {
                tool
            }
        })
        .collect();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    tools
}

pub(crate) fn prompt_list() -> Vec<Prompt> {
    vec![Prompt::new(
        "draft_reply",
        Some("Draft a reply comment for a Trello card and post it only after approval"),
        Some(vec![
            PromptArgument::new("card")
                .with_description("Card ID, short link, or Trello card URL")
                .with_required(true),
            PromptArgument::new("notes").with_description("What the reply should say"),
        ]),
    )]
}

pub(crate) fn draft_reply_text(card: &str, notes: &str) -> String {
    let mut text = format!(
        "Draft a reply comment for Trello card {card}.\n1. Read the card with get_card.\n2. {REPLY_STYLE}\n3. Show me the draft. Post it with add_comment only after I approve.\n"
    );
    let notes = notes.trim();
    if !notes.is_empty() {
        text.push_str(&format!("\nWhat to say: {notes}\n"));
    }
    text
}

fn tool_error(message: impl Into<String>) -> CallToolResponse {
    CallToolResult::error(vec![ContentBlock::text(message.into())]).into()
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new("trello-mcp", version::CURRENT))
        .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(tool_list()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let Some(definition) = TOOLS.iter().find(|tool| tool.name == request.name) else {
            return Err(McpError::invalid_params(
                format!("unknown tool {:?}", request.name),
                None,
            ));
        };
        let args = request.arguments.unwrap_or_default();
        if let Err(message) = validate_arguments(&definition.fields, &args) {
            return Ok(tool_error(message));
        }
        Ok(match self.handle(definition.name, &args).await {
            Ok(Output::Text(text)) => {
                CallToolResult::success(vec![ContentBlock::text(text)]).into()
            }
            Ok(Output::Json(value)) => {
                CallToolResult::success(vec![ContentBlock::text(encode_result(&value))]).into()
            }
            Err(err) => tool_error(err.to_string()),
        })
    }

    async fn list_prompts(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        Ok(ListPromptsResult::with_all_items(prompt_list()))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, McpError> {
        draft_reply_prompt(&request.name, request.arguments.as_ref()).map(Into::into)
    }
}

fn draft_reply_prompt(
    name: &str,
    arguments: Option<&Map<String, Value>>,
) -> Result<GetPromptResult, McpError> {
    if name != "draft_reply" {
        return Err(McpError::invalid_params(
            format!("unknown prompt {name:?}"),
            None,
        ));
    }
    let argument = |key: &str| {
        arguments
            .and_then(|args| args.get(key))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let card = argument("card");
    if card.is_empty() {
        return Err(McpError::invalid_params(
            format!("argument {:?} is required", "card"),
            None,
        ));
    }
    Ok(GetPromptResult::new(vec![PromptMessage::new_text(
        Role::User,
        draft_reply_text(&card, &argument("notes")),
    )])
    .with_description("Draft a Trello reply"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::tools::TOOLS;
    use serde_json::json;

    #[test]
    fn tools_and_prompts_match_go_fixture() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/tools_list.json")).unwrap();
        let actual = json!({
            "instructions": INSTRUCTIONS,
            "tools": serde_json::to_value(tool_list()).unwrap(),
            "prompts": serde_json::to_value(prompt_list()).unwrap(),
        });
        assert_eq!(actual["instructions"], fixture["instructions"]);
        let (got, want) = (
            actual["tools"].as_array().unwrap(),
            fixture["tools"].as_array().unwrap(),
        );
        assert_eq!(got.len(), want.len());
        for (got, want) in got.iter().zip(want) {
            assert_eq!(got, want, "tool {} differs", want["name"]);
        }
        assert_eq!(actual["prompts"], fixture["prompts"]);
    }

    #[test]
    fn tool_definitions_are_complete_and_unique() {
        let mut names: Vec<_> = TOOLS.iter().map(|t| t.name).collect();
        assert_eq!(names.len(), 28);
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 28);
        assert!(TOOLS.iter().all(|t| !t.description.is_empty()));
    }

    #[test]
    fn create_cards_schema_types_each_card() {
        let tool = TOOLS.iter().find(|t| t.name == "create_cards").unwrap();
        let schema = Value::Object(input_schema(&tool.fields));
        assert_eq!(
            schema["properties"]["cards"]["items"]["required"],
            json!(["name"])
        );
        assert_eq!(
            schema["properties"]["cards"]["items"]["properties"]["labels"]["items"]["type"],
            "string"
        );
    }

    #[test]
    fn validation_messages_match_go() {
        let tool = TOOLS.iter().find(|t| t.name == "create_cards").unwrap();
        let args = |v: Value| v.as_object().unwrap().clone();
        assert_eq!(
            validate_arguments(&tool.fields, &args(json!({"cards": []}))).unwrap_err(),
            "argument \"list\" is required"
        );
        assert_eq!(
            validate_arguments(&tool.fields, &args(json!({"list": " ", "cards": []}))).unwrap_err(),
            "argument \"list\" is required"
        );
        assert_eq!(
            validate_arguments(
                &tool.fields,
                &args(json!({"list": "a", "cards": [], "x": 1}))
            )
            .unwrap_err(),
            "unknown argument \"x\""
        );
        assert_eq!(
            validate_arguments(&tool.fields, &args(json!({"list": "a", "cards": "no"})))
                .unwrap_err(),
            "argument \"cards\" must be object_array"
        );
        assert_eq!(
            validate_arguments(&tool.fields, &args(json!({"list": "a", "cards": [{}]})))
                .unwrap_err(),
            "cards[0]: argument \"name\" is required"
        );
        let get_card = TOOLS.iter().find(|t| t.name == "get_card").unwrap();
        assert_eq!(
            validate_arguments(
                &get_card.fields,
                &args(json!({"cardId": "c", "commentsLimit": 1.5}))
            )
            .unwrap_err(),
            "argument \"commentsLimit\" must be an integer"
        );
        assert_eq!(
            validate_arguments(
                &get_card.fields,
                &args(json!({"cardId": "c", "delivery": "x"}))
            )
            .unwrap_err(),
            "argument \"delivery\" must be one of auto, inline, file"
        );
        assert!(
            validate_arguments(
                &get_card.fields,
                &args(json!({"cardId": "c", "commentsLimit": 3.0, "delivery": null}))
            )
            .is_ok()
        );
    }

    #[test]
    fn encode_result_keeps_ampersands() {
        assert_eq!(
            encode_result(&json!({"name": "Terms & <Privacy>"})),
            r#"{"name":"Terms & <Privacy>"}"#
        );
    }

    #[test]
    fn draft_reply_prompt_checks_name_and_card() {
        let args = |v: Value| v.as_object().unwrap().clone();
        let err = draft_reply_prompt("foo", Some(&args(json!({"card": "abc"})))).unwrap_err();
        assert!(
            err.message.contains("unknown prompt \"foo\""),
            "{}",
            err.message
        );
        let err = draft_reply_prompt("draft_reply", None).unwrap_err();
        assert!(err.message.contains("argument \"card\" is required"));
        let ok = draft_reply_prompt("draft_reply", Some(&args(json!({"card": " abc "})))).unwrap();
        assert_eq!(ok.messages.len(), 1);
    }

    #[test]
    fn draft_reply_text_matches_go() {
        assert_eq!(
            draft_reply_text("abc", " say hi "),
            "Draft a reply comment for Trello card abc.\n1. Read the card with get_card.\n2. Write plain English: main point first, short '- ' bullets, no jargon. Link every attachment or URL as [name](url); never paste a bare long URL.\n3. Show me the draft. Post it with add_comment only after I approve.\n\nWhat to say: say hi\n"
        );
    }
}
