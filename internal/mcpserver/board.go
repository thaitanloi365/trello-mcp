package mcpserver

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"strings"
)

// boardIndex fetches everything needed to turn a board's IDs into names and
// back in one request: lists, labels, members, and custom fields.
func (s *Server) boardIndex(ctx context.Context, boardID string) (map[string]any, error) {
	if err := s.client.EnsureBoardAllowed(ctx, boardID); err != nil {
		return nil, err
	}
	board, err := s.client.Do(ctx, http.MethodGet, "/boards/"+escape(boardID), map[string]any{
		"fields": "name,url", "lists": "all", "list_fields": "name,closed",
		"labels": "all", "label_fields": "name,color", "labels_limit": 1000,
		"members": "all", "member_fields": "fullName,username", "customFields": true,
	}, nil)
	if err != nil {
		return nil, err
	}
	index, ok := board.(map[string]any)
	if !ok {
		return nil, errors.New("Trello returned an invalid board response")
	}
	return index, nil
}

// boardSummary is the get_board result: open lists, labels, members, and
// custom fields with the IDs other tools accept.
func boardSummary(index map[string]any) map[string]any {
	lists := make([]any, 0)
	for _, list := range objectSlice(index["lists"]) {
		if closed, _ := list["closed"].(bool); !closed {
			lists = append(lists, map[string]any{"id": list["id"], "name": list["name"]})
		}
	}
	fields := make([]any, 0)
	for _, field := range objectSlice(index["customFields"]) {
		compact := map[string]any{"id": field["id"], "name": field["name"], "type": field["type"]}
		if options := objectSlice(field["options"]); len(options) > 0 {
			texts := make([]string, 0, len(options))
			for _, option := range options {
				texts = append(texts, nestedString(option, "value", "text"))
			}
			compact["options"] = texts
		}
		fields = append(fields, compact)
	}
	result := map[string]any{
		"lists":   lists,
		"labels":  compactObjects(index["labels"], "id", "name", "color"),
		"members": compactObjects(index["members"], "id", "fullName", "username"),
	}
	copyPresent(result, index, "id", "name", "url")
	if len(fields) > 0 {
		result["customFields"] = fields
	}
	return result
}

// refs resolves list, label, member, and custom field names to IDs on one
// board. The board is fetched once, and only when a value is not an ID.
type refs struct {
	server *Server
	board  func(context.Context) (string, error)
	index  map[string]any
}

func (s *Server) boardRefs(args map[string]any) *refs {
	return &refs{server: s, board: func(ctx context.Context) (string, error) { return s.board(ctx, args) }}
}

// boardIDRefs resolves names on a board whose ID the caller already has.
func (s *Server) boardIDRefs(boardID string) *refs {
	return &refs{server: s, board: func(context.Context) (string, error) { return boardID, nil }}
}

func (s *Server) cardRefs(cardID string) *refs {
	return s.parentRefs("/cards/" + escape(cardID))
}

func (s *Server) checklistRefs(checklistID string) *refs {
	return s.parentRefs("/checklists/" + escape(checklistID))
}

// parentRefs resolves names on the board of the card, list, or checklist at
// path.
func (s *Server) parentRefs(path string) *refs {
	return &refs{server: s, board: func(ctx context.Context) (string, error) {
		object, err := s.client.Do(ctx, http.MethodGet, path, map[string]any{"fields": "idBoard"}, nil)
		if err != nil {
			return "", err
		}
		if boardID := nestedString(object, "idBoard"); boardID != "" {
			return boardID, nil
		}
		return "", errors.New("Trello response did not include idBoard")
	}}
}

func (r *refs) load(ctx context.Context) (map[string]any, error) {
	if r.index != nil {
		return r.index, nil
	}
	boardID, err := r.board(ctx)
	if err != nil {
		return nil, err
	}
	r.index, err = r.server.boardIndex(ctx, boardID)
	return r.index, err
}

// id resolves one value of kind lists, labels, members, or customFields.
func (r *refs) id(ctx context.Context, kind, value string) (string, error) {
	value = strings.TrimSpace(value)
	if isTrelloID(value) {
		return value, nil
	}
	if kind == "members" && strings.EqualFold(strings.TrimPrefix(value, "@"), "me") {
		me, err := r.server.client.Do(ctx, http.MethodGet, "/members/me", map[string]any{"fields": "id"}, nil)
		return nestedString(me, "id"), err
	}
	index, err := r.load(ctx)
	if err != nil {
		return "", err
	}
	item, err := match(objectSlice(index[kind]), kind, value)
	return nestedString(item, "id"), err
}

func (r *refs) ids(ctx context.Context, kind string, values []any) ([]string, error) {
	result := make([]string, 0, len(values))
	for _, value := range values {
		id, err := r.id(ctx, kind, fmt.Sprint(value))
		if err != nil {
			return nil, err
		}
		result = append(result, id)
	}
	return result, nil
}

