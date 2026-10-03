package mcpserver

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"net/url"
	"path/filepath"
	"strings"
	"testing"

	"github.com/thaitanloi365/trello-mcp/internal/config"
	"github.com/thaitanloi365/trello-mcp/internal/trello"
)

func testServer(t *testing.T, allowed []string, handler http.HandlerFunc) (*Server, *config.Manager) {
	t.Helper()
	api := httptest.NewServer(handler)
	t.Cleanup(api.Close)
	cfg := config.Default()
	cfg.APIKey = "key"
	cfg.Token = "token"
	cfg.APIBaseURL = api.URL
	cfg.AllowedWorkspaceIDs = allowed
	path := filepath.Join(t.TempDir(), "config.json")
	if err := config.Save(path, cfg); err != nil {
		t.Fatal(err)
	}
	manager, err := config.NewManager(path)
	if err != nil {
		t.Fatal(err)
	}
	return New(trello.NewClient(manager)), manager
}

// boardFixture is a board index response shared by the name-resolution tests.
var boardFixture = map[string]any{
	"id": "board-1", "name": "Board",
	"lists": []any{
		map[string]any{"id": "list-todo", "name": "To Do"},
		map[string]any{"id": "list-done", "name": "Done"},
		map[string]any{"id": "list-old", "name": "Done", "closed": true},
	},
	"labels": []any{
		map[string]any{"id": "label-bug", "name": "Bug", "color": "red"},
		map[string]any{"id": "label-blue", "name": "", "color": "blue"},
	},
	"members": []any{map[string]any{"id": "member-joe", "fullName": "Joe Thai", "username": "joe"}},
}

func TestCreateCardsResolvesNames(t *testing.T) {
	var params url.Values
	server, _ := testServer(t, nil, func(w http.ResponseWriter, r *http.Request) {
		switch {
		case r.Method == http.MethodGet && r.URL.Path == "/boards/board-1":
			_ = json.NewEncoder(w).Encode(boardFixture)
		case r.Method == http.MethodPost && r.URL.Path == "/cards":
			params = r.URL.Query()
			_ = json.NewEncoder(w).Encode(map[string]any{"id": "card-1", "name": "New card", "badges": map[string]any{}})
		default:
			http.NotFound(w, r)
		}
	})
	result, err := server.handle(context.Background(), "create_cards", map[string]any{
		"list": "to do", "boardId": "board-1",
		"cards": []any{map[string]any{"name": "New card", "labels": []any{"bug", "blue"}, "members": []any{"@joe"}}},
	})
	if err != nil {
		t.Fatal(err)
	}
	if params.Get("idList") != "list-todo" || params.Get("idLabels") != "label-bug,label-blue" || params.Get("idMembers") != "member-joe" {
		t.Fatalf("create params = %v", params)
	}
	got, _ := json.Marshal(result)
	if string(got) != `[{"id":"card-1","name":"New card"}]` {
		t.Fatalf("result = %s", got)
	}
}

