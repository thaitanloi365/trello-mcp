package mcpserver

import (
	"context"
	"encoding/base64"
	"errors"
	"fmt"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/thaitanloi365/trello-mcp/internal/config"
)

func (s *Server) createCards(ctx context.Context, args map[string]any) (any, error) {
	listID, boardRefs, err := s.resolveList(ctx, args, nil)
	if err != nil {
		return nil, err
	}
	cards, _ := args["cards"].([]any)
	if len(cards) == 0 {
		return nil, errors.New("cards must include at least one card")
	}
	// Resolve every name first so a bad label or member creates nothing.
	requests := make([]map[string]any, 0, len(cards))
	for index, raw := range cards {
		card, _ := raw.(map[string]any)
		params := map[string]any{"idList": listID}
		for input, output := range map[string]string{"name": "name", "description": "desc", "due": "due", "start": "start", "position": "pos"} {
			copyArg(params, output, card, input)
		}
		for input, kind := range map[string]string{"labels": "labels", "members": "members"} {
			values, _ := card[input].([]any)
			if len(values) == 0 {
				continue
			}
			ids, err := boardRefs.ids(ctx, kind, values)
			if err != nil {
				return nil, fmt.Errorf("cards[%d].%s: %w", index, input, err)
			}
			params["id"+strings.ToUpper(kind[:1])+kind[1:]] = strings.Join(ids, ",")
		}
		requests = append(requests, params)
	}
	created := make([]any, 0, len(requests))
	createdIDs := make([]string, 0, len(requests))
	for index, params := range requests {
		result, err := s.write(ctx, http.MethodPost, "/cards", params, nil)
		if err != nil {
			return nil, fmt.Errorf("cards[%d] failed; cards already created: [%s]: %w", index, strings.Join(createdIDs, ", "), err)
		}
		created = append(created, result)
		createdIDs = append(createdIDs, nestedString(result, "id"))
	}
	return created, nil
}

func (s *Server) updateCard(ctx context.Context, args map[string]any) (any, error) {
	cardID, err := s.allowedCard(ctx, args, "cardId")
	if err != nil {
		return nil, err
	}
	params := mapArgs(args, map[string]string{
		"name": "name", "description": "desc", "due": "due", "start": "start", "dueComplete": "dueComplete",
		"dueReminder": "dueReminder", "position": "pos", "closed": "closed", "subscribed": "subscribed",
	})
	sets := []struct{ field, kind, add, remove string }{
		{"idLabels", "labels", "addLabels", "removeLabels"}, {"idMembers", "members", "addMembers", "removeMembers"},
	}
	var clears []string // DELETE paths for removals that leave a set empty
	needsCard := text(args, "list") != ""
	for _, set := range sets {
		adds, _ := args[set.add].([]any)
		removes, _ := args[set.remove].([]any)
		needsCard = needsCard || len(adds)+len(removes) > 0
	}
	if needsCard {
		card, err := s.client.Do(ctx, http.MethodGet, "/cards/"+escape(cardID), map[string]any{"fields": "idBoard,idLabels,idMembers"}, nil)
		if err != nil {
			return nil, err
		}
		boardRefs := s.boardIDRefs(nestedString(card, "idBoard"))
		if list := text(args, "list"); list != "" {
			listID, err := boardRefs.id(ctx, "lists", list)
			if err != nil {
				return nil, err
			}
			if err := s.ensureListAllowed(ctx, listID); err != nil {
				return nil, err
			}
			params["idList"] = listID
		}
		// The final label and member sets go in the one PUT, so the update is
		// all-or-nothing and safe to repeat.
		for _, set := range sets {
			adds, _ := args[set.add].([]any)
			removes, _ := args[set.remove].([]any)
			if len(adds)+len(removes) == 0 {
				continue
			}
			addIDs, err := boardRefs.ids(ctx, set.kind, adds)
			if err != nil {
				return nil, fmt.Errorf("%s: %w", set.add, err)
			}
			removeIDs, err := boardRefs.ids(ctx, set.kind, removes)
			if err != nil {
				return nil, fmt.Errorf("%s: %w", set.remove, err)
			}
			current := stringSlice(nestedValue(card, set.field))
			if final := applySet(current, addIDs, removeIDs); len(final) > 0 {
				params[set.field] = strings.Join(final, ",")
				continue
			}
			// An empty set can't go in the PUT, so each removal uses Trello's
			// own DELETE; IDs already off the card need no call.
			for _, id := range current {
				clears = append(clears, "/cards/"+escape(cardID)+"/"+set.field+"/"+escape(id))
			}
		}
	}
	if len(params) == 0 && len(clears) == 0 {
		return nil, errors.New("nothing to update")
	}
	var result any = map[string]any{"id": cardID}
	if len(params) > 0 {
		if result, err = s.write(ctx, http.MethodPut, "/cards/"+escape(cardID), params, nil); err != nil {
			return nil, err
		}
	}
	for _, path := range clears {
		if _, err := s.client.Do(ctx, http.MethodDelete, path, nil, nil); err != nil {
			return nil, err
		}
	}
	return result, nil
}