// match finds the one item whose ID or name equals value, ignoring case.
// Open items win over archived ones, and names win over the fallback key:
// username for members, color for labels.
func match(items []map[string]any, kind, value string) (map[string]any, error) {
	value = strings.TrimSpace(value)
	want := strings.ToLower(value)
	nameKey, fallbackKey := "name", map[string]string{"members": "username", "labels": "color"}[kind]
	if kind == "members" {
		nameKey, want = "fullName", strings.TrimPrefix(want, "@")
	}
	var byName, byFallback [2][]map[string]any // index 0 open, 1 archived
	options := make([]string, 0, len(items))
	for _, item := range items {
		if nestedString(item, "id") == value {
			return item, nil
		}
		state := 0
		if closed, _ := item["closed"].(bool); closed {
			state = 1
		} else {
			options = append(options, displayName(kind, item))
		}
		if strings.ToLower(nestedString(item, nameKey)) == want {
			byName[state] = append(byName[state], item)
		} else if fallbackKey != "" && strings.ToLower(nestedString(item, fallbackKey)) == want {
			byFallback[state] = append(byFallback[state], item)
		}
	}
	singular := strings.TrimSuffix(kind, "s")
	for _, found := range [][]map[string]any{byName[0], byFallback[0], byName[1], byFallback[1]} {
		switch len(found) {
		case 0:
			continue
		case 1:
			return found[0], nil
		default:
			return nil, fmt.Errorf("%s %q matches %d entries; use an ID from get_board", singular, value, len(found))
		}
	}
	return nil, fmt.Errorf("%s %q not found; options: %s", singular, value, strings.Join(options, ", "))
}

func displayName(kind string, item map[string]any) string {
	switch kind {
	case "members":
		return nestedString(item, "fullName") + " (@" + nestedString(item, "username") + ")"
	case "labels":
		if name := nestedString(item, "name"); name != "" {
			return name
		}
		return nestedString(item, "color")
	default:
		return nestedString(item, "name")
	}
}

// namesOf turns IDs into names using a boardIndex collection.
func namesOf(index map[string]any, kind string, ids any) []string {
	byID := make(map[string]string)
	for _, item := range objectSlice(index[kind]) {
		name := displayName(kind, item)
		if kind == "members" {
			name = nestedString(item, "fullName")
		}
		byID[nestedString(item, "id")] = name
	}
	names := make([]string, 0)
	for _, id := range stringSlice(ids) {
		if name, ok := byID[id]; ok {
			names = append(names, name)
		}
	}
	return names
}

func stringSlice(value any) []string {
	items, _ := value.([]any)
	result := make([]string, 0, len(items))
	for _, item := range items {
		if text, ok := item.(string); ok {
			result = append(result, text)
		}
	}
	return result
}

func isTrelloID(value string) bool {
	if len(value) != 24 {
		return false
	}
	for _, char := range value {
		if !strings.ContainsRune("0123456789abcdef", char) {
			return false
		}
	}
	return true
}

// resolveList resolves the list argument and returns refs for its board, so
// labels and members on the same call resolve without another board fetch.
// A list name resolves on boardId, else on fallback when given, else on the
// active board.
func (s *Server) resolveList(ctx context.Context, args map[string]any, fallback *refs) (string, *refs, error) {
	value := text(args, "list")
	if value == "" {
		return "", nil, errors.New("list is required")
	}
	var boardRefs *refs
	switch {
	case isTrelloID(value):
		boardRefs = s.parentRefs("/lists/" + escape(value))
	case text(args, "boardId") == "" && fallback != nil:
		boardRefs = fallback
	default:
		boardRefs = s.boardRefs(args)
	}
	listID, err := boardRefs.id(ctx, "lists", value)
	if err != nil {
		return "", nil, err
	}
	return listID, boardRefs, s.ensureListAllowed(ctx, listID)
}

// listCards returns open cards in a list, on a board, or assigned to me, with
// list, label, and member names in place of IDs.
func (s *Server) listCards(ctx context.Context, args map[string]any) (any, error) {
	params := map[string]any{
		"fields": "shortLink,name,due,start,dueComplete,idBoard,idList,idLabels,idMembers",
		"filter": "open", "limit": integer(args, "limit", 100),
	}
	var (
		raw any
		err error
	)
	indexes := make(map[string]map[string]any)
	switch {
	case text(args, "list") != "":
		listID, boardRefs, resolveErr := s.resolveList(ctx, args, nil)
		if resolveErr != nil {
			return nil, resolveErr
		}
		if boardRefs.index != nil {
			indexes[nestedString(boardRefs.index, "id")] = boardRefs.index
		}
		raw, err = s.client.Do(ctx, http.MethodGet, "/lists/"+escape(listID)+"/cards", params, nil)
	case text(args, "boardId") != "":
		boardID, boardErr := s.board(ctx, args)
		if boardErr != nil {
			return nil, boardErr
		}
		raw, err = s.client.Do(ctx, http.MethodGet, "/boards/"+escape(boardID)+"/cards", params, nil)
	default:
		if raw, err = s.client.Do(ctx, http.MethodGet, "/members/me/cards", params, nil); err == nil {
			raw, err = s.filterByWorkspace(ctx, raw)
		}
	}
	if err != nil {
		return nil, err
	}
	cards := objectSlice(raw)
	for _, card := range cards {
		boardID := nestedString(card, "idBoard")
		if _, done := indexes[boardID]; !done {
			if indexes[boardID], err = s.boardIndex(ctx, boardID); err != nil {
				return nil, err
			}
		}
	}
	rows := make([]any, 0, len(cards))
	for _, card := range cards {
		index := indexes[nestedString(card, "idBoard")]
		row := map[string]any{"id": card["shortLink"], "name": card["name"]}
		if len(indexes) > 1 {
			row["board"] = index["name"]
		}
		if list := namesOf(index, "lists", []any{card["idList"]}); len(list) == 1 && text(args, "list") == "" {
			row["list"] = list[0]
		}
		copyPresent(row, card, "due", "start")
		if complete, _ := card["dueComplete"].(bool); complete {
			row["dueComplete"] = true
		}
		if labels := namesOf(index, "labels", card["idLabels"]); len(labels) > 0 {
			row["labels"] = labels
		}
		if members := namesOf(index, "members", card["idMembers"]); len(members) > 0 {
			row["members"] = members
		}
		rows = append(rows, row)
	}
	return rows, nil
}
