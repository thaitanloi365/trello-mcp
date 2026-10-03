package mcpserver

func required(name string, kind fieldKind, description string) fieldDefinition {
	return fieldDefinition{Name: name, Kind: kind, Description: description, Required: true}
}

func optional(name string, kind fieldKind, description string) fieldDefinition {
	return fieldDefinition{Name: name, Kind: kind, Description: description}
}

func defaultedEnum(name, description, value string, values ...string) fieldDefinition {
	return fieldDefinition{Name: name, Kind: kindString, Description: description, Enum: values, Default: value}
}

func requiredEnum(name, description string, values ...string) fieldDefinition {
	return fieldDefinition{Name: name, Kind: kindString, Description: description, Required: true, Enum: values}
}

func enumerated(name, description string, values ...string) fieldDefinition {
	return fieldDefinition{Name: name, Kind: kindString, Description: description, Enum: values}
}

func defaulted(name string, kind fieldKind, description string, value any) fieldDefinition {
	return fieldDefinition{Name: name, Kind: kind, Description: description, Default: value}
}

// replyStyle is the house style for every comment posted to a card.
const replyStyle = "Write plain English: main point first, short '- ' bullets, no jargon. Link every attachment or URL as [name](url); never paste a bare long URL."

// approval keeps the preview rule on every tool whose text other people see.
const approval = " Show the user the exact text and wait for approval first."

const (
	cardRef  = "Card ID, short link, or Trello card URL"
	listRef  = "List name or ID; names resolve on boardId or the active board"
	position = "top, bottom, or a number"
)

var labelColors = []string{"yellow", "purple", "blue", "red", "green", "orange", "black", "sky", "pink", "lime"}

func boardField() fieldDefinition {
	return optional("boardId", kindString, "Board ID; defaults to the active board")
}