// applySet returns current plus add, minus remove, without duplicates.
func applySet(current, add, remove []string) []string {
	drop := make(map[string]bool, len(remove))
	for _, id := range remove {
		drop[id] = true
	}
	result := make([]string, 0, len(current)+len(add))
	for _, id := range append(current, add...) {
		if !drop[id] {
			drop[id] = true
			result = append(result, id)
		}
	}
	return result
}

func (s *Server) setCustomField(ctx context.Context, args map[string]any) (any, error) {
	value := text(args, "value")
	if boolean(args, "clear", false) {
		value = ""
	} else if value == "" {
		return nil, errors.New("give value, or clear: true")
	}
	cardID, err := s.allowedCard(ctx, args, "cardId")
	if err != nil {
		return nil, err
	}
	index, err := s.cardRefs(cardID).load(ctx)
	if err != nil {
		return nil, err
	}
	field, err := match(objectSlice(index["customFields"]), "customFields", text(args, "field"))
	if err != nil {
		return nil, err
	}
	body, err := customFieldBody(field, value)
	if err != nil {
		return nil, err
	}
	path := "/cards/" + escape(cardID) + "/customField/" + escape(nestedString(field, "id")) + "/item"
	return s.write(ctx, http.MethodPut, path, nil, body)
}

// customFieldBody builds Trello's request body for a field definition. An
// empty value clears the field; Trello has no DELETE for field items.
func customFieldBody(field map[string]any, value string) (map[string]any, error) {
	if value == "" {
		return map[string]any{"idValue": "", "value": ""}, nil
	}
	switch kind := nestedString(field, "type"); kind {
	case "text", "date":
		return map[string]any{"value": map[string]any{kind: value}}, nil
	case "number":
		if _, err := strconv.ParseFloat(value, 64); err != nil {
			return nil, fmt.Errorf("%s needs a number: %w", nestedString(field, "name"), err)
		}
		return map[string]any{"value": map[string]any{"number": value}}, nil
	case "checkbox":
		checked, err := strconv.ParseBool(value)
		if err != nil {
			return nil, fmt.Errorf("%s needs true or false", nestedString(field, "name"))
		}
		return map[string]any{"value": map[string]any{"checked": strconv.FormatBool(checked)}}, nil
	case "list":
		options := make([]string, 0)
		for _, option := range objectSlice(field["options"]) {
			text := nestedString(option, "value", "text")
			if nestedString(option, "id") == value || strings.EqualFold(text, value) {
				return map[string]any{"idValue": nestedString(option, "id")}, nil
			}
			options = append(options, text)
		}
		return nil, fmt.Errorf("%s option %q not found; options: %s", nestedString(field, "name"), value, strings.Join(options, ", "))
	default:
		return nil, fmt.Errorf("unsupported custom field type %q", kind)
	}
}

func (s *Server) addAttachment(ctx context.Context, args map[string]any) (any, error) {
	cardID, err := s.allowedCard(ctx, args, "cardId")
	if err != nil {
		return nil, err
	}
	link, filePath, data, name := text(args, "url"), text(args, "filePath"), text(args, "data"), text(args, "name")
	sources := 0
	for _, source := range []string{link, filePath, data} {
		if source != "" {
			sources++
		}
	}
	if sources != 1 {
		return nil, errors.New("give exactly one of url, filePath, or data")
	}
	switch {
	case link != "":
		params := map[string]any{"url": link}
		copyArg(params, "name", args, "name")
		copyArg(params, "setCover", args, "setCover")
		return s.write(ctx, http.MethodPost, "/cards/"+escape(cardID)+"/attachments", params, nil)
	case filePath != "":
		return acked(s.client.UploadFile(ctx, cardID, filePath, name))
	default:
		if name == "" {
			return nil, errors.New("name is required with data")
		}
		decoded, err := base64.StdEncoding.DecodeString(data)
		if err != nil {
			return nil, fmt.Errorf("decode data: %w", err)
		}
		return acked(s.client.UploadData(ctx, cardID, name, decoded))
	}
}

