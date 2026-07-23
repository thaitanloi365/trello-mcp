package mcpserver

import (
	"path/filepath"
	"testing"

	"github.com/thaitanloi365/trello-mcp/internal/config"
	"github.com/thaitanloi365/trello-mcp/internal/trello"
)

func TestToolDefinitionsAreCompleteAndUnique(t *testing.T) {
	definitions := toolDefinitions()
	if got, want := len(definitions), 57; got != want {
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
		"get_card", "add_card_to_list", "list_boards", "set_active_board",
		"create_checklist", "update_card_custom_field", "download_attachment", "get_health",
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
		if definition.OutputSchema == nil {
			t.Fatal("get_card output schema is nil")
		}
		schema := inputSchema(definition.Fields)
		properties := schema["properties"].(map[string]any)
		for _, name := range []string{"cardId", "detailLevel", "commentsLimit", "format", "delivery"} {
			if _, ok := properties[name]; !ok {
				t.Errorf("get_card schema is missing %q", name)
			}
		}
		commentsLimit := properties["commentsLimit"].(map[string]any)
		if got := commentsLimit["default"]; got != 10 {
			t.Errorf("commentsLimit default = %#v, want 10", got)
		}
		format := properties["format"].(map[string]any)
		if got := format["default"]; got != "markdown" {
			t.Errorf("format default = %#v, want markdown", got)
		}
		delivery := properties["delivery"].(map[string]any)
		if got := delivery["default"]; got != "auto" {
			t.Errorf("delivery default = %#v, want auto", got)
		}
		return
	}
	t.Fatal("get_card definition not found")
}
