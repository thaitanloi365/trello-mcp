package mcpserver

import (
	"context"
	"encoding/json"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestNormalizeCardRef(t *testing.T) {
	t.Parallel()
	tests := []struct {
		name string
		in   string
		want string
	}{
		{name: "full ID", in: "6a595e613293292bd6ef5a4c", want: "6a595e613293292bd6ef5a4c"},
		{name: "short link", in: "7yNeeFiE", want: "7yNeeFiE"},
		{name: "full URL", in: "https://trello.com/c/7yNeeFiE/430-example?menu=filter#comment-1", want: "7yNeeFiE"},
		{name: "URL without scheme", in: "trello.com/c/7yNeeFiE/430-example", want: "7yNeeFiE"},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			got, err := normalizeCardRef(test.in)
			if err != nil {
				t.Fatal(err)
			}
			if got != test.want {
				t.Fatalf("normalizeCardRef(%q) = %q, want %q", test.in, got, test.want)
			}
		})
	}
}

func TestNormalizeCardRefRejectsNonCardURLs(t *testing.T) {
	t.Parallel()
	for _, value := range []string{
		"https://example.com/c/7yNeeFiE",
		"https://trello.com/b/board-id",
		"card/id",
	} {
		if _, err := normalizeCardRef(value); err == nil {
			t.Errorf("normalizeCardRef(%q) unexpectedly succeeded", value)
		}
	}
}

func TestGetCardCompactUsesOneDirectRequest(t *testing.T) {
	requests := 0
	server, _ := testServer(t, nil, func(w http.ResponseWriter, r *http.Request) {
		requests++
		if r.URL.Path == "/boards/board-1/customFields" {
			_ = json.NewEncoder(w).Encode([]any{map[string]any{"id": "field-1", "name": "Priority", "type": "text"}})
			return
		}
		if r.Method != http.MethodGet || r.URL.Path != "/cards/7yNeeFiE" {
			http.NotFound(w, r)
			return
		}
		query := r.URL.Query()
		if got := query.Get("fields"); got == "" || got == "all" {
			t.Errorf("fields = %q, want an explicit compact field set", got)
		}
		if got := query.Get("actions"); got != "commentCard" {
			t.Errorf("actions = %q, want commentCard", got)
		}
		if got := query.Get("actions_limit"); got != "10" {
			t.Errorf("actions_limit = %q, want 10", got)
		}
		if got := query.Get("attachment_fields"); got == "all" {
			t.Error("attachment_fields unexpectedly requests all fields")
		}
		_ = json.NewEncoder(w).Encode(map[string]any{
			"id":        "card-1",
			"idBoard":   "board-1",
			"idList":    "list-1",
			"shortLink": "7yNeeFiE",
			"name":      "Card name",
			"desc":      "Card description",
			"closed":    false,
			"list":      map[string]any{"id": "list-1", "name": "Doing"},
			"url":       "https://trello.com/c/7yNeeFiE/card-name",
			"cover":     map[string]any{"scaled": []any{"large preview data"}},
			"labels": []any{
				map[string]any{"id": "label-1", "name": "Urgent", "color": "red", "uses": 99},
			},
			"attachments": []any{
				map[string]any{
					"id": "attachment-1", "name": "spec.pdf", "url": "https://example.test/spec.pdf",
					"mimeType": "application/pdf", "previews": []any{"large preview data"},
				},
			},
			"members": []any{
				map[string]any{"id": "member-1", "fullName": "Test User", "username": "test", "avatarUrl": "large"},
			},
			"checklists": []any{
				map[string]any{
					"id": "checklist-1", "name": "Acceptance Criteria", "idBoard": "board-1",
					"checkItems": []any{
						map[string]any{"id": "item-1", "name": "Works", "state": "incomplete", "idChecklist": "checklist-1"},
					},
				},
			},
			"customFieldItems": []any{
				map[string]any{"id": "custom-1", "idCustomField": "field-1", "value": map[string]any{"text": "High"}, "extra": "drop"},
			},
			"actions": []any{
				map[string]any{
					"id": "action-1", "idMemberCreator": "member-1", "date": "2026-07-23T00:00:00.000Z",
					"data": map[string]any{
						"text":  "A useful comment",
						"card":  map[string]any{"id": "card-1", "name": "duplicated card"},
						"board": map[string]any{"id": "board-1", "name": "duplicated board"},
					},
					"memberCreator": map[string]any{
						"id": "member-1", "fullName": "Test User", "username": "test", "avatarUrl": "large",
					},
					"display": map[string]any{"translationKey": "large duplicated display metadata"},
				},
			},
		})
	})

	result, err := server.handle(context.Background(), "get_card", map[string]any{
		"cardId": "https://trello.com/c/7yNeeFiE/430-card-name",
	})
	if err != nil {
		t.Fatal(err)
	}
	if requests != 2 {
		t.Fatalf("Trello request count = %d, want 2 (card, custom field definitions)", requests)
	}
	markdown, ok := result.(rawTextResult)
	if !ok {
		t.Fatalf("result type = %T, want rawTextResult", result)
	}
	for _, want := range []string{
		"# Card name", "- List: Doing", "- Labels: Urgent", "- Members: Test User (@test)", "- Priority: High",
		"## Acceptance Criteria (checklist checklist-1)", "- [ ] Works (item item-1)",
		"- [spec.pdf](https://example.test/spec.pdf) (attachment attachment-1)",
		"### Test User (@test), 2026-07-23T00:00:00.000Z (comment action-1)", "A useful comment",
	} {
		if !strings.Contains(string(markdown), want) {
			t.Errorf("Markdown is missing %q:\n%s", want, markdown)
		}
	}
	for _, dropped := range []string{"large preview data", "duplicated", "avatar"} {
		if strings.Contains(string(markdown), dropped) {
			t.Errorf("Markdown contains %q", dropped)
		}
	}
}