func (s *Server) setActive(ctx context.Context, boardID, workspaceID string) (any, error) {
	if boardID == "" && workspaceID == "" {
		return nil, errors.New("give boardId, workspaceId, or both")
	}
	// Validate both before saving either.
	result := make(map[string]any)
	if workspaceID != "" {
		if err := s.client.RequireWorkspaceAllowed(workspaceID); err != nil {
			return nil, err
		}
		workspace, err := s.client.Do(ctx, http.MethodGet, "/organizations/"+escape(workspaceID), map[string]any{"fields": "displayName"}, nil)
		if err != nil {
			return nil, err
		}
		result["workspace"] = nestedString(workspace, "displayName")
	}
	if boardID != "" {
		if err := s.client.EnsureBoardAllowed(ctx, boardID); err != nil {
			return nil, err
		}
		board, err := s.client.Do(ctx, http.MethodGet, "/boards/"+escape(boardID), map[string]any{"fields": "name"}, nil)
		if err != nil {
			return nil, err
		}
		result["board"] = nestedString(board, "name")
	}
	return result, s.client.Manager().Update(func(cfg *config.Config) error {
		if workspaceID != "" {
			cfg.WorkspaceID = workspaceID
		}
		if boardID != "" {
			cfg.DefaultBoardID = boardID
		}
		return nil
	})
}

func (s *Server) listBoards(ctx context.Context, workspaceID string) (any, error) {
	if workspaceID != "" {
		if err := s.client.RequireWorkspaceAllowed(workspaceID); err != nil {
			return nil, err
		}
		return s.client.Do(ctx, http.MethodGet, "/organizations/"+escape(workspaceID)+"/boards", map[string]any{"filter": "open", "fields": "id,name"}, nil)
	}
	result, err := s.client.Do(ctx, http.MethodGet, "/members/me/boards", map[string]any{
		"filter": "open", "fields": "id,name,idOrganization", "organization": true, "organization_fields": "displayName",
	}, nil)
	if err != nil {
		return nil, err
	}
	boards := make([]any, 0)
	for _, board := range objectSlice(s.filterObjectsByWorkspace(result, "idOrganization")) {
		row := map[string]any{"id": board["id"], "name": board["name"]}
		if workspace := nestedString(board, "organization", "displayName"); workspace != "" {
			row["workspace"] = workspace
		}
		boards = append(boards, row)
	}
	return boards, nil
}

func (s *Server) activity(ctx context.Context, args map[string]any) (any, error) {
	var path string
	cardScoped := text(args, "cardId") != ""
	if cardScoped {
		cardID, err := s.allowedCard(ctx, args, "cardId")
		if err != nil {
			return nil, err
		}
		path = "/cards/" + escape(cardID) + "/actions"
	} else {
		boardID, err := s.board(ctx, args)
		if err != nil {
			return nil, err
		}
		path = "/boards/" + escape(boardID) + "/actions"
	}
	actions, err := s.client.Do(ctx, http.MethodGet, path, map[string]any{
		"filter": "all", "limit": integer(args, "limit", 50), "memberCreator_fields": "fullName",
	}, nil)
	if err != nil {
		return nil, err
	}
	result := compactActions(actions)
	if cardScoped {
		for _, action := range result {
			delete(action.(map[string]any), "card")
			delete(action.(map[string]any), "cardId")
		}
	}
	return result, nil
}

func (s *Server) allChecklists(ctx context.Context, cardRef, boardID string) ([]map[string]any, error) {
	if cardRef != "" {
		cardID, err := normalizeCardRef(cardRef)
		if err != nil {
			return nil, err
		}
		if err := s.ensureCardAllowed(ctx, cardID); err != nil {
			return nil, err
		}
		value, err := s.client.Do(ctx, http.MethodGet, "/cards/"+escape(cardID)+"/checklists", map[string]any{
			"fields": "name", "checkItems": "all", "checkItem_fields": "name,state,due,idMember",
		}, nil)
		if err != nil {
			return nil, err
		}
		return objectSlice(value), nil
	}
	resolved, err := s.board(ctx, map[string]any{"boardId": boardID})
	if err != nil {
		return nil, err
	}
	value, err := s.client.Do(ctx, http.MethodGet, "/boards/"+escape(resolved)+"/cards", map[string]any{
		"filter": "visible", "fields": "shortLink,name", "checklists": "all", "checkItem_fields": "name,state,due,idMember", "limit": 1000,
	}, nil)
	if err != nil {
		return nil, err
	}
	result := make([]map[string]any, 0)
	for _, card := range objectSlice(value) {
		for _, checklist := range objectSlice(card["checklists"]) {
			checklist["cardId"], checklist["card"] = card["shortLink"], card["name"]
			result = append(result, checklist)
		}
	}
	return result, nil
}

