//! The 28 tool definitions. Descriptions are the model's only documentation,
//! so their text must stay identical to the Go version.

use std::sync::LazyLock;

use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Str,
    Number,
    Integer,
    Bool,
    StrArray,
    ObjectArray,
    Object,
}

impl Kind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Kind::Str => "string",
            Kind::Number => "number",
            Kind::Integer => "integer",
            Kind::Bool => "boolean",
            Kind::StrArray => "string_array",
            Kind::ObjectArray => "object_array",
            Kind::Object => "object",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FieldDefinition {
    pub name: &'static str,
    pub kind: Kind,
    pub description: String,
    pub required: bool,
    pub enum_values: Vec<&'static str>,
    pub default: Option<Value>,
    pub items: Vec<FieldDefinition>,
}

#[derive(Debug, Clone)]
pub(crate) struct ToolDefinition {
    pub name: &'static str,
    pub description: String,
    pub fields: Vec<FieldDefinition>,
    pub read_only: bool,
}

/// House style for every comment posted to a card.
pub(crate) const REPLY_STYLE: &str = "Write plain English: main point first, short '- ' bullets, no jargon. Link every attachment or URL as [name](url); never paste a bare long URL.";
/// Keeps the preview rule on every tool whose text other people see.
const APPROVAL: &str = " Show the user the exact text and wait for approval first.";
const CARD_REF: &str = "Card ID, short link, or Trello card URL";
const LIST_REF: &str = "List name or ID; names resolve on boardId or the active board";
const POSITION: &str = "top, bottom, or a number";
const LABEL_COLORS: &[&str] = &[
    "yellow", "purple", "blue", "red", "green", "orange", "black", "sky", "pink", "lime",
];

