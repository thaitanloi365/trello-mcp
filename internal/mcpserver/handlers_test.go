package mcpserver

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"path/filepath"
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

func TestHandleAddCardToList(t *testing.T) {
	var created bool
	server, _ := testServer(t, nil, func(w http.ResponseWriter, r *http.Request) {
		switch {
		case r.Method == http.MethodGet && r.URL.Path == "/lists/list-1":
			_ = json.NewEncoder(w).Encode(map[string]any{"id": "list-1", "idBoard": "board-1"})
		case r.Method == http.MethodPost && r.URL.Path == "/cards":
			created = true
			if got := r.URL.Query().Get("idList"); got != "list-1" {
				t.Errorf("idList = %q", got)
			}
			if got := r.URL.Query().Get("name"); got != "New card" {
				t.Errorf("name = %q", got)
			}
			_ = json.NewEncoder(w).Encode(map[string]any{"id": "card-1", "name": "New card"})
		default:
			http.NotFound(w, r)
		}
	})
	result, err := server.handle(context.Background(), "add_card_to_list", map[string]any{
		"listId": "list-1", "name": "New card", "description": "Description",
	})
	if err != nil {
		t.Fatal(err)
	}
	if !created || nestedString(result, "id") != "card-1" {
		t.Fatalf("card was not created: %#v", result)
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
	if _, err := server.handle(context.Background(), "set_active_board", map[string]any{"boardId": "board-1"}); err != nil {
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
	_, err := server.handle(context.Background(), "archive_card", map[string]any{"cardId": "card-1"})
	if err == nil {
		t.Fatal("expected workspace restriction error")
	}
	if mutated {
		t.Fatal("card was mutated despite workspace restriction")
	}
}
