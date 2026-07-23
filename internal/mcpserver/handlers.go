package mcpserver

import (
	"context"
	"encoding/base64"
	"errors"
	"fmt"
	"net/http"
	"net/url"
	"strings"
	"time"
)

func (s *Server) handle(ctx context.Context, name string, args map[string]any) (any, error) {
	switch name {
	case "get_cards_by_list_id":
		listID := text(args, "listId")
		if err := s.ensureListAllowed(ctx, listID); err != nil {
			return nil, err
		}
		return s.client.Do(ctx, http.MethodGet, "/lists/"+escape(listID)+"/cards", map[string]any{
			"fields": "id,name,desc,due,start,dueComplete,closed,pos,url,idBoard,idList,idMembers,idLabels", "limit": integer(args, "limit", 100),
		}, nil)
	case "get_lists":
		boardID, err := s.board(ctx, args)
		if err != nil {
			return nil, err
		}
		return s.client.Do(ctx, http.MethodGet, "/boards/"+escape(boardID)+"/lists", map[string]any{"filter": "open", "fields": "id,name,closed,pos,idBoard,subscribed"}, nil)
	case "get_recent_activity":
		boardID, err := s.board(ctx, args)
		if err != nil {
			return nil, err
		}
		return s.client.Do(ctx, http.MethodGet, "/boards/"+escape(boardID)+"/actions", map[string]any{"filter": "all", "limit": integer(args, "limit", 50)}, nil)
	case "add_card_to_list":
		listID := text(args, "listId")
		if err := s.ensureListAllowed(ctx, listID); err != nil {
			return nil, err
		}
		return s.client.Do(ctx, http.MethodPost, "/cards", mapArgs(args, map[string]string{
			"listId": "idList", "name": "name", "description": "desc", "due": "due", "start": "start", "position": "pos",
		}), nil)
	case "update_card_details":
		cardID := text(args, "cardId")
		if err := s.ensureCardAllowed(ctx, cardID); err != nil {
			return nil, err
		}
		return s.client.Do(ctx, http.MethodPut, "/cards/"+escape(cardID), mapArgs(args, map[string]string{
			"name": "name", "description": "desc", "due": "due", "start": "start", "dueComplete": "dueComplete", "closed": "closed", "position": "pos",
		}), nil)
	case "archive_card":
		return s.updateCardFlag(ctx, text(args, "cardId"), "closed", true)
	case "watch_card":
		return s.updateCardFlag(ctx, text(args, "cardId"), "subscribed", boolean(args, "subscribed", true))
	case "watch_list":
		listID := text(args, "listId")
		if err := s.ensureListAllowed(ctx, listID); err != nil {
			return nil, err
		}
		return s.client.Do(ctx, http.MethodPut, "/lists/"+escape(listID), map[string]any{"subscribed": boolean(args, "subscribed", true)}, nil)
	case "move_card":
		cardID, listID := text(args, "cardId"), text(args, "listId")
		if err := s.ensureCardAllowed(ctx, cardID); err != nil {
			return nil, err
		}
		if err := s.ensureListAllowed(ctx, listID); err != nil {
			return nil, err
		}
		params := map[string]any{"idList": listID}
		copyArg(params, "pos", args, "position")
		return s.client.Do(ctx, http.MethodPut, "/cards/"+escape(cardID), params, nil)
	case "add_list_to_board":
		boardID, err := s.board(ctx, args)
		if err != nil {
			return nil, err
		}
		params := map[string]any{"idBoard": boardID, "name": text(args, "name")}
		copyArg(params, "pos", args, "position")
		return s.client.Do(ctx, http.MethodPost, "/lists", params, nil)
	case "archive_list":
		listID := text(args, "listId")
		if err := s.ensureListAllowed(ctx, listID); err != nil {
			return nil, err
		}
		return s.client.Do(ctx, http.MethodPut, "/lists/"+escape(listID), map[string]any{"closed": true}, nil)
	case "update_list":
		listID := text(args, "listId")
		if err := s.ensureListAllowed(ctx, listID); err != nil {
			return nil, err
		}
		return s.client.Do(ctx, http.MethodPut, "/lists/"+escape(listID), mapArgs(args, map[string]string{"name": "name", "closed": "closed"}), nil)
	case "update_list_position":
		listID := text(args, "listId")
		if err := s.ensureListAllowed(ctx, listID); err != nil {
			return nil, err
		}
		return s.client.Do(ctx, http.MethodPut, "/lists/"+escape(listID), map[string]any{"pos": text(args, "position")}, nil)
	case "get_my_cards":
		result, err := s.client.Do(ctx, http.MethodGet, "/members/me/cards", map[string]any{
			"filter": "visible", "fields": "id,name,desc,due,start,closed,url,idBoard,idList", "limit": integer(args, "limit", 100),
		}, nil)
		if err != nil {
			return nil, err
		}
		return s.filterByWorkspace(ctx, result)
	case "attach_image_to_card":
		cardID := text(args, "cardId")
		if err := s.ensureCardAllowed(ctx, cardID); err != nil {
			return nil, err
		}
		params := map[string]any{"url": text(args, "imageUrl"), "name": textDefault(args, "name", "Image Attachment")}
		copyArg(params, "setCover", args, "setCover")
		return s.client.Do(ctx, http.MethodPost, "/cards/"+escape(cardID)+"/attachments", params, nil)
	case "attach_file_to_card":
		cardID := text(args, "cardId")
		if err := s.ensureCardAllowed(ctx, cardID); err != nil {
			return nil, err
		}
		return s.client.UploadFile(ctx, cardID, text(args, "filePath"), text(args, "name"))
	case "attach_data_to_card", "attach_image_data_to_card":
		cardID := text(args, "cardId")
		if err := s.ensureCardAllowed(ctx, cardID); err != nil {
			return nil, err
		}
		key := "data"
		if name == "attach_image_data_to_card" {
			key = "imageData"
		}
		decoded, err := base64.StdEncoding.DecodeString(text(args, key))
		if err != nil {
			return nil, fmt.Errorf("decode %s: %w", key, err)
		}
		return s.client.UploadData(ctx, cardID, text(args, "name"), decoded)
	case "list_boards":
		result, err := s.client.Do(ctx, http.MethodGet, "/members/me/boards", map[string]any{"filter": "open", "fields": "id,name,desc,closed,url,idOrganization"}, nil)
		if err != nil {
			return nil, err
		}
		return s.filterObjectsByWorkspace(result, "idOrganization"), nil
	case "set_active_board":
		boardID := text(args, "boardId")
		if err := s.client.EnsureBoardAllowed(ctx, boardID); err != nil {
			return nil, err
		}
		board, err := s.client.Do(ctx, http.MethodGet, "/boards/"+escape(boardID), map[string]any{"fields": "id,name,desc,url,idOrganization"}, nil)
		if err != nil {
			return nil, err
		}
		if err := s.persistBoard(boardID); err != nil {
			return nil, err
		}
		return board, nil
	case "list_workspaces":
		result, err := s.client.Do(ctx, http.MethodGet, "/members/me/organizations", map[string]any{"fields": "id,displayName,name,desc,url"}, nil)
		if err != nil {
			return nil, err
		}
		return s.filterObjectsByWorkspace(result, "id"), nil
	case "create_board":
		workspaceID, err := s.workspaceID(args)
		if err != nil {
			return nil, err
		}
		params := map[string]any{"name": text(args, "name"), "idOrganization": workspaceID, "defaultLists": boolean(args, "defaultLists", true)}
		copyArg(params, "desc", args, "description")
		return s.client.Do(ctx, http.MethodPost, "/boards", params, nil)
	case "set_active_workspace":
		workspaceID := text(args, "workspaceId")
		if err := s.client.RequireWorkspaceAllowed(workspaceID); err != nil {
			return nil, err
		}
		workspace, err := s.client.Do(ctx, http.MethodGet, "/organizations/"+escape(workspaceID), map[string]any{"fields": "id,displayName,name,desc,url"}, nil)
		if err != nil {
			return nil, err
		}
		if err := s.persistWorkspace(workspaceID); err != nil {
			return nil, err
		}
		return workspace, nil
	case "list_boards_in_workspace":
		workspaceID := text(args, "workspaceId")
		if err := s.client.RequireWorkspaceAllowed(workspaceID); err != nil {
			return nil, err
		}
		return s.client.Do(ctx, http.MethodGet, "/organizations/"+escape(workspaceID)+"/boards", map[string]any{"filter": "open", "fields": "id,name,desc,closed,url,idOrganization"}, nil)
	case "get_active_board_info":
		boardID, err := s.board(ctx, nil)
		if err != nil {
			return nil, err
		}
		return s.client.Do(ctx, http.MethodGet, "/boards/"+escape(boardID), map[string]any{"fields": "id,name,desc,closed,url,idOrganization"}, nil)
	case "get_card":
		return s.getCard(ctx, text(args, "cardId"), getCardOptions{
			detailLevel:     textDefault(args, "detailLevel", "compact"),
			commentsLimit:   integer(args, "commentsLimit", 10),
			format:          textDefault(args, "format", "markdown"),
			delivery:        textDefault(args, "delivery", "auto"),
			includeMarkdown: boolean(args, "includeMarkdown", false),
		})
	case "add_comment":
		cardID := text(args, "cardId")
		if err := s.ensureCardAllowed(ctx, cardID); err != nil {
			return nil, err
		}
		return s.client.Do(ctx, http.MethodPost, "/cards/"+escape(cardID)+"/actions/comments", map[string]any{"text": text(args, "text")}, nil)
	case "update_comment", "delete_comment":
		commentID := text(args, "commentId")
		if err := s.ensureActionAllowed(ctx, commentID); err != nil {
			return nil, err
		}
		method, params := http.MethodDelete, map[string]any(nil)
		if name == "update_comment" {
			method, params = http.MethodPut, map[string]any{"text": text(args, "text")}
		}
		return s.client.Do(ctx, method, "/actions/"+escape(commentID)+"/comments", params, nil)
	case "get_card_comments":
		cardID := text(args, "cardId")
		if err := s.ensureCardAllowed(ctx, cardID); err != nil {
			return nil, err
		}
		comments, err := s.client.Do(ctx, http.MethodGet, "/cards/"+escape(cardID)+"/actions", map[string]any{
			"filter": "commentCard", "limit": integer(args, "limit", 10),
			"fields": "id,idMemberCreator,data,type,date",
		}, nil)
		if err != nil {
			return nil, err
		}
		return compactComments(comments), nil
	case "create_checklist":
		cardID := text(args, "cardId")
		if err := s.ensureCardAllowed(ctx, cardID); err != nil {
			return nil, err
		}
		params := map[string]any{"name": text(args, "name")}
		copyArg(params, "pos", args, "position")
		return s.client.Do(ctx, http.MethodPost, "/cards/"+escape(cardID)+"/checklists", params, nil)
	case "get_checklist_items", "get_checklist_by_name", "get_acceptance_criteria":
		checklistName := text(args, "name")
		if name == "get_acceptance_criteria" {
			checklistName = "Acceptance Criteria"
		}
		checklist, err := s.findChecklist(ctx, checklistName, text(args, "cardId"), text(args, "boardId"))
		if err != nil {
			return nil, err
		}
		if name == "get_checklist_items" || name == "get_acceptance_criteria" {
			return checklist["checkItems"], nil
		}
		return checklist, nil
	case "add_checklist_item":
		checklistID := text(args, "checklistId")
		if err := s.ensureChecklistAllowed(ctx, checklistID); err != nil {
			return nil, err
		}
		return s.client.Do(ctx, http.MethodPost, "/checklists/"+escape(checklistID)+"/checkItems", mapArgs(args, map[string]string{
			"name": "name", "checked": "checked", "position": "pos", "due": "due", "dueReminder": "dueReminder", "memberId": "idMember",
		}), nil)
	case "find_checklist_items_by_description":
		return s.findChecklistItems(ctx, text(args, "description"), text(args, "cardId"), text(args, "boardId"))
	case "update_checklist_item":
		cardID := text(args, "cardId")
		if err := s.ensureCardAllowed(ctx, cardID); err != nil {
			return nil, err
		}
		return s.client.Do(ctx, http.MethodPut, "/cards/"+escape(cardID)+"/checkItem/"+escape(text(args, "checkItemId")), mapArgs(args, map[string]string{
			"name": "name", "state": "state", "position": "pos", "due": "due", "dueReminder": "dueReminder", "memberId": "idMember",
		}), nil)
	case "delete_checklist_item":
		checklistID := text(args, "checklistId")
		if err := s.ensureChecklistAllowed(ctx, checklistID); err != nil {
			return nil, err
		}
		return s.client.Do(ctx, http.MethodDelete, "/checklists/"+escape(checklistID)+"/checkItems/"+escape(text(args, "checkItemId")), nil, nil)
	case "get_board_members":
		boardID, err := s.board(ctx, args)
		if err != nil {
			return nil, err
		}
		return s.client.Do(ctx, http.MethodGet, "/boards/"+escape(boardID)+"/members", map[string]any{"fields": "id,fullName,username,initials,avatarUrl"}, nil)
	case "assign_member_to_card", "remove_member_from_card":
		cardID := text(args, "cardId")
		if err := s.ensureCardAllowed(ctx, cardID); err != nil {
			return nil, err
		}
		memberID := text(args, "memberId")
		if name == "assign_member_to_card" {
			return s.client.Do(ctx, http.MethodPost, "/cards/"+escape(cardID)+"/idMembers", map[string]any{"value": memberID}, nil)
		}
		return s.client.Do(ctx, http.MethodDelete, "/cards/"+escape(cardID)+"/idMembers/"+escape(memberID), nil, nil)
	case "get_board_labels":
		boardID, err := s.board(ctx, args)
		if err != nil {
			return nil, err
		}
		return s.client.Do(ctx, http.MethodGet, "/boards/"+escape(boardID)+"/labels", map[string]any{"limit": 1000, "fields": "id,name,color,idBoard,uses"}, nil)
	case "create_label":
		boardID, err := s.board(ctx, args)
		if err != nil {
			return nil, err
		}
		return s.client.Do(ctx, http.MethodPost, "/labels", map[string]any{"idBoard": boardID, "name": text(args, "name"), "color": text(args, "color")}, nil)
	case "update_label":
		if err := s.ensureLabelAllowed(ctx, text(args, "labelId")); err != nil {
			return nil, err
		}
		return s.client.Do(ctx, http.MethodPut, "/labels/"+escape(text(args, "labelId")), mapArgs(args, map[string]string{"name": "name", "color": "color"}), nil)
	case "delete_label":
		if err := s.ensureLabelAllowed(ctx, text(args, "labelId")); err != nil {
			return nil, err
		}
		return s.client.Do(ctx, http.MethodDelete, "/labels/"+escape(text(args, "labelId")), nil, nil)
	case "copy_card":
		listID := text(args, "listId")
		if err := s.ensureListAllowed(ctx, listID); err != nil {
			return nil, err
		}
		if err := s.ensureCardAllowed(ctx, text(args, "sourceCardId")); err != nil {
			return nil, err
		}
		params := mapArgs(args, map[string]string{"sourceCardId": "idCardSource", "listId": "idList", "name": "name", "position": "pos"})
		if values, ok := args["keepFromSource"].([]any); ok {
			parts := make([]string, 0, len(values))
			for _, value := range values {
				parts = append(parts, fmt.Sprint(value))
			}
			params["keepFromSource"] = strings.Join(parts, ",")
		}
		return s.client.Do(ctx, http.MethodPost, "/cards", params, nil)
	case "copy_checklist":
		cardID := text(args, "cardId")
		if err := s.ensureCardAllowed(ctx, cardID); err != nil {
			return nil, err
		}
		if err := s.ensureChecklistAllowed(ctx, text(args, "sourceChecklistId")); err != nil {
			return nil, err
		}
		return s.client.Do(ctx, http.MethodPost, "/checklists", mapArgs(args, map[string]string{
			"sourceChecklistId": "idChecklistSource", "cardId": "idCard", "name": "name", "position": "pos",
		}), nil)
	case "add_cards_to_list":
		return s.addCards(ctx, text(args, "listId"), args["cards"].([]any))
	case "get_board_custom_fields":
		boardID, err := s.board(ctx, args)
		if err != nil {
			return nil, err
		}
		return s.client.Do(ctx, http.MethodGet, "/boards/"+escape(boardID)+"/customFields", nil, nil)
	case "update_card_custom_field":
		return s.updateCustomField(ctx, args)
	case "get_card_history":
		cardID := text(args, "cardId")
		if err := s.ensureCardAllowed(ctx, cardID); err != nil {
			return nil, err
		}
		return s.client.Do(ctx, http.MethodGet, "/cards/"+escape(cardID)+"/actions", map[string]any{"filter": "all", "limit": integer(args, "limit", 100)}, nil)
	case "download_attachment":
		cardID := text(args, "cardId")
		if err := s.ensureCardAllowed(ctx, cardID); err != nil {
			return nil, err
		}
		return s.client.DownloadAttachment(ctx, cardID, text(args, "attachmentId"), text(args, "destinationPath"))
	case "get_health":
		return s.health(ctx, false)
	case "get_health_detailed":
		return s.health(ctx, true)
	case "get_health_metadata":
		return s.healthMetadata()
	case "get_health_performance":
		start := time.Now()
		_, err := s.client.Do(ctx, http.MethodGet, "/members/me", map[string]any{"fields": "id"}, nil)
		return map[string]any{"ok": err == nil, "latency_ms": time.Since(start).Milliseconds(), "error": errorText(err)}, nil
	case "perform_system_repair":
		return s.repair()
	default:
		return nil, fmt.Errorf("tool %q is not implemented", name)
	}
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

func (s *Server) updateCardFlag(ctx context.Context, cardID, field string, value any) (any, error) {
	if err := s.ensureCardAllowed(ctx, cardID); err != nil {
		return nil, err
	}
	return s.client.Do(ctx, http.MethodPut, "/cards/"+escape(cardID), map[string]any{field: value}, nil)
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
	current := value
	for _, name := range path {
		object, ok := current.(map[string]any)
		if !ok {
			return ""
		}
		current = object[name]
	}
	result, _ := current.(string)
	return result
}

func errorText(err error) any {
	if err == nil {
		return nil
	}
	return err.Error()
}