func (s *Server) findChecklistItems(ctx context.Context, args map[string]any) (any, error) {
	query, checklistName := strings.ToLower(text(args, "query")), text(args, "checklist")
	if query == "" && checklistName == "" {
		return nil, errors.New("give query, checklist, or both")
	}
	checklists, err := s.allChecklists(ctx, text(args, "cardId"), text(args, "boardId"))
	if err != nil {
		return nil, err
	}
	matches := make([]any, 0)
	checklistFound := false
	var index map[string]any
	for _, checklist := range checklists {
		if checklistName != "" && !strings.EqualFold(nestedString(checklist, "name"), checklistName) {
			continue
		}
		checklistFound = true
		// Items are grouped under their checklist so card and checklist names
		// appear once, not on every item.
		items := make([]any, 0)
		for _, item := range objectSlice(checklist["checkItems"]) {
			if query != "" && !strings.Contains(strings.ToLower(nestedString(item, "name")), query) {
				continue
			}
			row := make(map[string]any)
			copyPresent(row, item, "id", "name", "state", "due")
			if memberID := nestedString(item, "idMember"); memberID != "" {
				if index == nil {
					if index, err = s.checklistBoardRefs(args).load(ctx); err != nil {
						return nil, err
					}
				}
				row["member"] = memberID
				if names := namesOf(index, "members", []any{memberID}); len(names) == 1 {
					row["member"] = names[0]
				}
			}
			items = append(items, row)
		}
		if len(items) > 0 {
			group := map[string]any{"checklistId": checklist["id"], "checklist": checklist["name"], "items": items}
			copyPresent(group, checklist, "cardId", "card")
			matches = append(matches, group)
		}
	}
	if checklistName != "" && !checklistFound {
		names := make([]string, 0, len(checklists))
		for _, checklist := range checklists {
			if name := nestedString(checklist, "name"); !slices.Contains(names, name) {
				names = append(names, name)
			}
		}
		return nil, fmt.Errorf("checklist %q not found; checklists: %s", checklistName, strings.Join(names, ", "))
	}
	return matches, nil
}

// checklistBoardRefs resolves member names on the board find_checklist_items
// searched: the card's board, or boardId, or the active board.
func (s *Server) checklistBoardRefs(args map[string]any) *refs {
	if cardID, err := normalizeCardRef(text(args, "cardId")); err == nil {
		return s.cardRefs(cardID)
	}
	return s.boardRefs(args)
}

func (s *Server) createChecklist(ctx context.Context, args map[string]any) (any, error) {
	items, _ := args["items"].([]any)
	for index, item := range items {
		if name, ok := item.(string); !ok || strings.TrimSpace(name) == "" {
			return nil, fmt.Errorf("items[%d] must be non-empty text", index)
		}
	}
	cardID, err := s.allowedCard(ctx, args, "cardId")
	if err != nil {
		return nil, err
	}
	params := map[string]any{"name": text(args, "name")}
	copyArg(params, "pos", args, "position")
	checklist, err := s.write(ctx, http.MethodPost, "/cards/"+escape(cardID)+"/checklists", params, nil)
	if err != nil || len(items) == 0 {
		return checklist, err
	}
	checklistID := nestedString(checklist, "id")
	if checklistID == "" {
		return nil, errors.New("Trello checklist response did not include an id")
	}
	created := make([]any, 0, len(items))
	for index, item := range items {
		result, err := s.write(ctx, http.MethodPost, "/checklists/"+escape(checklistID)+"/checkItems", map[string]any{"name": item}, nil)
		if err != nil {
			return nil, fmt.Errorf("checklist %s was created with items[0:%d]; add the rest with add_checklist_item, not create_checklist: %w", checklistID, index, err)
		}
		created = append(created, result)
	}
	checklist.(map[string]any)["checkItems"] = created
	return checklist, nil
}

type getCardOptions struct {
	commentsLimit int
	delivery      string
	outputDir     string
	fileThreshold int
}

func (options getCardOptions) normalized() (getCardOptions, error) {
	if options.commentsLimit < 0 || options.commentsLimit > 100 {
		return options, errors.New("commentsLimit must be between 0 and 100")
	}
	options.delivery = strings.ToLower(strings.TrimSpace(options.delivery))
	if options.delivery == "" {
		options.delivery = "auto"
	}
	if options.delivery != "inline" && options.delivery != "file" && options.delivery != "auto" {
		return options, errors.New("delivery must be inline, file, or auto")
	}
	if options.fileThreshold <= 0 {
		options.fileThreshold = defaultCardFileThreshold
	}
	return options, nil
}