func TestUpdateCardMovesLabelsAndSetsReminder(t *testing.T) {
	var calls []string
	server, _ := testServer(t, nil, func(w http.ResponseWriter, r *http.Request) {
		calls = append(calls, r.Method+" "+r.URL.Path)
		switch {
		case r.Method == http.MethodGet && r.URL.Path == "/cards/card-1":
			_ = json.NewEncoder(w).Encode(map[string]any{"idBoard": "board-1", "idLabels": []any{"label-blue"}, "idMembers": []any{}})
		case r.Method == http.MethodGet && r.URL.Path == "/boards/board-1":
			_ = json.NewEncoder(w).Encode(boardFixture)
		case r.Method == http.MethodPut && r.URL.Path == "/cards/card-1":
			query := r.URL.Query()
			if query.Get("idList") != "list-done" || query.Get("dueReminder") != "1440" || query.Get("due") != "2026-11-01" ||
				query.Get("idLabels") != "label-blue,label-bug" {
				t.Errorf("update params = %v", query)
			}
			_ = json.NewEncoder(w).Encode(map[string]any{
				"id": "card-1", "name": "Card", "desc": "long description", "due": "2026-11-01T00:00:00.000Z",
				"dueReminder": 1440, "idList": "list-done", "shortUrl": "https://trello.com/c/abc", "url": "https://trello.com/c/abc/1-card",
			})
		default:
			http.NotFound(w, r)
		}
	})
	result, err := server.handle(context.Background(), "update_card", map[string]any{
		"cardId": "card-1", "list": "Done", "due": "2026-11-01", "dueReminder": float64(1440), "addLabels": []any{"Bug", "blue"},
	})
	if err != nil {
		t.Fatal(err)
	}
	got, _ := json.Marshal(result)
	want := `{"due":"2026-11-01T00:00:00.000Z","dueReminder":1440,"id":"card-1","idList":"list-done","name":"Card","shortUrl":"https://trello.com/c/abc"}`
	if string(got) != want {
		t.Fatalf("result = %s, want %s", got, want)
	}
	if want := "GET /cards/card-1,GET /boards/board-1,PUT /cards/card-1"; strings.Join(calls, ",") != want {
		t.Fatalf("calls = %v, want %s", calls, want)
	}
}

func TestUpdateCardUnknownNameWritesNothing(t *testing.T) {
	server, _ := testServer(t, nil, func(w http.ResponseWriter, r *http.Request) {
		switch {
		case r.Method == http.MethodGet && r.URL.Path == "/cards/card-1":
			_ = json.NewEncoder(w).Encode(map[string]any{"idBoard": "board-1"})
		case r.Method == http.MethodGet && r.URL.Path == "/boards/board-1":
			_ = json.NewEncoder(w).Encode(boardFixture)
		default:
			t.Errorf("unexpected write %s %s", r.Method, r.URL.Path)
			http.NotFound(w, r)
		}
	})
	_, err := server.handle(context.Background(), "update_card", map[string]any{
		"cardId": "card-1", "name": "Renamed", "addLabels": []any{"Feature"},
	})
	if err == nil || !strings.Contains(err.Error(), `label "Feature" not found; options: Bug, blue`) {
		t.Fatalf("err = %v", err)
	}
}

func TestListCardsUsesNames(t *testing.T) {
	server, _ := testServer(t, nil, func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/members/me/cards":
			_ = json.NewEncoder(w).Encode([]any{map[string]any{
				"shortLink": "abc", "name": "Card", "idBoard": "board-1", "idList": "list-done",
				"idLabels": []any{"label-bug"}, "idMembers": []any{"member-joe"}, "dueComplete": false, "due": nil,
			}})
		case "/boards/board-1":
			_ = json.NewEncoder(w).Encode(boardFixture)
		default:
			http.NotFound(w, r)
		}
	})
	result, err := server.handle(context.Background(), "list_cards", map[string]any{})
	if err != nil {
		t.Fatal(err)
	}
	got, _ := json.Marshal(result)
	if want := `[{"id":"abc","labels":["Bug"],"list":"Done","members":["Joe Thai"],"name":"Card"}]`; string(got) != want {
		t.Fatalf("result = %s, want %s", got, want)
	}
}

func TestCustomFieldBody(t *testing.T) {
	t.Parallel()
	list := map[string]any{"name": "Priority", "type": "list", "options": []any{
		map[string]any{"id": "opt-high", "value": map[string]any{"text": "High"}},
	}}
	tests := []struct {
		field map[string]any
		value string
		want  string
	}{
		{list, "high", `{"idValue":"opt-high"}`},
		{list, "", `{"idValue":"","value":""}`},
		{map[string]any{"type": "checkbox"}, "TRUE", `{"value":{"checked":"true"}}`},
		{map[string]any{"type": "number"}, "3.5", `{"value":{"number":"3.5"}}`},
		{map[string]any{"type": "date"}, "2026-11-01", `{"value":{"date":"2026-11-01"}}`},
	}
	for _, test := range tests {
		body, err := customFieldBody(test.field, test.value)
		if err != nil {
			t.Fatal(err)
		}
		if got, _ := json.Marshal(body); string(got) != test.want {
			t.Errorf("customFieldBody(%v, %q) = %s, want %s", test.field["type"], test.value, got, test.want)
		}
	}
	if _, err := customFieldBody(list, "Low"); err == nil || !strings.Contains(err.Error(), "options: High") {
		t.Errorf("unknown option err = %v", err)
	}
}