func toolDefinitions() []toolDefinition {
	return []toolDefinition{
		{Name: "get_card", ReadOnly: true, Description: "Read a card as Markdown: details, custom fields, checklists with IDs, attachment links, and recent comments", Fields: []fieldDefinition{
			required("cardId", kindString, cardRef),
			defaulted("commentsLimit", kindInteger, "Latest comments to include, 0 to 100", 10),
			defaultedEnum("delivery", "auto writes output over 16 KiB to a private file and returns its path", "auto", "auto", "inline", "file"),
		}},
		{Name: "list_cards", ReadOnly: true, Description: "List open cards in a list, on a board, or, with neither, assigned to me", Fields: []fieldDefinition{
			optional("list", kindString, listRef), optional("boardId", kindString, "Board ID to list all its open cards"),
			defaulted("limit", kindInteger, "Maximum cards", 100),
		}},
		{Name: "get_activity", ReadOnly: true, Description: "Recent actions on a card, or on a board when cardId is omitted", Fields: []fieldDefinition{
			optional("cardId", kindString, cardRef), boardField(), defaulted("limit", kindInteger, "Maximum actions", 50),
		}},
		{Name: "get_board", ReadOnly: true, Description: "Get a board's lists, labels, members, and custom fields with their IDs", Fields: []fieldDefinition{boardField()}},
		{Name: "list_boards", ReadOnly: true, Description: "List open boards, optionally in one workspace", Fields: []fieldDefinition{
			optional("workspaceId", kindString, "Workspace ID"),
		}},
		{Name: "list_workspaces", ReadOnly: true, Description: "List workspaces", Fields: nil},
		{Name: "find_checklist_items", ReadOnly: true, Description: "Find checklist items by text, checklist name, or both, on a card or board; results are grouped by checklist", Fields: []fieldDefinition{
			optional("query", kindString, "Case-insensitive text in the item"),
			optional("checklist", kindString, "Checklist name, such as Acceptance Criteria"),
			optional("cardId", kindString, cardRef+"; omit to search the board"), boardField(),
		}},
		{Name: "get_health", ReadOnly: true, Description: "Check configuration and Trello API reachability", Fields: nil},
		{Name: "download_attachment", Description: "Download a card attachment to a local file", Fields: []fieldDefinition{
			required("cardId", kindString, cardRef), required("attachmentId", kindString, "Attachment ID"),
			required("destinationPath", kindString, "Local destination path"),
		}},

		{Name: "create_cards", Description: "Create one or more cards in a list" + approval, Fields: []fieldDefinition{
			required("list", kindString, listRef), boardField(),
			{Name: "cards", Kind: kindObjectArray, Required: true, Description: "Cards to create, in order", Items: []fieldDefinition{
				required("name", kindString, "Card title"), optional("description", kindString, "Markdown description"),
				optional("due", kindString, "ISO 8601 due date"), optional("start", kindString, "ISO 8601 start date"),
				optional("labels", kindStringArray, "Label names or IDs"), optional("members", kindStringArray, "Member names, usernames, IDs, or me"),
				optional("position", kindString, position),
			}},
		}},
		{Name: "update_card", Description: "Change a card's fields, dates, list, archive or watch state, labels, or members; omitted fields stay unchanged" + approval, Fields: []fieldDefinition{
			required("cardId", kindString, cardRef),
			optional("name", kindString, "New title"), optional("description", kindString, "New Markdown description"),
			optional("due", kindString, "ISO 8601 due date; empty string clears"), optional("start", kindString, "ISO 8601 start date; empty string clears"),
			optional("dueComplete", kindBoolean, "Mark the due date done"), optional("dueReminder", kindInteger, "Reminder minutes before due; -1 for none"),
			optional("list", kindString, "Move to this list, by name or ID on the card's board"), optional("position", kindString, position),
			optional("closed", kindBoolean, "Archive (true) or restore (false)"), optional("subscribed", kindBoolean, "Watch the card"),
			optional("addLabels", kindStringArray, "Label names or IDs to add"), optional("removeLabels", kindStringArray, "Label names or IDs to remove"),
			optional("addMembers", kindStringArray, "Member names, usernames, IDs, or me to add"), optional("removeMembers", kindStringArray, "Members to remove"),
		}},
		{Name: "set_custom_field", Description: "Set or clear a custom field on a card", Fields: []fieldDefinition{
			required("cardId", kindString, cardRef), required("field", kindString, "Custom field name or ID"),
			optional("value", kindString, "Text, number, true or false, ISO date, or list option"),
			optional("clear", kindBoolean, "Clear the field instead of setting a value"),
		}},
		{Name: "copy_card", Description: "Copy a card into a list", Fields: []fieldDefinition{
			required("sourceCardId", kindString, cardRef), required("list", kindString, listRef), boardField(),
			optional("name", kindString, "Name for the copy"), optional("position", kindString, position),
			optional("keepFromSource", kindStringArray, "Parts to copy: attachments, checklists, comments, due, labels, members, stickers"),
		}},
		{Name: "create_list", Description: "Create a list on a board", Fields: []fieldDefinition{
			boardField(), required("name", kindString, "List name"), optional("position", kindString, position),
		}},
		{Name: "update_list", Description: "Rename, move, archive, or watch a list", Fields: []fieldDefinition{
			required("list", kindString, listRef), boardField(), optional("name", kindString, "New name"),
			optional("position", kindString, position), optional("closed", kindBoolean, "Archive (true) or restore (false)"),
			optional("subscribed", kindBoolean, "Watch the list"),
		}},
		{Name: "create_board", Description: "Create a board in a workspace", Fields: []fieldDefinition{
			required("name", kindString, "Board name"), optional("workspaceId", kindString, "Workspace ID; defaults to the active workspace"),
			optional("description", kindString, "Board description"), defaulted("defaultLists", kindBoolean, "Create Trello's default lists", true),
		}},
		{Name: "set_active", Description: "Save the default board, workspace, or both for later calls", Fields: []fieldDefinition{
			optional("boardId", kindString, "Board ID"), optional("workspaceId", kindString, "Workspace ID"),
		}},
		{Name: "add_comment", Description: "Post a comment on a card. " + replyStyle + approval, Fields: []fieldDefinition{
			required("cardId", kindString, cardRef), required("text", kindString, "Comment text"),
		}},
		{Name: "update_comment", Description: "Edit a comment. " + replyStyle + approval, Fields: []fieldDefinition{
			required("commentId", kindString, "Comment ID"), required("text", kindString, "New comment text"),
		}},
		{Name: "delete_comment", Description: "Delete a comment", Fields: []fieldDefinition{required("commentId", kindString, "Comment ID")}},
		{Name: "create_checklist", Description: "Create a checklist on a card with its items" + approval, Fields: []fieldDefinition{
			required("cardId", kindString, cardRef), required("name", kindString, "Checklist name"),
			optional("items", kindStringArray, "Item texts, in order"), optional("position", kindString, position),
		}},
		{Name: "add_checklist_item", Description: "Add an item to a checklist" + approval, Fields: []fieldDefinition{
			required("checklistId", kindString, "Checklist ID"), required("name", kindString, "Item text"),
			optional("checked", kindBoolean, "Start as done"), optional("position", kindString, position),
			optional("due", kindString, "ISO 8601 due date"), optional("dueReminder", kindInteger, "Reminder minutes before due; -1 for none"),
			optional("member", kindString, "Assignee name, username, ID, or me"),
		}},
		{Name: "update_checklist_item", Description: "Change a checklist item's text, done state, position, due date, or assignee" + approval, Fields: []fieldDefinition{
			required("cardId", kindString, cardRef), required("checkItemId", kindString, "Checklist item ID"),
			optional("name", kindString, "New text"), optional("checked", kindBoolean, "Done (true) or not done (false)"),
			optional("position", kindString, position), optional("due", kindString, "ISO 8601 due date; empty string clears"),
			optional("dueReminder", kindInteger, "Reminder minutes before due; -1 for none"),
			optional("member", kindString, "Assignee name, username, ID, or me; empty string clears"),
		}},
		{Name: "delete_checklist_item", Description: "Delete a checklist item", Fields: []fieldDefinition{
			required("checklistId", kindString, "Checklist ID"), required("checkItemId", kindString, "Checklist item ID"),
		}},
		{Name: "create_label", Description: "Create a board label", Fields: []fieldDefinition{
			boardField(), required("name", kindString, "Label name"), requiredEnum("color", "Label color", labelColors...),
		}},
		{Name: "update_label", Description: "Rename or recolor a board label", Fields: []fieldDefinition{
			required("labelId", kindString, "Label ID"), optional("name", kindString, "New name"),
			enumerated("color", "New color; null removes it", append(labelColors, "null")...),
		}},
		{Name: "delete_label", Description: "Delete a board label", Fields: []fieldDefinition{required("labelId", kindString, "Label ID")}},
		{Name: "add_attachment", Description: "Attach a link, a local file, or base64 data to a card; give exactly one of url, filePath, or data", Fields: []fieldDefinition{
			required("cardId", kindString, cardRef), optional("url", kindString, "Link or image URL"),
			optional("filePath", kindString, "Local file path, subject to max_upload_bytes"), optional("data", kindString, "Base64 file content"),
			optional("name", kindString, "Attachment name; required with data"), optional("setCover", kindBoolean, "Use the url image as the card cover"),
		}},
	}
}