func TestGetCardWithWorkspaceRestrictionKeepsAuthorizationPreflight(t *testing.T) {
	requests := make([]string, 0)
	server, _ := testServer(t, []string{"workspace-1"}, func(w http.ResponseWriter, r *http.Request) {
		requests = append(requests, r.URL.Path+"?fields="+r.URL.Query().Get("fields"))
		switch {
		case r.URL.Path == "/cards/card-1" && r.URL.Query().Get("fields") == "id,idBoard":
			_ = json.NewEncoder(w).Encode(map[string]any{"id": "card-1", "idBoard": "board-1"})
		case r.URL.Path == "/boards/board-1":
			_ = json.NewEncoder(w).Encode(map[string]any{"id": "board-1", "idOrganization": "workspace-1"})
		case r.URL.Path == "/cards/card-1":
			_ = json.NewEncoder(w).Encode(map[string]any{
				"id": "card-1", "idBoard": "board-1", "name": "Allowed card", "actions": []any{},
			})
		default:
			http.NotFound(w, r)
		}
	})

	if _, err := server.handle(context.Background(), "get_card", map[string]any{"cardId": "card-1"}); err != nil {
		t.Fatal(err)
	}
	if len(requests) != 3 {
		t.Fatalf("Trello request count = %d, want 3: %v", len(requests), requests)
	}
	if strings.Contains(strings.Join(requests, "\n"), "/actions") {
		t.Fatalf("comments were fetched separately: %v", requests)
	}
}

func TestGetCardDefaultsToMarkdownAutoInline(t *testing.T) {
	server, _ := testServer(t, nil, func(w http.ResponseWriter, _ *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]any{
			"id": "card-1", "name": "Markdown card", "desc": "Useful description",
			"url": "https://trello.com/c/markdown", "actions": []any{},
		})
	})

	result, err := server.handle(context.Background(), "get_card", map[string]any{
		"cardId": "card-1",
	})
	if err != nil {
		t.Fatal(err)
	}
	markdown, ok := result.(rawTextResult)
	if !ok {
		t.Fatalf("result type = %T, want rawTextResult", result)
	}
	if !strings.Contains(string(markdown), "# Markdown card") || strings.Contains(string(markdown), `"name"`) {
		t.Fatalf("unexpected Markdown output: %s", markdown)
	}
}

