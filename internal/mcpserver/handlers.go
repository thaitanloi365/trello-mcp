package mcpserver

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"net/url"
	"strings"
)

func (s *Server) handle(ctx context.Context, name string, args map[string]any) (any, error) {
	switch name {
	case "get_card":
		return s.getCard(ctx, text(args, "cardId"), getCardOptions{
			commentsLimit: integer(args, "commentsLimit", 10),
			delivery:      textDefault(args, "delivery", "auto"),
		})
	case "list_cards":
		return s.listCards(ctx, args)
	case "get_activity":
		return s.activity(ctx, args)
	case "get_board":
		boardID, err := s.board(ctx, args)
		if err != nil {
			return nil, err
		}
		index, err := s.boardIndex(ctx, boardID)
		if err != nil {
			return nil, err
		}
		return boardSummary(index), nil
	case "list_boards":
		return s.listBoards(ctx, text(args, "workspaceId"))
	case "list_workspaces":
		result, err := s.client.Do(ctx, http.MethodGet, "/members/me/organizations", map[string]any{"fields": "id,displayName"}, nil)
		if err != nil {
			return nil, err
		}
		return s.filterObjectsByWorkspace(result, "id"), nil
	case "find_checklist_items":
		return s.findChecklistItems(ctx, args)
	case "get_health":
		return s.health(ctx)
	case "download_attachment":
		cardID, err := s.allowedCard(ctx, args, "cardId")
		if err != nil {
			return nil, err
		}
		return s.client.DownloadAttachment(ctx, cardID, text(args, "attachmentId"), text(args, "destinationPath"))

	case "create_cards":
		return s.createCards(ctx, args)
	case "update_card":
		return s.updateCard(ctx, args)
	case "set_custom_field":
		return s.setCustomField(ctx, args)
	case "copy_card":
		sourceRef, err := s.allowedCard(ctx, args, "sourceCardId")
		if err != nil {
			return nil, err
		}
		// idCardSource must be the full ID, and a list name means the source card's board.
		source, err := s.client.Do(ctx, http.MethodGet, "/cards/"+escape(sourceRef), map[string]any{"fields": "id,idBoard"}, nil)
		if err != nil {
			return nil, err
		}
		listID, _, err := s.resolveList(ctx, args, s.boardIDRefs(nestedString(source, "idBoard")))
		if err != nil {
			return nil, err
		}
		params := mapArgs(args, map[string]string{"name": "name", "position": "pos"})
		params["idCardSource"], params["idList"] = nestedString(source, "id"), listID
		if values, ok := args["keepFromSource"].([]any); ok {
			params["keepFromSource"] = joinValues(values)
		}
		return s.write(ctx, http.MethodPost, "/cards", params, nil)
	case "create_list":
		boardID, err := s.board(ctx, args)
		if err != nil {
			return nil, err
		}
		params := map[string]any{"idBoard": boardID, "name": text(args, "name")}
		copyArg(params, "pos", args, "position")
		return s.write(ctx, http.MethodPost, "/lists", params, nil)
	case "update_list":
		listID, _, err := s.resolveList(ctx, args, nil)
		if err != nil {
			return nil, err
		}
		return s.write(ctx, http.MethodPut, "/lists/"+escape(listID), mapArgs(args, map[string]string{
			"name": "name", "position": "pos", "closed": "closed", "subscribed": "subscribed",
		}), nil)
	case "create_board":
		workspaceID, err := s.workspaceID(args)
		if err != nil {
			return nil, err
		}
		params := map[string]any{"name": text(args, "name"), "idOrganization": workspaceID, "defaultLists": boolean(args, "defaultLists", true)}
		copyArg(params, "desc", args, "description")
		return s.write(ctx, http.MethodPost, "/boards", params, nil)
	case "set_active":
		return s.setActive(ctx, text(args, "boardId"), text(args, "workspaceId"))
	case "add_comment":
		cardID, err := s.allowedCard(ctx, args, "cardId")
		if err != nil {
			return nil, err
		}
		return s.write(ctx, http.MethodPost, "/cards/"+escape(cardID)+"/actions/comments", map[string]any{"text": linkAttachments(text(args, "text"))}, nil)
	case "update_comment", "delete_comment":
		commentID := text(args, "commentId")
		if err := s.ensureActionAllowed(ctx, commentID); err != nil {
			return nil, err
		}
		method, params := http.MethodDelete, map[string]any(nil)
		if name == "update_comment" {
			method, params = http.MethodPut, map[string]any{"text": linkAttachments(text(args, "text"))}
		}
		return s.write(ctx, method, "/actions/"+escape(commentID)+"/comments", params, nil)
	case "create_checklist":
		return s.createChecklist(ctx, args)
	case "add_checklist_item":
		checklistID := text(args, "checklistId")
		if err := s.ensureChecklistAllowed(ctx, checklistID); err != nil {
			return nil, err
		}
		params := mapArgs(args, map[string]string{"name": "name", "checked": "checked", "position": "pos", "due": "due", "dueReminder": "dueReminder"})
		if member := text(args, "member"); member != "" {
			memberID, err := s.checklistRefs(checklistID).id(ctx, "members", member)
			if err != nil {
				return nil, err
			}
			params["idMember"] = memberID
		}
		return s.write(ctx, http.MethodPost, "/checklists/"+escape(checklistID)+"/checkItems", params, nil)
	case "update_checklist_item":
		cardID, err := s.allowedCard(ctx, args, "cardId")
		if err != nil {
			return nil, err
		}
		params := mapArgs(args, map[string]string{"name": "name", "position": "pos", "due": "due", "dueReminder": "dueReminder"})
		if checked, ok := args["checked"].(bool); ok {
			params["state"] = "incomplete"
			if checked {
				params["state"] = "complete"
			}
		}
		if _, exists := args["member"]; exists {
			params["idMember"] = ""
			if member := text(args, "member"); member != "" {
				if params["idMember"], err = s.cardRefs(cardID).id(ctx, "members", member); err != nil {
					return nil, err
				}
			}
		}
		return s.write(ctx, http.MethodPut, "/cards/"+escape(cardID)+"/checkItem/"+escape(text(args, "checkItemId")), params, nil)
	case "delete_checklist_item":
		checklistID := text(args, "checklistId")
		if err := s.ensureChecklistAllowed(ctx, checklistID); err != nil {
			return nil, err
		}
		return s.write(ctx, http.MethodDelete, "/checklists/"+escape(checklistID)+"/checkItems/"+escape(text(args, "checkItemId")), nil, nil)
	case "create_label":
		boardID, err := s.board(ctx, args)
		if err != nil {
			return nil, err
		}
		return s.write(ctx, http.MethodPost, "/labels", map[string]any{"idBoard": boardID, "name": text(args, "name"), "color": text(args, "color")}, nil)
	case "update_label", "delete_label":
		labelID := text(args, "labelId")
		if err := s.ensureLabelAllowed(ctx, labelID); err != nil {
			return nil, err
		}
		if name == "delete_label" {
			return s.write(ctx, http.MethodDelete, "/labels/"+escape(labelID), nil, nil)
		}
		return s.write(ctx, http.MethodPut, "/labels/"+escape(labelID), mapArgs(args, map[string]string{"name": "name", "color": "color"}), nil)
	case "add_attachment":
		return s.addAttachment(ctx, args)
	default:
		return nil, fmt.Errorf("tool %q is not implemented", name)
	}
}

