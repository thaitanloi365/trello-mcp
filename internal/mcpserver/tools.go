package mcpserver

func required(name string, kind fieldKind, description string) fieldDefinition {
	return fieldDefinition{Name: name, Kind: kind, Description: description, Required: true}
}

func optional(name string, kind fieldKind, description string) fieldDefinition {
	return fieldDefinition{Name: name, Kind: kind, Description: description}
}

func enumerated(name, description string, values ...string) fieldDefinition {
	return fieldDefinition{Name: name, Kind: kindString, Description: description, Enum: values}
}

func defaultedEnum(name, description, value string, values ...string) fieldDefinition {
	return fieldDefinition{Name: name, Kind: kindString, Description: description, Enum: values, Default: value}
}

func requiredEnum(name, description string, values ...string) fieldDefinition {
	return fieldDefinition{Name: name, Kind: kindString, Description: description, Required: true, Enum: values}
}

func defaulted(name string, kind fieldKind, description string, value any) fieldDefinition {
	return fieldDefinition{Name: name, Kind: kind, Description: description, Default: value}
}

func boardField() fieldDefinition {
	return optional("boardId", kindString, "Trello board ID; uses the configured active board when omitted")
}

func cardOutputSchema() map[string]any {
	return map[string]any{
		"type": "object",
		"properties": map[string]any{
			"id":                map[string]any{"type": "string"},
			"shortLink":         map[string]any{"type": "string"},
			"name":              map[string]any{"type": "string"},
			"desc":              map[string]any{"type": "string"},
			"url":               map[string]any{"type": "string"},
			"closed":            map[string]any{"type": "boolean"},
			"labels":            map[string]any{"type": "array"},
			"members":           map[string]any{"type": "array"},
			"attachments":       map[string]any{"type": "array"},
			"checklists":        map[string]any{"type": "array"},
			"comments":          map[string]any{"type": "array"},
			"customFieldItems":  map[string]any{"type": "array"},
			"commentsTruncated": map[string]any{"type": "boolean"},
			"delivery":          map[string]any{"type": "string"},
			"format":            map[string]any{"type": "string"},
			"path":              map[string]any{"type": "string"},
			"bytes":             map[string]any{"type": "integer"},
			"generatedAt":       map[string]any{"type": "string"},
		},
		"additionalProperties": true,
	}
}