func (s *Server) getCard(ctx context.Context, cardRef string, options getCardOptions) (any, error) {
	cardID, err := normalizeCardRef(cardRef)
	if err != nil {
		return nil, err
	}
	options, err = options.normalized()
	if err != nil {
		return nil, err
	}
	if err := s.ensureCardAllowed(ctx, cardID); err != nil {
		return nil, err
	}
	card, err := s.client.Do(ctx, http.MethodGet, "/cards/"+escape(cardID), cardRequestParams(options), nil)
	if err != nil {
		return nil, err
	}
	object, ok := card.(map[string]any)
	if !ok {
		return nil, errors.New("Trello returned an invalid card response")
	}
	// Checklist assignees need not be card members; name them from the board.
	for _, checklist := range objectSlice(object["checklists"]) {
		for _, item := range objectSlice(checklist["checkItems"]) {
			memberID := nestedString(item, "idMember")
			if memberID != "" && object["boardMembers"] == nil && len(namesOf(object, "members", []any{memberID})) == 0 {
				boardMembers, err := s.client.Do(ctx, http.MethodGet, "/boards/"+escape(nestedString(object, "idBoard"))+"/members", map[string]any{"fields": "fullName"}, nil)
				if err != nil {
					return nil, err
				}
				object["boardMembers"] = boardMembers
			}
		}
	}
	var fields []map[string]any
	if len(objectSlice(object["customFieldItems"])) > 0 {
		definitions, err := s.client.Do(ctx, http.MethodGet, "/boards/"+escape(nestedString(object, "idBoard"))+"/customFields", nil, nil)
		if err != nil {
			return nil, err
		}
		fields = objectSlice(definitions)
	}
	return deliverCard(cardID, renderCardMarkdown(object, fields, options.commentsLimit), options)
}

func cardRequestParams(options getCardOptions) map[string]any {
	params := map[string]any{
		"fields": "idBoard,name,desc,closed,due,dueComplete,dueReminder,start,shortUrl,labels",
		"list":   true, "list_fields": "name", "board": true, "board_fields": "name",
		"members": true, "member_fields": "id,fullName,username",
		"attachments": true, "attachment_fields": "id,name,url",
		"checklists": "all", "checklist_fields": "name", "checkItem_fields": "id,name,state,due,idMember",
		"customFieldItems": true,
	}
	if options.commentsLimit > 0 {
		params["actions"] = "commentCard"
		params["actions_limit"] = options.commentsLimit
		params["action_fields"] = "id,data,date"
		params["action_memberCreator_fields"] = "fullName,username"
	}
	return params
}

func deliverCard(cardID, markdown string, options getCardOptions) (any, error) {
	if options.delivery == "inline" || (options.delivery == "auto" && len(markdown) <= options.fileThreshold) {
		return rawTextResult(markdown), nil
	}
	path, err := writeCardOutput(cardID, "markdown", []byte(markdown), options.outputDir)
	if err != nil {
		return nil, err
	}
	return map[string]any{
		"path": path, "bytes": len(markdown),
		"instruction": "The card is too large to return inline; read the Markdown file at path.",
	}, nil
}