func TestDeliverCardModes(t *testing.T) {
	card := "# Delivery card\n\nUseful details"

	t.Run("inline", func(t *testing.T) {
		result, err := deliverCard("card-1", card, getCardOptions{
			delivery: "inline", fileThreshold: 1,
		})
		if err != nil {
			t.Fatal(err)
		}
		if _, ok := result.(rawTextResult); !ok {
			t.Fatalf("result type = %T, want rawTextResult", result)
		}
	})

	t.Run("auto inline below threshold", func(t *testing.T) {
		result, err := deliverCard("card-1", card, getCardOptions{
			delivery: "auto", fileThreshold: 1 << 20,
		})
		if err != nil {
			t.Fatal(err)
		}
		if _, ok := result.(rawTextResult); !ok {
			t.Fatalf("result type = %T, want rawTextResult", result)
		}
	})

	t.Run("auto file above threshold", func(t *testing.T) {
		outputDir := filepath.Join(t.TempDir(), "outputs")
		result, err := deliverCard("card-1", card, getCardOptions{
			delivery: "auto", fileThreshold: 1, outputDir: outputDir,
		})
		if err != nil {
			t.Fatal(err)
		}
		metadata, ok := result.(map[string]any)
		if !ok {
			t.Fatalf("result type = %T, want map[string]any", result)
		}
		path, _ := metadata["path"].(string)
		if filepath.Dir(path) != outputDir || filepath.Ext(path) != ".md" {
			t.Fatalf("output path = %q", path)
		}
		data, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		if !strings.Contains(string(data), "# Delivery card") {
			t.Fatalf("unexpected file content: %s", data)
		}
		if info, err := os.Stat(path); err != nil {
			t.Fatal(err)
		} else if got := info.Mode().Perm(); got != 0o600 {
			t.Errorf("file mode = %04o, want 0600", got)
		}
		if info, err := os.Stat(outputDir); err != nil {
			t.Fatal(err)
		} else if got := info.Mode().Perm(); got != 0o700 {
			t.Errorf("directory mode = %04o, want 0700", got)
		}
	})
}

func TestGetCardRejectsInvalidOptionsBeforeCallingTrello(t *testing.T) {
	requests := 0
	server, _ := testServer(t, nil, func(w http.ResponseWriter, _ *http.Request) {
		requests++
		http.Error(w, "unexpected request", http.StatusInternalServerError)
	})
	_, err := server.handle(context.Background(), "get_card", map[string]any{
		"cardId": "card-1", "commentsLimit": float64(101),
	})
	if err == nil {
		t.Fatal("expected commentsLimit validation error")
	}
	if requests != 0 {
		t.Fatalf("Trello request count = %d, want 0", requests)
	}

	_, err = server.handle(context.Background(), "get_card", map[string]any{
		"cardId": "card-1", "delivery": "elsewhere",
	})
	if err == nil {
		t.Fatal("expected delivery validation error")
	}
	if requests != 0 {
		t.Fatalf("Trello request count = %d, want 0", requests)
	}
}

func TestGetCardNamesChecklistAssigneesFromTheBoard(t *testing.T) {
	server, _ := testServer(t, nil, func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/cards/card-1":
			_ = json.NewEncoder(w).Encode(map[string]any{
				"idBoard": "board-1", "name": "Card",
				"checklists": []any{map[string]any{"id": "cl-1", "name": "Tasks", "checkItems": []any{
					map[string]any{"id": "item-1", "name": "Ship", "state": "incomplete", "idMember": "member-anna"},
					map[string]any{"id": "item-2", "name": "Test", "state": "incomplete", "idMember": "member-gone"},
				}}},
			})
		case "/boards/board-1/members":
			_ = json.NewEncoder(w).Encode([]any{map[string]any{"id": "member-anna", "fullName": "Anna"}})
		default:
			http.NotFound(w, r)
		}
	})
	result, err := server.handle(context.Background(), "get_card", map[string]any{"cardId": "card-1"})
	if err != nil {
		t.Fatal(err)
	}
	markdown := string(result.(rawTextResult))
	for _, want := range []string{"- [ ] Ship (item item-1, assigned Anna)", "- [ ] Test (item item-2, assigned member member-gone)"} {
		if !strings.Contains(markdown, want) {
			t.Errorf("Markdown is missing %q:\n%s", want, markdown)
		}
	}
}