func toolDefinitions() []toolDefinition {
	return []toolDefinition{
		{Name: "get_cards_by_list_id", Description: "Get cards from a Trello list", Fields: []fieldDefinition{
			required("listId", kindString, "ID of the list"), optional("limit", kindInteger, "Maximum cards to return"),
		}},
		{Name: "get_lists", Description: "Get open lists on a board", Fields: []fieldDefinition{boardField()}},
		{Name: "get_recent_activity", Description: "Get recent actions on a board", Fields: []fieldDefinition{
			boardField(), defaulted("limit", kindInteger, "Maximum actions to return", 50),
		}},
		{Name: "add_card_to_list", Description: "Create a card in a list", Fields: []fieldDefinition{
			required("listId", kindString, "ID of the destination list"), required("name", kindString, "Card name"),
			optional("description", kindString, "Card description"), optional("due", kindString, "ISO 8601 due date"),
			optional("start", kindString, "Start date in YYYY-MM-DD format"), optional("position", kindString, "top, bottom, or a numeric position"),
		}},
		{Name: "update_card_details", Description: "Update card fields", Fields: []fieldDefinition{
			required("cardId", kindString, "ID of the card"), optional("name", kindString, "New card name"),
			optional("description", kindString, "New card description"), optional("due", kindString, "ISO 8601 due date, or empty string to clear"),
			optional("start", kindString, "Start date, or empty string to clear"), optional("dueComplete", kindBoolean, "Whether the due date is complete"),
			optional("closed", kindBoolean, "Whether the card is archived"), optional("position", kindString, "top, bottom, or a numeric position"),
		}},
		{Name: "archive_card", Description: "Archive a card", Fields: []fieldDefinition{required("cardId", kindString, "ID of the card")}},
		{Name: "watch_card", Description: "Subscribe or unsubscribe the current member from a card", Fields: []fieldDefinition{
			required("cardId", kindString, "ID of the card"), defaulted("subscribed", kindBoolean, "Whether to watch the card", true),
		}},
		{Name: "watch_list", Description: "Subscribe or unsubscribe the current member from a list", Fields: []fieldDefinition{
			required("listId", kindString, "ID of the list"), defaulted("subscribed", kindBoolean, "Whether to watch the list", true),
		}},
		{Name: "move_card", Description: "Move a card to another list", Fields: []fieldDefinition{
			required("cardId", kindString, "ID of the card"), required("listId", kindString, "Destination list ID"), optional("position", kindString, "top, bottom, or a numeric position"),
		}},
		{Name: "add_list_to_board", Description: "Create a list on a board", Fields: []fieldDefinition{
			boardField(), required("name", kindString, "List name"), optional("position", kindString, "top, bottom, or a numeric position"),
		}},
		{Name: "archive_list", Description: "Archive a list", Fields: []fieldDefinition{required("listId", kindString, "ID of the list")}},
		{Name: "update_list", Description: "Update a list name or archived state", Fields: []fieldDefinition{
			required("listId", kindString, "ID of the list"), optional("name", kindString, "New list name"), optional("closed", kindBoolean, "Whether the list is archived"),
		}},
		{Name: "update_list_position", Description: "Move a list within its board", Fields: []fieldDefinition{
			required("listId", kindString, "ID of the list"), required("position", kindString, "top, bottom, or a numeric position"),
		}},
		{Name: "get_my_cards", Description: "Get cards assigned to the authenticated member", Fields: []fieldDefinition{
			defaulted("limit", kindInteger, "Maximum cards to return", 100),
		}},
		{Name: "attach_image_to_card", Description: "Attach an image URL to a card", Fields: []fieldDefinition{
			required("cardId", kindString, "ID of the card"), required("imageUrl", kindString, "Public image URL"),
			defaulted("name", kindString, "Attachment name", "Image Attachment"), optional("setCover", kindBoolean, "Use the image as the card cover"),
		}},
		{Name: "attach_file_to_card", Description: "Attach a local file to a card, subject to max_upload_bytes", Fields: []fieldDefinition{
			required("cardId", kindString, "ID of the card"), required("filePath", kindString, "Absolute or working-directory-relative local file path"), optional("name", kindString, "Attachment name"),
		}},
		{Name: "attach_data_to_card", Description: "Attach base64-encoded data to a card", Fields: []fieldDefinition{
			required("cardId", kindString, "ID of the card"), required("data", kindString, "Base64-encoded attachment data"), required("name", kindString, "Attachment file name"),
		}},
		{Name: "attach_image_data_to_card", Description: "Attach base64-encoded image data to a card", Fields: []fieldDefinition{
			required("cardId", kindString, "ID of the card"), required("imageData", kindString, "Base64-encoded image data"), required("name", kindString, "Image file name"),
		}},
		{Name: "list_boards", Description: "List boards visible to the authenticated member, filtered by allowed workspaces", Fields: nil},
		{Name: "set_active_board", Description: "Validate and persist the active board", Fields: []fieldDefinition{required("boardId", kindString, "Board ID")}},
		{Name: "list_workspaces", Description: "List visible workspaces, filtered by allowed_workspaces", Fields: nil},
		{Name: "create_board", Description: "Create a board in a workspace", Fields: []fieldDefinition{
			required("name", kindString, "Board name"), optional("workspaceId", kindString, "Workspace ID; uses the configured active workspace when omitted"),
			optional("description", kindString, "Board description"), defaulted("defaultLists", kindBoolean, "Create Trello's default lists", true),
		}},
		{Name: "set_active_workspace", Description: "Validate and persist the active workspace", Fields: []fieldDefinition{required("workspaceId", kindString, "Workspace ID")}},
		{Name: "list_boards_in_workspace", Description: "List boards in an allowed workspace", Fields: []fieldDefinition{required("workspaceId", kindString, "Workspace ID")}},
		{Name: "get_active_board_info", Description: "Get details about the configured active board", Fields: nil},
		{Name: "get_card", Description: "Get LLM-optimized card details directly by full ID, short link, or Trello card URL", Fields: []fieldDefinition{
			required("cardId", kindString, "Full card ID, 8-character short link, or trello.com/c/... card URL"),
			defaultedEnum("detailLevel", "compact returns only decision-useful fields; full returns raw Trello fields", "compact", "compact", "full"),
			defaulted("commentsLimit", kindInteger, "Latest comments to include; 0 excludes comments, maximum 100", 10),
			defaultedEnum("format", "markdown is optimized for LLM context; json returns structured data", "markdown", "json", "markdown"),
			defaultedEnum("delivery", "inline returns content directly; file writes a private cache file; auto writes only responses larger than 16 KiB", "auto", "inline", "file", "auto"),
			optional("includeMarkdown", kindBoolean, "Deprecated alias for format=markdown"),
		}, OutputSchema: cardOutputSchema()},
		{Name: "add_comment", Description: "Add a comment to a card", Fields: []fieldDefinition{
			required("cardId", kindString, "ID of the card"), required("text", kindString, "Comment text"),
		}},
		{Name: "update_comment", Description: "Update a Trello comment action", Fields: []fieldDefinition{
			required("commentId", kindString, "Comment action ID"), required("text", kindString, "New comment text"),
		}},
		{Name: "delete_comment", Description: "Delete a Trello comment action", Fields: []fieldDefinition{required("commentId", kindString, "Comment action ID")}},
		{Name: "get_card_comments", Description: "Get compact LLM-oriented comments on a card", Fields: []fieldDefinition{
			required("cardId", kindString, "ID or short link of the card"), defaulted("limit", kindInteger, "Maximum latest comments to return", 10),
		}},
		{Name: "create_checklist", Description: "Create a checklist on a card", Fields: []fieldDefinition{
			required("cardId", kindString, "ID of the card"), required("name", kindString, "Checklist name"), optional("position", kindString, "top, bottom, or a numeric position"),
		}},
		{Name: "get_checklist_items", Description: "Get checklist items by checklist name", Fields: []fieldDefinition{
			required("name", kindString, "Checklist name"), optional("cardId", kindString, "Card ID to scope the lookup"), boardField(),
		}},
		{Name: "add_checklist_item", Description: "Add an item to a checklist", Fields: []fieldDefinition{
			required("checklistId", kindString, "Checklist ID"), required("name", kindString, "Item text"), optional("checked", kindBoolean, "Initial completion state"), optional("position", kindString, "top, bottom, or a numeric position"), optional("due", kindString, "ISO 8601 due date"), optional("dueReminder", kindInteger, "Reminder offset in minutes"), optional("memberId", kindString, "Assigned member ID"),
		}},
		{Name: "find_checklist_items_by_description", Description: "Find checklist items containing text on a card or board", Fields: []fieldDefinition{
			required("description", kindString, "Case-insensitive text to find"), optional("cardId", kindString, "Card ID to scope the lookup"), boardField(),
		}},
		{Name: "get_acceptance_criteria", Description: "Get items from the Acceptance Criteria checklist", Fields: []fieldDefinition{
			optional("cardId", kindString, "Card ID to scope the lookup"), boardField(),
		}},
		{Name: "get_checklist_by_name", Description: "Get a checklist by name", Fields: []fieldDefinition{
			required("name", kindString, "Checklist name"), optional("cardId", kindString, "Card ID to scope the lookup"), boardField(),
		}},
		{Name: "update_checklist_item", Description: "Update a checklist item", Fields: []fieldDefinition{
			required("cardId", kindString, "ID of the card"), required("checkItemId", kindString, "Checklist item ID"),
			optional("name", kindString, "New item text"), enumerated("state", "Completion state", "complete", "incomplete"),
			optional("position", kindString, "top, bottom, or a numeric position"), optional("due", kindString, "ISO 8601 due date, or empty string to clear"),
			optional("dueReminder", kindInteger, "Reminder offset in minutes"), optional("memberId", kindString, "Assigned member ID, or empty string to clear"),
		}},
		{Name: "delete_checklist_item", Description: "Delete an item from a checklist", Fields: []fieldDefinition{
			required("checklistId", kindString, "Checklist ID"), required("checkItemId", kindString, "Checklist item ID"),
		}},
		{Name: "get_board_members", Description: "Get members of a board", Fields: []fieldDefinition{boardField()}},
		{Name: "assign_member_to_card", Description: "Assign a member to a card", Fields: []fieldDefinition{
			required("cardId", kindString, "Card ID"), required("memberId", kindString, "Member ID"),
		}},
		{Name: "remove_member_from_card", Description: "Remove a member from a card", Fields: []fieldDefinition{
			required("cardId", kindString, "Card ID"), required("memberId", kindString, "Member ID"),
		}},
		{Name: "get_board_labels", Description: "Get labels on a board", Fields: []fieldDefinition{boardField()}},
		{Name: "create_label", Description: "Create a board label", Fields: []fieldDefinition{
			boardField(), required("name", kindString, "Label name"), requiredEnum("color", "Trello label color", "yellow", "purple", "blue", "red", "green", "orange", "black", "sky", "pink", "lime"),
		}},
		{Name: "update_label", Description: "Update a board label", Fields: []fieldDefinition{
			required("labelId", kindString, "Label ID"), optional("name", kindString, "New label name"), enumerated("color", "New Trello label color", "yellow", "purple", "blue", "red", "green", "orange", "black", "sky", "pink", "lime", "null"),
		}},
		{Name: "delete_label", Description: "Delete a board label", Fields: []fieldDefinition{required("labelId", kindString, "Label ID")}},
		{Name: "copy_card", Description: "Copy a card into a list", Fields: []fieldDefinition{
			required("sourceCardId", kindString, "Source card ID"), required("listId", kindString, "Destination list ID"), optional("name", kindString, "Name for the copied card"), optional("position", kindString, "top, bottom, or a numeric position"), optional("keepFromSource", kindStringArray, "Fields to copy, such as attachments,checklists,comments,due,labels,members,stickers"),
		}},
		{Name: "copy_checklist", Description: "Copy a checklist to a card", Fields: []fieldDefinition{
			required("sourceChecklistId", kindString, "Source checklist ID"), required("cardId", kindString, "Destination card ID"), optional("name", kindString, "Name for the copied checklist"), optional("position", kindString, "top, bottom, or a numeric position"),
		}},
		{Name: "add_cards_to_list", Description: "Create multiple cards sequentially in a list", Fields: []fieldDefinition{
			required("listId", kindString, "Destination list ID"), required("cards", kindObjectArray, "Cards containing at least a name and optionally description, due, start, and position"),
		}},
		{Name: "get_board_custom_fields", Description: "Get custom field definitions for a board", Fields: []fieldDefinition{boardField()}},
		{Name: "update_card_custom_field", Description: "Set or clear a custom field value on a card", Fields: []fieldDefinition{
			required("cardId", kindString, "Card ID"), required("customFieldId", kindString, "Custom field definition ID"),
			requiredEnum("type", "Custom field type", "text", "number", "checkbox", "date", "list", "clear"), optional("value", kindString, "Value; list fields use an option ID"),
		}},
		{Name: "get_card_history", Description: "Get action history for a card", Fields: []fieldDefinition{
			required("cardId", kindString, "Card ID"), defaulted("limit", kindInteger, "Maximum actions to return", 100),
		}},
		{Name: "download_attachment", Description: "Download a card attachment to a local file", Fields: []fieldDefinition{
			required("cardId", kindString, "Card ID"), required("attachmentId", kindString, "Attachment ID"), required("destinationPath", kindString, "Local destination path"),
		}},
		{Name: "get_health", Description: "Check local configuration and Trello API reachability", Fields: nil},
		{Name: "get_health_detailed", Description: "Get detailed server, config, and Trello health information", Fields: nil},
		{Name: "get_health_metadata", Description: "Get server metadata without exposing credentials", Fields: nil},
		{Name: "get_health_performance", Description: "Measure a lightweight Trello API request", Fields: nil},
		{Name: "perform_system_repair", Description: "Reload and validate persistent configuration; no remote data is modified", Fields: nil},
	}
}