// allowedCard normalizes a card reference argument and applies the workspace
// allow-list.
func (s *Server) allowedCard(ctx context.Context, args map[string]any, key string) (string, error) {
	cardID, err := normalizeCardRef(text(args, key))
	if err != nil {
		return "", err
	}
	return cardID, s.ensureCardAllowed(ctx, cardID)
}

func joinValues(values []any) string {
	parts := make([]string, 0, len(values))
	for _, value := range values {
		parts = append(parts, fmt.Sprint(value))
	}
	return strings.Join(parts, ",")
}

func (s *Server) board(ctx context.Context, args map[string]any) (string, error) {
	boardID, err := s.client.BoardID(text(args, "boardId"))
	if err != nil {
		return "", err
	}
	if err := s.client.EnsureBoardAllowed(ctx, boardID); err != nil {
		return "", err
	}
	return boardID, nil
}

func (s *Server) workspaceID(args map[string]any) (string, error) {
	workspaceID := text(args, "workspaceId")
	if workspaceID == "" {
		cfg, err := s.client.Manager().Effective()
		if err != nil {
			return "", err
		}
		workspaceID = cfg.WorkspaceID
	}
	if err := s.client.RequireWorkspaceAllowed(workspaceID); err != nil {
		return "", err
	}
	return workspaceID, nil
}

// write calls a mutating endpoint and returns a short ack instead of the full
// Trello object, which can run to several KiB; get_card has the full view.
func (s *Server) write(ctx context.Context, method, path string, params map[string]any, body any) (any, error) {
	return acked(s.client.Do(ctx, method, path, params, body))
}

