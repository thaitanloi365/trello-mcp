package mcpserver

import (
	"path/filepath"
	"strings"
	"testing"

	"github.com/thaitanloi365/trello-mcp/internal/config"
	"github.com/thaitanloi365/trello-mcp/internal/trello"
)

func TestToolDefinitionsAreCompleteAndUnique(t *testing.T) {
	definitions := toolDefinitions()
	if got, want := len(definitions), 28; got != want {
		t.Fatalf("tool count = %d, want %d", got, want)
	}
	seen := make(map[string]bool)
	for _, definition := range definitions {
		if seen[definition.Name] {
			t.Fatalf("duplicate tool %q", definition.Name)
		}
		seen[definition.Name] = true
		schema := inputSchema(definition.Fields)
		if schema["type"] != "object" {
			t.Fatalf("tool %q schema is not an object", definition.Name)
		}
	}
	for _, requiredName := range []string{
		"get_card", "list_cards", "create_cards", "update_card", "add_comment",
		"create_checklist", "set_custom_field", "add_attachment", "get_health",
	} {
		if !seen[requiredName] {
			t.Errorf("missing tool %q", requiredName)
		}
	}
}

func TestNewServerRegistersAllSchemas(t *testing.T) {
	path := filepath.Join(t.TempDir(), "config.json")
	if err := config.Save(path, config.Default()); err != nil {
		t.Fatal(err)
	}
	manager, err := config.NewManager(path)
	if err != nil {
		t.Fatal(err)
	}
	if server := New(trello.NewClient(manager)); server == nil || server.MCP() == nil {
		t.Fatal("server was not created")
	}
}

func TestGetCardDefinitionIsLLMOptimized(t *testing.T) {
	t.Parallel()
	for _, definition := range toolDefinitions() {
		if definition.Name != "get_card" {
			continue
		}
		if !definition.ReadOnly {
			t.Error("get_card is not marked read-only")
		}
		properties := inputSchema(definition.Fields)["properties"].(map[string]any)
		if got := properties["commentsLimit"].(map[string]any)["default"]; got != 10 {
			t.Errorf("commentsLimit default = %#v, want 10", got)
		}
		if got := properties["delivery"].(map[string]any)["default"]; got != "auto" {
			t.Errorf("delivery default = %#v, want auto", got)
		}
		return
	}
	t.Fatal("get_card definition not found")
}

func TestCreateCardsSchemaTypesEachCard(t *testing.T) {
	t.Parallel()
	for _, definition := range toolDefinitions() {
		if definition.Name != "create_cards" {
			continue
		}
		if err := validateArguments(definition.Fields, map[string]any{
			"list": "Doing", "cards": []any{map[string]any{"title": "wrong key"}},
		}); err == nil || !strings.Contains(err.Error(), "cards[0]") {
			t.Fatalf("err = %v, want a cards[0] validation error", err)
		}
		return
	}
	t.Fatal("create_cards definition not found")
}

func TestEncodeResultKeepsAmpersands(t *testing.T) {
	t.Parallel()
	got, err := encodeResult(map[string]any{"name": "Terms & <Privacy>"})
	if err != nil {
		t.Fatal(err)
	}
	if want := `{"name":"Terms & <Privacy>"}`; got != want {
		t.Fatalf("encodeResult = %s, want %s", got, want)
	}
}