// renderCardMarkdown renders a card in the order a reader needs it: status,
// description, checklists, attachments, then comments. IDs appear only where a
// write tool needs them.
func renderCardMarkdown(card map[string]any, fieldDefinitions []map[string]any, commentsLimit int) string {
	var out strings.Builder
	line := func(label, value string) {
		if value != "" {
			out.WriteString("- " + label + ": " + value + "\n")
		}
	}
	out.WriteString("# " + nestedString(card, "name") + "\n\n")
	if board := nestedString(card, "board", "name"); board != "" {
		line("Board", board+" (board "+nestedString(card, "idBoard")+")")
	}
	line("List", nestedString(card, "list", "name"))
	if closed, _ := card["closed"].(bool); closed {
		line("Status", "Archived")
	}
	line("Start", nestedString(card, "start"))
	if due := nestedString(card, "due"); due != "" {
		if complete, _ := card["dueComplete"].(bool); complete {
			due += " (done)"
		}
		if reminder, ok := card["dueReminder"].(float64); ok && reminder >= 0 {
			due += fmt.Sprintf(", reminder %g min before", reminder)
		}
		line("Due", due)
	}
	labels := make([]string, 0)
	for _, label := range objectSlice(card["labels"]) {
		labels = append(labels, displayName("labels", label))
	}
	line("Labels", strings.Join(labels, ", "))
	members := make([]string, 0)
	memberNames := make(map[string]string)
	for _, member := range objectSlice(card["boardMembers"]) {
		memberNames[nestedString(member, "id")] = nestedString(member, "fullName")
	}
	for _, member := range objectSlice(card["members"]) {
		members = append(members, nestedString(member, "fullName")+" (@"+nestedString(member, "username")+")")
		memberNames[nestedString(member, "id")] = nestedString(member, "fullName")
	}
	line("Members", strings.Join(members, ", "))
	definitions := make(map[string]map[string]any)
	for _, definition := range fieldDefinitions {
		definitions[nestedString(definition, "id")] = definition
	}
	for _, item := range objectSlice(card["customFieldItems"]) {
		definition := definitions[nestedString(item, "idCustomField")]
		line(nestedString(definition, "name"), customFieldText(definition, item))
	}
	line("Link", nestedString(card, "shortUrl"))
	if description := nestedString(card, "desc"); description != "" {
		out.WriteString("\n## Description\n\n" + description + "\n")
	}
	for _, checklist := range objectSlice(card["checklists"]) {
		out.WriteString("\n## " + nestedString(checklist, "name") + " (checklist " + nestedString(checklist, "id") + ")\n\n")
		for _, item := range objectSlice(checklist["checkItems"]) {
			mark := " "
			if nestedString(item, "state") == "complete" {
				mark = "x"
			}
			details := "item " + nestedString(item, "id")
			if due := nestedString(item, "due"); due != "" {
				details += ", due " + due
			}
			if memberID := nestedString(item, "idMember"); memberID != "" {
				name := memberNames[memberID]
				if name == "" {
					name = "member " + memberID
				}
				details += ", assigned " + name
			}
			out.WriteString(fmt.Sprintf("- [%s] %s (%s)\n", mark, nestedString(item, "name"), details))
		}
	}
	if attachments := objectSlice(card["attachments"]); len(attachments) > 0 {
		out.WriteString("\n## Attachments\n\n")
		for _, attachment := range attachments {
			out.WriteString(fmt.Sprintf("- [%s](%s) (attachment %s)\n", nestedString(attachment, "name"), nestedString(attachment, "url"), nestedString(attachment, "id")))
		}
	}
	if comments := objectSlice(card["actions"]); len(comments) > 0 {
		out.WriteString("\n## Latest comments\n")
		if len(comments) == commentsLimit {
			out.WriteString(fmt.Sprintf("\nShowing the latest %d; raise commentsLimit for more.\n", commentsLimit))
		}
		for _, comment := range comments {
			author := nestedString(comment, "memberCreator", "fullName")
			if username := nestedString(comment, "memberCreator", "username"); username != "" {
				author += " (@" + username + ")"
			}
			out.WriteString(fmt.Sprintf("\n### %s, %s (comment %s)\n\n%s\n", author, nestedString(comment, "date"), nestedString(comment, "id"), nestedString(comment, "data", "text")))
		}
	}
	return strings.TrimSpace(out.String())
}

func customFieldText(definition map[string]any, item map[string]any) string {
	if optionID := nestedString(item, "idValue"); optionID != "" {
		for _, option := range objectSlice(definition["options"]) {
			if nestedString(option, "id") == optionID {
				return nestedString(option, "value", "text")
			}
		}
	}
	value, _ := item["value"].(map[string]any)
	for _, key := range []string{"text", "number", "date", "checked"} {
		if text, ok := value[key].(string); ok && text != "" {
			return text
		}
	}
	return ""
}

func (s *Server) health(ctx context.Context) (any, error) {
	start := time.Now()
	_, apiErr := s.client.Do(ctx, http.MethodGet, "/members/me", map[string]any{"fields": "id"}, nil)
	result := map[string]any{
		"ok": apiErr == nil, "version": Version, "latency_ms": time.Since(start).Milliseconds(),
		"config_path": s.client.Manager().Path(),
	}
	if apiErr != nil {
		result["error"] = apiErr.Error()
	}
	if cfg, err := s.client.Manager().Effective(); err == nil {
		result["active_board_id"] = cfg.DefaultBoardID
		result["active_workspace_id"] = cfg.WorkspaceID
		result["allowed_workspace_count"] = len(cfg.AllowedWorkspaceIDs)
		result["credentials_configured"] = cfg.APIKey != "" && cfg.Token != ""
		result["api_base_url"] = cfg.APIBaseURL
		result["request_timeout_seconds"] = cfg.RequestTimeoutSecs
		result["max_upload_bytes"] = cfg.MaxUploadBytes
	}
	return result, nil
}

func (s *Server) filterObjectsByWorkspace(value any, field string) any {
	cfg, err := s.client.Manager().Effective()
	if err != nil || len(cfg.AllowedWorkspaceIDs) == 0 {
		return value
	}
	items, ok := value.([]any)
	if !ok {
		return value
	}
	filtered := make([]any, 0, len(items))
	for _, item := range items {
		workspaceID := nestedString(item, field)
		if workspaceID != "" && s.client.WorkspaceAllowed(workspaceID) {
			filtered = append(filtered, item)
		}
	}
	return filtered
}