func acked(value any, err error) (any, error) {
	if err != nil {
		return nil, err
	}
	object, ok := value.(map[string]any)
	if !ok {
		return value, nil
	}
	result := make(map[string]any)
	copyPresent(result, object, "id", "name", "shortUrl", "idList", "due", "start", "dueComplete", "dueReminder", "closed", "state", "color", "value", "date")
	if _, ok := result["shortUrl"]; !ok {
		copyPresent(result, object, "url")
	}
	if items, ok := object["checkItems"]; ok {
		result["checkItems"] = compactObjects(items, "id", "name", "state")
	}
	if len(result) == 0 {
		return map[string]any{"ok": true}, nil
	}
	return result, nil
}

func (s *Server) workspaceRestrictionsEnabled() (bool, error) {
	cfg, err := s.client.Manager().Effective()
	if err != nil {
		return false, err
	}
	return len(cfg.AllowedWorkspaceIDs) > 0, nil
}

func (s *Server) ensureCardAllowed(ctx context.Context, cardID string) error {
	enabled, err := s.workspaceRestrictionsEnabled()
	if err != nil || !enabled {
		return err
	}
	card, err := s.client.Do(ctx, http.MethodGet, "/cards/"+escape(cardID), map[string]any{"fields": "id,idBoard"}, nil)
	if err != nil {
		return err
	}
	boardID := nestedString(card, "idBoard")
	if boardID == "" {
		return errors.New("Trello card response did not include idBoard")
	}
	return s.client.EnsureBoardAllowed(ctx, boardID)
}

func (s *Server) ensureListAllowed(ctx context.Context, listID string) error {
	enabled, err := s.workspaceRestrictionsEnabled()
	if err != nil || !enabled {
		return err
	}
	list, err := s.client.Do(ctx, http.MethodGet, "/lists/"+escape(listID), map[string]any{"fields": "id,idBoard"}, nil)
	if err != nil {
		return err
	}
	boardID := nestedString(list, "idBoard")
	if boardID == "" {
		return errors.New("Trello list response did not include idBoard")
	}
	return s.client.EnsureBoardAllowed(ctx, boardID)
}

func (s *Server) ensureChecklistAllowed(ctx context.Context, checklistID string) error {
	enabled, err := s.workspaceRestrictionsEnabled()
	if err != nil || !enabled {
		return err
	}
	checklist, err := s.client.Do(ctx, http.MethodGet, "/checklists/"+escape(checklistID), map[string]any{"fields": "id,idBoard"}, nil)
	if err != nil {
		return err
	}
	boardID := nestedString(checklist, "idBoard")
	if boardID == "" {
		return errors.New("Trello checklist response did not include idBoard")
	}
	return s.client.EnsureBoardAllowed(ctx, boardID)
}

func (s *Server) ensureActionAllowed(ctx context.Context, actionID string) error {
	enabled, err := s.workspaceRestrictionsEnabled()
	if err != nil || !enabled {
		return err
	}
	action, err := s.client.Do(ctx, http.MethodGet, "/actions/"+escape(actionID), map[string]any{"fields": "data"}, nil)
	if err != nil {
		return err
	}
	boardID := nestedString(action, "data", "board", "id")
	if boardID == "" {
		return errors.New("Trello action response did not include a board ID")
	}
	return s.client.EnsureBoardAllowed(ctx, boardID)
}

func (s *Server) ensureLabelAllowed(ctx context.Context, labelID string) error {
	enabled, err := s.workspaceRestrictionsEnabled()
	if err != nil || !enabled {
		return err
	}
	label, err := s.client.Do(ctx, http.MethodGet, "/labels/"+escape(labelID), map[string]any{"fields": "id,idBoard"}, nil)
	if err != nil {
		return err
	}
	boardID := nestedString(label, "idBoard")
	if boardID == "" {
		return errors.New("Trello label response did not include idBoard")
	}
	return s.client.EnsureBoardAllowed(ctx, boardID)
}

func text(args map[string]any, name string) string {
	if args == nil {
		return ""
	}
	value, _ := args[name].(string)
	return strings.TrimSpace(value)
}

func textDefault(args map[string]any, name, fallback string) string {
	if value := text(args, name); value != "" {
		return value
	}
	return fallback
}

func integer(args map[string]any, name string, fallback int) int {
	if value, ok := args[name].(float64); ok {
		return int(value)
	}
	return fallback
}

func boolean(args map[string]any, name string, fallback bool) bool {
	if value, ok := args[name].(bool); ok {
		return value
	}
	return fallback
}

func mapArgs(args map[string]any, names map[string]string) map[string]any {
	result := make(map[string]any)
	for input, output := range names {
		copyArg(result, output, args, input)
	}
	return result
}

func copyArg(target map[string]any, output string, args map[string]any, input string) {
	if value, exists := args[input]; exists && value != nil {
		target[output] = value
	}
}

func escape(value string) string { return url.PathEscape(value) }

func nestedString(value any, path ...string) string {
	result, _ := nestedValue(value, path...).(string)
	return result
}
