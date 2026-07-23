package clientconfig

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestProjectConfigPaths(t *testing.T) {
	tests := map[string]string{
		ClientCodex:       filepath.Join(".codex", "config.toml"),
		ClientClaudeCode:  ".mcp.json",
		ClientAntigravity: filepath.Join(".agents", "mcp_config.json"),
		ClientOpenCode:    "opencode.json",
	}
	for client, want := range tests {
		got, err := ProjectConfigPath(client)
		if err != nil {
			t.Fatal(err)
		}
		if got != want {
			t.Fatalf("ProjectConfigPath(%q) = %q, want %q", client, got, want)
		}
	}
	if _, err := ProjectConfigPath(ClientClaudeDesktop); err == nil {
		t.Fatal("Claude Desktop project scope should be rejected")
	}
}

func TestSetupProjectJSONClients(t *testing.T) {
	options := Options{Command: testCommand, ConfigPath: testConfig}
	for _, client := range []string{ClientClaudeCode, ClientAntigravity, ClientOpenCode} {
		t.Run(client, func(t *testing.T) {
			root := t.TempDir()
			target, err := SetupProject(client, root, options)
			if err != nil {
				t.Fatal(err)
			}
			data, err := os.ReadFile(target)
			if err != nil {
				t.Fatal(err)
			}
			var document map[string]any
			if err := json.Unmarshal(data, &document); err != nil {
				t.Fatalf("invalid JSON: %v\n%s", err, data)
			}
			containerKey := "mcpServers"
			if client == ClientOpenCode {
				containerKey = "mcp"
			}
			container, ok := document[containerKey].(map[string]any)
			if !ok || container["trello"] == nil {
				t.Fatalf("missing trello server in %s: %#v", containerKey, document)
			}
		})
	}
}

func TestSetupProjectPreservesExistingJSON(t *testing.T) {
	root := t.TempDir()
	target := filepath.Join(root, ".mcp.json")
	existing := `{"otherSetting":true,"mcpServers":{"other":{"command":"other"}}}`
	if err := os.WriteFile(target, []byte(existing), 0o640); err != nil {
		t.Fatal(err)
	}
	if _, err := SetupProject(ClientClaudeCode, root, Options{Command: testCommand, ConfigPath: testConfig}); err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(target)
	if err != nil {
		t.Fatal(err)
	}
	var document map[string]any
	if err := json.Unmarshal(data, &document); err != nil {
		t.Fatal(err)
	}
	if document["otherSetting"] != true {
		t.Fatal("existing top-level setting was removed")
	}
	servers := document["mcpServers"].(map[string]any)
	if servers["other"] == nil || servers["trello"] == nil {
		t.Fatalf("existing or Trello server missing: %#v", servers)
	}
	info, err := os.Stat(target)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm() != 0o640 {
		t.Fatalf("mode = %o, want 640", info.Mode().Perm())
	}
}

func TestSetupProjectUpdatesManagedCodexBlock(t *testing.T) {
	root := t.TempDir()
	target := filepath.Join(root, ".codex", "config.toml")
	if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(target, []byte("model = \"example\"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	options := Options{Command: testCommand, ConfigPath: testConfig}
	if _, err := SetupProject(ClientCodex, root, options); err != nil {
		t.Fatal(err)
	}
	options.Command = "/new/trello-mcp"
	if _, err := SetupProject(ClientCodex, root, options); err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(target)
	if err != nil {
		t.Fatal(err)
	}
	content := string(data)
	if !strings.Contains(content, "model = \"example\"") || !strings.Contains(content, options.Command) {
		t.Fatalf("existing setting or updated command missing:\n%s", content)
	}
	if strings.Count(content, "[mcp_servers.trello]") != 1 || strings.Count(content, codexManagedBegin) != 1 {
		t.Fatalf("managed section was duplicated:\n%s", content)
	}
}

func TestSetupProjectRejectsUnmanagedCodexSection(t *testing.T) {
	root := t.TempDir()
	target := filepath.Join(root, ".codex", "config.toml")
	if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(target, []byte("model = \"example\"\n\n[mcp_servers.trello]\ncommand = \"old\"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	_, err := SetupProject(ClientCodex, root, Options{Command: testCommand, ConfigPath: testConfig})
	if err == nil || !strings.Contains(err.Error(), "un-managed") {
		t.Fatalf("error = %v", err)
	}
	if !strings.Contains(err.Error(), target+":3:") {
		t.Fatalf("error does not contain clickable config location %s:3: %v", target, err)
	}
}