fn field(
    name: &'static str,
    kind: Kind,
    description: impl Into<String>,
    required: bool,
) -> FieldDefinition {
    FieldDefinition {
        name,
        kind,
        description: description.into(),
        required,
        enum_values: Vec::new(),
        default: None,
        items: Vec::new(),
    }
}
fn required(name: &'static str, kind: Kind, description: impl Into<String>) -> FieldDefinition {
    field(name, kind, description, true)
}
fn optional(name: &'static str, kind: Kind, description: impl Into<String>) -> FieldDefinition {
    field(name, kind, description, false)
}
fn defaulted(name: &'static str, kind: Kind, description: &str, value: Value) -> FieldDefinition {
    FieldDefinition {
        default: Some(value),
        ..optional(name, kind, description)
    }
}
fn defaulted_enum(
    name: &'static str,
    description: &str,
    value: &str,
    values: &[&'static str],
) -> FieldDefinition {
    FieldDefinition {
        enum_values: values.to_vec(),
        default: Some(json!(value)),
        ..optional(name, Kind::Str, description)
    }
}
fn required_enum(
    name: &'static str,
    description: &str,
    values: &[&'static str],
) -> FieldDefinition {
    FieldDefinition {
        enum_values: values.to_vec(),
        ..required(name, Kind::Str, description)
    }
}
fn enumerated(name: &'static str, description: &str, values: &[&'static str]) -> FieldDefinition {
    FieldDefinition {
        enum_values: values.to_vec(),
        ..optional(name, Kind::Str, description)
    }
}
fn board_field() -> FieldDefinition {
    optional(
        "boardId",
        Kind::Str,
        "Board ID; defaults to the active board",
    )
}
fn tool(
    name: &'static str,
    read_only: bool,
    description: impl Into<String>,
    fields: Vec<FieldDefinition>,
) -> ToolDefinition {
    ToolDefinition {
        name,
        description: description.into(),
        fields,
        read_only,
    }
}

pub(crate) static TOOLS: LazyLock<Vec<ToolDefinition>> = LazyLock::new(tool_definitions);

fn tool_definitions() -> Vec<ToolDefinition> {
    use Kind::*;
    let approved = |text: &str| format!("{text}{APPROVAL}");
    let update_label_colors = [LABEL_COLORS, &["null"][..]].concat();
    vec![
        tool(
            "get_card",
            true,
            "Read a card as Markdown: details, custom fields, checklists with IDs, attachment links, and recent comments",
            vec![
                required("cardId", Str, CARD_REF),
                defaulted(
                    "commentsLimit",
                    Integer,
                    "Latest comments to include, 0 to 100",
                    json!(10),
                ),
                defaulted_enum(
                    "delivery",
                    "auto writes output over 16 KiB to a private file and returns its path",
                    "auto",
                    &["auto", "inline", "file"],
                ),
            ],
        ),
        tool(
            "list_cards",
            true,
            "List open cards in a list, on a board, or, with neither, assigned to me",
            vec![
                optional("list", Str, LIST_REF),
                optional("boardId", Str, "Board ID to list all its open cards"),
                defaulted("limit", Integer, "Maximum cards", json!(100)),
            ],
        ),
        tool(
            "get_activity",
            true,
            "Recent actions on a card, or on a board when cardId is omitted",
            vec![
                optional("cardId", Str, CARD_REF),
                board_field(),
                defaulted("limit", Integer, "Maximum actions", json!(50)),
            ],
        ),
        tool(
            "get_board",
            true,
            "Get a board's lists, labels, members, and custom fields with their IDs",
            vec![board_field()],
        ),
        tool(
            "list_boards",
            true,
            "List open boards, optionally in one workspace",
            vec![optional("workspaceId", Str, "Workspace ID")],
        ),
        tool("list_workspaces", true, "List workspaces", vec![]),
        tool(
            "find_checklist_items",
            true,
            "Find checklist items by text, checklist name, or both, on a card or board; results are grouped by checklist",
            vec![
                optional("query", Str, "Case-insensitive text in the item"),
                optional(
                    "checklist",
                    Str,
                    "Checklist name, such as Acceptance Criteria",
                ),
                optional(
                    "cardId",
                    Str,
                    format!("{CARD_REF}; omit to search the board"),
                ),
                board_field(),
            ],
        ),
        tool(
            "get_health",
            true,
            "Check configuration and Trello API reachability",
            vec![],
        ),
        tool(
            "download_attachment",
            false,
            "Download a card attachment to a local file",
            vec![
                required("cardId", Str, CARD_REF),
                required("attachmentId", Str, "Attachment ID"),
                required("destinationPath", Str, "Local destination path"),
            ],
        ),
        tool(
            "create_cards",
            false,
            approved("Create one or more cards in a list"),
            vec![
                required("list", Str, LIST_REF),
                board_field(),
                FieldDefinition {
                    items: vec![
                        required("name", Str, "Card title"),
                        optional("description", Str, "Markdown description"),
                        optional("due", Str, "ISO 8601 due date"),
                        optional("start", Str, "ISO 8601 start date"),
                        optional("labels", StrArray, "Label names or IDs"),
                        optional("members", StrArray, "Member names, usernames, IDs, or me"),
                        optional("position", Str, POSITION),
                    ],
                    ..required("cards", ObjectArray, "Cards to create, in order")
                },
            ],
        ),
        tool(
            "update_card",
            false,
            approved(
                "Change a card's fields, dates, list, archive or watch state, labels, or members; omitted fields stay unchanged",
            ),
            vec![
                required("cardId", Str, CARD_REF),
                optional("name", Str, "New title"),
                optional("description", Str, "New Markdown description"),
                optional("due", Str, "ISO 8601 due date; empty string clears"),
                optional("start", Str, "ISO 8601 start date; empty string clears"),
                optional("dueComplete", Bool, "Mark the due date done"),
                optional(
                    "dueReminder",
                    Integer,
                    "Reminder minutes before due; -1 for none",
                ),
                optional(
                    "list",
                    Str,
                    "Move to this list, by name or ID on the card's board",
                ),
                optional("position", Str, POSITION),
                optional("closed", Bool, "Archive (true) or restore (false)"),
                optional("subscribed", Bool, "Watch the card"),
                optional("addLabels", StrArray, "Label names or IDs to add"),
                optional("removeLabels", StrArray, "Label names or IDs to remove"),
                optional(
                    "addMembers",
                    StrArray,
                    "Member names, usernames, IDs, or me to add",
                ),
                optional("removeMembers", StrArray, "Members to remove"),
            ],
        ),
        tool(
            "set_custom_field",
            false,
            "Set or clear a custom field on a card",
            vec![
                required("cardId", Str, CARD_REF),
                required("field", Str, "Custom field name or ID"),
                optional(
                    "value",
                    Str,
                    "Text, number, true or false, ISO date, or list option",
                ),
                optional("clear", Bool, "Clear the field instead of setting a value"),
            ],
        ),
        tool(
            "copy_card",
            false,
            "Copy a card into a list",
            vec![
                required("sourceCardId", Str, CARD_REF),
                required("list", Str, LIST_REF),
                board_field(),
                optional("name", Str, "Name for the copy"),
                optional("position", Str, POSITION),
                optional(
                    "keepFromSource",
                    StrArray,
                    "Parts to copy: attachments, checklists, comments, due, labels, members, stickers",
                ),
            ],
        ),
        tool(
            "create_list",
            false,
            "Create a list on a board",
            vec![
                board_field(),
                required("name", Str, "List name"),
                optional("position", Str, POSITION),
            ],
        ),
        tool(
            "update_list",
            false,
            "Rename, move, archive, or watch a list",
            vec![
                required("list", Str, LIST_REF),
                board_field(),
                optional("name", Str, "New name"),
                optional("position", Str, POSITION),
                optional("closed", Bool, "Archive (true) or restore (false)"),
                optional("subscribed", Bool, "Watch the list"),
            ],
        ),
        tool(
            "create_board",
            false,
            "Create a board in a workspace",
            vec![
                required("name", Str, "Board name"),
                optional(
                    "workspaceId",
                    Str,
                    "Workspace ID; defaults to the active workspace",
                ),
                optional("description", Str, "Board description"),
                defaulted(
                    "defaultLists",
                    Bool,
                    "Create Trello's default lists",
                    json!(true),
                ),
            ],
        ),
        tool(
            "set_active",
            false,
            "Save the default board, workspace, or both for later calls",
            vec![
                optional("boardId", Str, "Board ID"),
                optional("workspaceId", Str, "Workspace ID"),
            ],
        ),
        tool(
            "add_comment",
            false,
            format!("Post a comment on a card. {REPLY_STYLE}{APPROVAL}"),
            vec![
                required("cardId", Str, CARD_REF),
                required("text", Str, "Comment text"),
            ],
        ),
        tool(
            "update_comment",
            false,
            format!("Edit a comment. {REPLY_STYLE}{APPROVAL}"),
            vec![
                required("commentId", Str, "Comment ID"),
                required("text", Str, "New comment text"),
            ],
        ),
        tool(
            "delete_comment",
            false,
            "Delete a comment",
            vec![required("commentId", Str, "Comment ID")],
        ),
        tool(
            "create_checklist",
            false,
            approved("Create a checklist on a card with its items"),
            vec![
                required("cardId", Str, CARD_REF),
                required("name", Str, "Checklist name"),
                optional("items", StrArray, "Item texts, in order"),
                optional("position", Str, POSITION),
            ],
        ),
        tool(
            "add_checklist_item",
            false,
            approved("Add an item to a checklist"),
            vec![
                required("checklistId", Str, "Checklist ID"),
                required("name", Str, "Item text"),
                optional("checked", Bool, "Start as done"),
                optional("position", Str, POSITION),
                optional("due", Str, "ISO 8601 due date"),
                optional(
                    "dueReminder",
                    Integer,
                    "Reminder minutes before due; -1 for none",
                ),
                optional("member", Str, "Assignee name, username, ID, or me"),
            ],
        ),
        tool(
            "update_checklist_item",
            false,
            approved("Change a checklist item's text, done state, position, due date, or assignee"),
            vec![
                required("cardId", Str, CARD_REF),
                required("checkItemId", Str, "Checklist item ID"),
                optional("name", Str, "New text"),
                optional("checked", Bool, "Done (true) or not done (false)"),
                optional("position", Str, POSITION),
                optional("due", Str, "ISO 8601 due date; empty string clears"),
                optional(
                    "dueReminder",
                    Integer,
                    "Reminder minutes before due; -1 for none",
                ),
                optional(
                    "member",
                    Str,
                    "Assignee name, username, ID, or me; empty string clears",
                ),
            ],
        ),
        tool(
            "delete_checklist_item",
            false,
            "Delete a checklist item",
            vec![
                required("checklistId", Str, "Checklist ID"),
                required("checkItemId", Str, "Checklist item ID"),
            ],
        ),
        tool(
            "create_label",
            false,
            "Create a board label",
            vec![
                board_field(),
                required("name", Str, "Label name"),
                required_enum("color", "Label color", LABEL_COLORS),
            ],
        ),
        tool(
            "update_label",
            false,
            "Rename or recolor a board label",
            vec![
                required("labelId", Str, "Label ID"),
                optional("name", Str, "New name"),
                enumerated("color", "New color; null removes it", &update_label_colors),
            ],
        ),
        tool(
            "delete_label",
            false,
            "Delete a board label",
            vec![required("labelId", Str, "Label ID")],
        ),
        tool(
            "add_attachment",
            false,
            "Attach a link, a local file, or base64 data to a card; give exactly one of url, filePath, or data",
            vec![
                required("cardId", Str, CARD_REF),
                optional("url", Str, "Link or image URL"),
                optional(
                    "filePath",
                    Str,
                    "Local file path, subject to max_upload_bytes",
                ),
                optional("data", Str, "Base64 file content"),
                optional("name", Str, "Attachment name; required with data"),
                optional("setCover", Bool, "Use the url image as the card cover"),
            ],
        ),
    ]
}