func (s *Server) filterByWorkspace(ctx context.Context, value any) (any, error) {
	cfg, err := s.client.Manager().Effective()
	if err != nil || len(cfg.AllowedWorkspaceIDs) == 0 {
		return value, err
	}
	items, ok := value.([]any)
	if !ok {
		return value, nil
	}
	allowedBoards := make(map[string]bool)
	filtered := make([]any, 0, len(items))
	for _, item := range items {
		boardID := nestedString(item, "idBoard")
		allowed, exists := allowedBoards[boardID]
		if !exists {
			allowed = s.client.EnsureBoardAllowed(ctx, boardID) == nil
			allowedBoards[boardID] = allowed
		}
		if allowed {
			filtered = append(filtered, item)
		}
	}
	return filtered, nil
}

func objectSlice(value any) []map[string]any {
	if objects, ok := value.([]map[string]any); ok {
		return objects
	}
	items, ok := value.([]any)
	if !ok {
		return nil
	}
	result := make([]map[string]any, 0, len(items))
	for _, item := range items {
		if object, ok := item.(map[string]any); ok {
			result = append(result, object)
		}
	}
	return result
}

var (
	attachmentURL = regexp.MustCompile(`^https://trello\.com/1/cards/\w+/attachments/\w+/download/(\S+)$`)
	word          = regexp.MustCompile(`\S+`)
)

// linkAttachments turns bare attachment URLs into [file name](url) links. Only
// whole words outside code spans change, so URLs already wrapped in Markdown
// (links, bold, quotes, backticks) stay as written.
func linkAttachments(text string) string {
	parts := strings.Split(text, "`")
	for index := 0; index < len(parts); index += 2 {
		parts[index] = word.ReplaceAllStringFunc(parts[index], linkAttachment)
	}
	return strings.Join(parts, "`")
}

func linkAttachment(word string) string {
	link := strings.TrimRight(word, ".,;:!?)")
	match := attachmentURL.FindStringSubmatch(link)
	if match == nil || strings.Contains(match[1], "://") {
		return word
	}
	name, err := url.PathUnescape(match[1])
	if err != nil {
		name = match[1]
	}
	target := strings.NewReplacer("(", "%28", ")", "%29").Replace(link)
	return "[" + name + "](" + target + ")" + word[len(link):]
}

// compactActions flattens Trello actions, about 3 KiB each, to who did what
// to which card, with each changed field's old and new value.
func compactActions(value any) []any {
	result := make([]any, 0)
	for _, action := range objectSlice(value) {
		compact := make(map[string]any)
		copyPresent(compact, action, "id", "type", "date")
		for key, path := range map[string][]string{
			"by": {"memberCreator", "fullName"}, "member": {"member", "fullName"},
			"card": {"data", "card", "name"}, "cardId": {"data", "card", "shortLink"}, "list": {"data", "list", "name"},
			"from": {"data", "listBefore", "name"}, "to": {"data", "listAfter", "name"}, "text": {"data", "text"},
			"checklist": {"data", "checklist", "name"}, "checkItem": {"data", "checkItem", "name"},
			"state": {"data", "checkItem", "state"}, "attachment": {"data", "attachment", "name"},
			"attachmentId": {"data", "attachment", "id"}, "label": {"data", "label", "name"}, "field": {"data", "customField", "name"},
		} {
			if value := nestedString(action, path...); value != "" {
				compact[key] = value
			}
		}
		if old, ok := nestedValue(action, "data", "old").(map[string]any); ok && len(old) > 0 {
			// updateCheckItem changes data.checkItem, updateList data.list, and so on.
			entity := strings.TrimPrefix(nestedString(action, "type"), "update")
			if entity != "" {
				entity = strings.ToLower(entity[:1]) + entity[1:]
			}
			changed := make(map[string]any, len(old))
			for key, before := range old {
				changed[key] = map[string]any{"from": shorten(before), "to": shorten(nestedValue(action, "data", entity, key))}
			}
			compact["changed"] = changed
		}
		result = append(result, compact)
	}
	return result
}

// shorten keeps long text, such as an edited description, from flooding an
// activity entry.
func shorten(value any) any {
	text, ok := value.(string)
	if runes := []rune(text); ok && len(runes) > 120 {
		return string(runes[:120]) + "…"
	}
	return value
}

func nestedValue(value any, path ...string) any {
	for _, name := range path {
		object, ok := value.(map[string]any)
		if !ok {
			return nil
		}
		value = object[name]
	}
	return value
}

const defaultCardFileThreshold = 16 * 1024