func TestSetActiveBoardPersistsSelection(t *testing.T) {
	server, manager := testServer(t, nil, func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet || r.URL.Path != "/boards/board-1" {
			http.NotFound(w, r)
			return
		}
		_ = json.NewEncoder(w).Encode(map[string]any{"id": "board-1", "name": "Board"})
	})
	if _, err := server.handle(context.Background(), "set_active", map[string]any{"boardId": "board-1"}); err != nil {
		t.Fatal(err)
	}
	if got := manager.File().DefaultBoardID; got != "board-1" {
		t.Fatalf("active board = %q", got)
	}
}

func TestWorkspaceRestrictionBlocksCardMutation(t *testing.T) {
	mutated := false
	server, _ := testServer(t, []string{"allowed-workspace"}, func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/cards/card-1":
			if r.Method == http.MethodPut {
				mutated = true
			}
			_ = json.NewEncoder(w).Encode(map[string]any{"id": "card-1", "idBoard": "board-1"})
		case "/boards/board-1":
			_ = json.NewEncoder(w).Encode(map[string]any{"id": "board-1", "idOrganization": "blocked-workspace"})
		default:
			http.NotFound(w, r)
		}
	})
	_, err := server.handle(context.Background(), "update_card", map[string]any{"cardId": "card-1", "closed": true})
	if err == nil {
		t.Fatal("expected workspace restriction error")
	}
	if mutated {
		t.Fatal("card was mutated despite workspace restriction")
	}
}

func TestCreateChecklistAddsItemsAndReturnsAck(t *testing.T) {
	var added []string
	server, _ := testServer(t, nil, func(w http.ResponseWriter, r *http.Request) {
		switch {
		case r.Method == http.MethodPost && r.URL.Path == "/cards/card-1/checklists":
			_ = json.NewEncoder(w).Encode(map[string]any{"id": "cl-1", "name": "Action Items", "idBoard": "board-1", "limits": map[string]any{}})
		case r.Method == http.MethodPost && r.URL.Path == "/checklists/cl-1/checkItems":
			name := r.URL.Query().Get("name")
			added = append(added, name)
			_ = json.NewEncoder(w).Encode(map[string]any{"id": "item-" + name, "name": name, "state": "incomplete", "idChecklist": "cl-1", "limits": map[string]any{}})
		default:
			http.NotFound(w, r)
		}
	})
	result, err := server.handle(context.Background(), "create_checklist", map[string]any{
		"cardId": "card-1", "name": "Action Items", "items": []any{"a", "b"},
	})
	if err != nil {
		t.Fatal(err)
	}
	if len(added) != 2 || added[0] != "a" || added[1] != "b" {
		t.Fatalf("items added = %v", added)
	}
	got, _ := json.Marshal(result)
	want := `{"checkItems":[{"id":"item-a","name":"a","state":"incomplete"},{"id":"item-b","name":"b","state":"incomplete"}],"id":"cl-1","name":"Action Items"}`
	if string(got) != want {
		t.Fatalf("result = %s, want %s", got, want)
	}
}

func TestLinkAttachments(t *testing.T) {
	t.Parallel()
	const link = "https://trello.com/1/cards/abc123/attachments/def456/download/RDS%20report.pdf"
	const parens = "https://trello.com/1/cards/abc123/attachments/def456/download/Invoice%20(1).pdf"
	tests := map[string]string{
		"- See " + link + ".":              "- See [RDS report.pdf](" + link + ").",
		link + " " + link:                  "[RDS report.pdf](" + link + ") [RDS report.pdf](" + link + ")",
		"(see " + link + ")":               "(see [RDS report.pdf](" + link + "))",
		parens:                             "[Invoice (1).pdf](https://trello.com/1/cards/abc123/attachments/def456/download/Invoice%20%281%29.pdf)",
		"[report](" + link + ")":           "[report](" + link + ")",
		"[see " + link + "](" + link + ")": "[see " + link + "](" + link + ")",
		"<" + link + ">":                   "<" + link + ">",
		"**" + link + "**":                 "**" + link + "**",
		`"` + link + `"`:                   `"` + link + `"`,
		"`run " + link + "`":               "`run " + link + "`",
		link + "," + link:                  link + "," + link,
		"https://github.com/x/y/1":         "https://github.com/x/y/1",
	}
	for in, want := range tests {
		if got := linkAttachments(in); got != want {
			t.Errorf("linkAttachments(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestCompactActions(t *testing.T) {
	t.Parallel()
	got, _ := json.Marshal(compactActions([]any{map[string]any{
		"id": "a-1", "type": "updateCard", "date": "2026-10-03T00:00:00Z", "limits": map[string]any{},
		"memberCreator": map[string]any{"fullName": "Joe", "avatarUrl": "https://example.com/a.png"},
		"data": map[string]any{
			"card":  map[string]any{"id": "c-1", "name": "Card", "shortLink": "abc", "due": "2026-11-01"},
			"board": map[string]any{"id": "b-1", "name": "Board"},
			"old":   map[string]any{"due": nil, "dueReminder": nil},
		},
	}}))
	want := `[{"by":"Joe","card":"Card","cardId":"abc","changed":{"due":{"from":null,"to":"2026-11-01"},"dueReminder":{"from":null,"to":null}},"date":"2026-10-03T00:00:00Z","id":"a-1","type":"updateCard"}]`
	if string(got) != want {
		t.Fatalf("compactActions = %s, want %s", got, want)
	}
}

func TestCopyCardUsesFullSourceIDAndSourceBoard(t *testing.T) {
	server, _ := testServer(t, nil, func(w http.ResponseWriter, r *http.Request) {
		switch {
		case r.Method == http.MethodGet && r.URL.Path == "/cards/abc12345":
			_ = json.NewEncoder(w).Encode(map[string]any{"id": "6a0000000000000000000001", "idBoard": "board-1"})
		case r.Method == http.MethodGet && r.URL.Path == "/boards/board-1":
			_ = json.NewEncoder(w).Encode(boardFixture)
		case r.Method == http.MethodPost && r.URL.Path == "/cards":
			query := r.URL.Query()
			if query.Get("idCardSource") != "6a0000000000000000000001" || query.Get("idList") != "list-done" {
				t.Errorf("copy params = %v", query)
			}
			_ = json.NewEncoder(w).Encode(map[string]any{"id": "card-2", "name": "Copy"})
		default:
			http.NotFound(w, r)
		}
	})
	if _, err := server.handle(context.Background(), "copy_card", map[string]any{"sourceCardId": "abc12345", "list": "Done"}); err != nil {
		t.Fatal(err)
	}
}

func TestMatchPrefersNamesAndOpenItems(t *testing.T) {
	t.Parallel()
	labels := []map[string]any{
		{"id": "label-named", "name": "Green", "color": "lime"},
		{"id": "label-unnamed", "name": "", "color": "green"},
	}
	if item, err := match(labels, "labels", "green"); err != nil || item["id"] != "label-named" {
		t.Errorf("match(green) = %v, %v; want the label named Green", item, err)
	}
	lists := []map[string]any{{"id": "list-q3", "name": "Q3", "closed": true}}
	if item, err := match(lists, "lists", "q3"); err != nil || item["id"] != "list-q3" {
		t.Errorf("match(archived q3) = %v, %v", item, err)
	}
	if _, err := match([]map[string]any{{"id": "list-inbox", "name": "@inbox"}}, "lists", "@inbox"); err != nil {
		t.Errorf("match(@inbox) = %v", err)
	}
}

func TestSetCustomFieldNeedsValueOrClear(t *testing.T) {
	server, _ := testServer(t, nil, func(w http.ResponseWriter, r *http.Request) {
		switch {
		case r.Method == http.MethodGet && r.URL.Path == "/cards/card-1":
			_ = json.NewEncoder(w).Encode(map[string]any{"idBoard": "board-1"})
		case r.Method == http.MethodGet && r.URL.Path == "/boards/board-1":
			_ = json.NewEncoder(w).Encode(map[string]any{"id": "board-1", "customFields": []any{map[string]any{"id": "field-1", "name": "Points", "type": "number"}}})
		default:
			http.NotFound(w, r)
		}
		t.Errorf("unexpected request %s %s", r.Method, r.URL.Path)
	})
	_, err := server.handle(context.Background(), "set_custom_field", map[string]any{"cardId": "card-1", "field": "points"})
	if err == nil || !strings.Contains(err.Error(), "clear: true") {
		t.Fatalf("err = %v, want a value-or-clear error", err)
	}
}

func TestUpdateCardRemovingLastLabelUsesDelete(t *testing.T) {
	var calls []string
	server, _ := testServer(t, nil, func(w http.ResponseWriter, r *http.Request) {
		calls = append(calls, r.Method+" "+r.URL.Path)
		switch {
		case r.Method == http.MethodGet && r.URL.Path == "/cards/card-1":
			_ = json.NewEncoder(w).Encode(map[string]any{"idBoard": "board-1", "idLabels": []any{"label-bug"}})
		case r.Method == http.MethodGet && r.URL.Path == "/boards/board-1":
			_ = json.NewEncoder(w).Encode(boardFixture)
		case r.Method == http.MethodDelete:
			_ = json.NewEncoder(w).Encode(map[string]any{"_value": nil})
		default:
			http.NotFound(w, r)
		}
	})
	result, err := server.handle(context.Background(), "update_card", map[string]any{
		"cardId": "card-1", "removeLabels": []any{"Bug", "blue"},
	})
	if err != nil {
		t.Fatal(err)
	}
	if want := "GET /cards/card-1,GET /boards/board-1,DELETE /cards/card-1/idLabels/label-bug"; strings.Join(calls, ",") != want {
		t.Fatalf("calls = %v, want %s", calls, want)
	}
	if nestedString(result, "id") != "card-1" {
		t.Fatalf("result = %v", result)
	}
}

func TestCompactActionsReadsTheUpdatedEntity(t *testing.T) {
	t.Parallel()
	got, _ := json.Marshal(compactActions([]any{map[string]any{
		"type": "updateCheckItem",
		"data": map[string]any{
			"card":      map[string]any{"name": "Card"},
			"checkItem": map[string]any{"name": "New text", "state": "incomplete"},
			"old":       map[string]any{"name": "Old text"},
		},
	}}))
	want := `[{"card":"Card","changed":{"name":{"from":"Old text","to":"New text"}},"checkItem":"New text","state":"incomplete","type":"updateCheckItem"}]`
	if string(got) != want {
		t.Fatalf("compactActions = %s, want %s", got, want)
	}
}

func TestSetActiveRejectsBoardWithoutSavingWorkspace(t *testing.T) {
	server, manager := testServer(t, []string{"workspace-1"}, func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/organizations/workspace-1":
			_ = json.NewEncoder(w).Encode(map[string]any{"displayName": "Team"})
		case "/boards/board-x":
			_ = json.NewEncoder(w).Encode(map[string]any{"id": "board-x", "idOrganization": "workspace-other"})
		default:
			http.NotFound(w, r)
		}
	})
	_, err := server.handle(context.Background(), "set_active", map[string]any{"workspaceId": "workspace-1", "boardId": "board-x"})
	if err == nil {
		t.Fatal("expected a workspace restriction error")
	}
	if got := manager.File().WorkspaceID; got != "" {
		t.Fatalf("workspace saved as %q despite the error", got)
	}
}