func normalizeCardRef(value string) (string, error) {
	value = strings.TrimSpace(value)
	if value == "" {
		return "", errors.New("card reference is required")
	}
	lower := strings.ToLower(value)
	if strings.HasPrefix(lower, "trello.com/") || strings.HasPrefix(lower, "www.trello.com/") {
		value = "https://" + value
	}
	if parsed, err := url.Parse(value); err == nil && parsed.Host != "" {
		if parsed.Scheme != "http" && parsed.Scheme != "https" {
			return "", errors.New("Trello card URL must use http or https")
		}
		host := strings.ToLower(parsed.Hostname())
		if host != "trello.com" && host != "www.trello.com" {
			return "", fmt.Errorf("unsupported Trello card URL host %q", parsed.Hostname())
		}
		parts := strings.Split(strings.Trim(parsed.EscapedPath(), "/"), "/")
		if len(parts) < 2 || !strings.EqualFold(parts[0], "c") {
			return "", errors.New("Trello card URL must have the form trello.com/c/{shortLink}")
		}
		decoded, err := url.PathUnescape(parts[1])
		if err != nil {
			return "", fmt.Errorf("decode Trello short link: %w", err)
		}
		value = decoded
	}
	if strings.ContainsAny(value, "/?#") {
		return "", errors.New("card reference must be a card ID, short link, or Trello card URL")
	}
	return value, nil
}

func writeCardOutput(cardID, format string, payload []byte, outputDir string) (string, error) {
	if outputDir == "" {
		cacheDir, err := os.UserCacheDir()
		if err != nil {
			return "", fmt.Errorf("resolve Trello MCP cache directory: %w", err)
		}
		outputDir = filepath.Join(cacheDir, "trello-mcp", "outputs")
	}
	if err := os.MkdirAll(outputDir, 0o700); err != nil {
		return "", fmt.Errorf("create Trello MCP output directory: %w", err)
	}
	if err := os.Chmod(outputDir, 0o700); err != nil {
		return "", fmt.Errorf("secure Trello MCP output directory: %w", err)
	}

	extension := "json"
	if format == "markdown" {
		extension = "md"
	}
	temp, err := os.CreateTemp(outputDir, ".card-*.tmp")
	if err != nil {
		return "", fmt.Errorf("create temporary Trello MCP output: %w", err)
	}
	tempPath := temp.Name()
	cleanup := func() { _ = os.Remove(tempPath) }
	if err := temp.Chmod(0o600); err != nil {
		_ = temp.Close()
		cleanup()
		return "", fmt.Errorf("secure temporary Trello MCP output: %w", err)
	}
	if _, err := temp.Write(append(payload, '\n')); err != nil {
		_ = temp.Close()
		cleanup()
		return "", fmt.Errorf("write temporary Trello MCP output: %w", err)
	}
	if err := temp.Sync(); err != nil {
		_ = temp.Close()
		cleanup()
		return "", fmt.Errorf("sync temporary Trello MCP output: %w", err)
	}
	if err := temp.Close(); err != nil {
		cleanup()
		return "", fmt.Errorf("close temporary Trello MCP output: %w", err)
	}

	randomPart := strings.TrimSuffix(strings.TrimPrefix(filepath.Base(tempPath), ".card-"), ".tmp")
	filename := fmt.Sprintf("card-%s-%s-%s.%s",
		safeFilePart(cardID), time.Now().UTC().Format("20060102T150405Z"), randomPart, extension)
	path := filepath.Join(outputDir, filename)
	if err := os.Rename(tempPath, path); err != nil {
		cleanup()
		return "", fmt.Errorf("save Trello MCP output: %w", err)
	}
	if err := os.Chmod(path, 0o600); err != nil {
		return "", fmt.Errorf("secure Trello MCP output: %w", err)
	}
	absolute, err := filepath.Abs(path)
	if err != nil {
		return "", fmt.Errorf("resolve Trello MCP output path: %w", err)
	}
	return absolute, nil
}

func safeFilePart(value string) string {
	var out strings.Builder
	for _, char := range value {
		switch {
		case char >= 'a' && char <= 'z',
			char >= 'A' && char <= 'Z',
			char >= '0' && char <= '9',
			char == '-', char == '_':
			out.WriteRune(char)
		default:
			out.WriteByte('-')
		}
		if out.Len() >= 48 {
			break
		}
	}
	result := strings.Trim(out.String(), "-")
	if result == "" {
		return "card"
	}
	return result
}

func compactObjects(value any, fields ...string) []any {
	result := make([]any, 0)
	for _, object := range objectSlice(value) {
		compact := make(map[string]any)
		copyPresent(compact, object, fields...)
		result = append(result, compact)
	}
	return result
}

func copyPresent(target, source map[string]any, fields ...string) {
	for _, field := range fields {
		value, exists := source[field]
		if !exists || value == nil {
			continue
		}
		if text, ok := value.(string); ok && text == "" {
			continue
		}
		target[field] = value
	}
}
