package main

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/thaitanloi365/trello-mcp/internal/config"
	"github.com/thaitanloi365/trello-mcp/internal/mcpserver"
)

func TestCobraHelp(t *testing.T) {
	var stdout, stderr bytes.Buffer
	if err := run([]string{"--help"}, &stdout, &stderr); err != nil {
		t.Fatal(err)
	}
	for _, expected := range []string{"Available Commands:", "client-config", "setup", "completion", "--config string"} {
		if !strings.Contains(stdout.String(), expected) {
			t.Fatalf("help does not contain %q:\n%s", expected, stdout.String())
		}
	}
	if stderr.Len() != 0 {
		t.Fatalf("unexpected stderr: %s", stderr.String())
	}
}

func TestClientConfigCommand(t *testing.T) {
	commandPath := filepath.Join(t.TempDir(), "bin", "trello-mcp")
	configPath := filepath.Join(t.TempDir(), "config.json")

	t.Run("codex", func(t *testing.T) {
		var stdout, stderr bytes.Buffer
		err := run([]string{
			"--config", configPath,
			"client-config", "codex",
			"--command", commandPath,
		}, &stdout, &stderr)
		if err != nil {
			t.Fatalf("run: %v; stderr=%s", err, stderr.String())
		}
		for _, expected := range []string{
			"[mcp_servers.trello]",
			"command = \"" + commandPath + "\"",
			"\"--config\", \"" + configPath + "\"",
		} {
			if !strings.Contains(stdout.String(), expected) {
				t.Fatalf("configuration does not contain %q:\n%s", expected, stdout.String())
			}
		}
	})

	for _, client := range []string{"claude-code", "claude-desktop", "antigravity"} {
		t.Run(client, func(t *testing.T) {
			var stdout, stderr bytes.Buffer
			err := run([]string{
				"client-config", client,
				"--command", commandPath,
				"--config", configPath,
			}, &stdout, &stderr)
			if err != nil {
				t.Fatalf("run: %v; stderr=%s", err, stderr.String())
			}
			var document struct {
				MCPServers map[string]struct {
					Type    string   `json:"type"`
					Command string   `json:"command"`
					Args    []string `json:"args"`
				} `json:"mcpServers"`
			}
			if err := json.Unmarshal(stdout.Bytes(), &document); err != nil {
				t.Fatalf("invalid JSON: %v\n%s", err, stdout.String())
			}
			server := document.MCPServers["trello"]
			if server.Command != commandPath || len(server.Args) != 3 || server.Args[2] != configPath {
				t.Fatalf("unexpected server configuration: %#v", server)
			}
			if client == "claude-code" && server.Type != "stdio" {
				t.Fatalf("Claude Code type = %q, want stdio", server.Type)
			}
			if client != "claude-code" && server.Type != "" {
				t.Fatalf("%s type = %q, want empty", client, server.Type)
			}
		})
	}

	t.Run("opencode", func(t *testing.T) {
		var stdout, stderr bytes.Buffer
		err := run([]string{
			"client-config", "opencode",
			"--command", commandPath,
			"--config", configPath,
		}, &stdout, &stderr)
		if err != nil {
			t.Fatalf("run: %v; stderr=%s", err, stderr.String())
		}
		var document struct {
			Schema string `json:"$schema"`
			MCP    map[string]struct {
				Type    string   `json:"type"`
				Command []string `json:"command"`
				Enabled bool     `json:"enabled"`
			} `json:"mcp"`
		}
		if err := json.Unmarshal(stdout.Bytes(), &document); err != nil {
			t.Fatalf("invalid JSON: %v\n%s", err, stdout.String())
		}
		server := document.MCP["trello"]
		if document.Schema != "https://opencode.ai/config.json" || server.Type != "local" || !server.Enabled {
			t.Fatalf("unexpected OpenCode configuration: %#v", document)
		}
		wantCommand := []string{commandPath, "serve", "--config", configPath}
		if strings.Join(server.Command, "\x00") != strings.Join(wantCommand, "\x00") {
			t.Fatalf("command = %#v, want %#v", server.Command, wantCommand)
		}
	})
}

func TestClientConfigCommandUsesServerDefaultConfig(t *testing.T) {
	commandPath := filepath.Join(t.TempDir(), "bin", "trello-mcp")
	var stdout, stderr bytes.Buffer
	if err := run([]string{
		"client-config", "codex",
		"--command", commandPath,
	}, &stdout, &stderr); err != nil {
		t.Fatalf("run: %v; stderr=%s", err, stderr.String())
	}
	if strings.Contains(stdout.String(), "--config") || strings.Contains(stdout.String(), ".trello-mcp") {
		t.Fatalf("default config path should not be included:\n%s", stdout.String())
	}
	if !strings.Contains(stdout.String(), `args = ["serve"]`) {
		t.Fatalf("unexpected configuration:\n%s", stdout.String())
	}
}

func TestSetupProjectCommand(t *testing.T) {
	projectDir := t.TempDir()
	commandPath := filepath.Join(t.TempDir(), "bin", "trello-mcp")
	configPath := filepath.Join(t.TempDir(), "config.json")
	var stdout, stderr bytes.Buffer
	err := run([]string{
		"setup", "opencode",
		"--scope", "project",
		"--project-dir", projectDir,
		"--command", commandPath,
		"--config", configPath,
	}, &stdout, &stderr)
	if err != nil {
		t.Fatalf("run: %v; stderr=%s", err, stderr.String())
	}
	target := filepath.Join(projectDir, "opencode.json")
	if got := strings.TrimSpace(stdout.String()); got != target {
		t.Fatalf("setup output = %q, want %q", got, target)
	}
	data, err := os.ReadFile(target)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(data), "\"type\": \"local\"") || !strings.Contains(string(data), commandPath) {
		t.Fatalf("unexpected OpenCode project config:\n%s", data)
	}
}

func TestSetupAllProjectClientsCommand(t *testing.T) {
	projectDir := t.TempDir()
	commandPath := filepath.Join(t.TempDir(), "bin", "trello-mcp")
	configPath := filepath.Join(t.TempDir(), "config.json")
	var stdout, stderr bytes.Buffer
	err := run([]string{
		"setup", "--all",
		"--scope", "project",
		"--project-dir", projectDir,
		"--command", commandPath,
		"--config", configPath,
	}, &stdout, &stderr)
	if err != nil {
		t.Fatalf("run: %v; stderr=%s", err, stderr.String())
	}

	relativeTargets := []string{
		filepath.Join(".codex", "config.toml"),
		".mcp.json",
		filepath.Join(".agents", "mcp_config.json"),
		"opencode.json",
	}
	wantTargets := make([]string, len(relativeTargets))
	for index, relative := range relativeTargets {
		target := filepath.Join(projectDir, relative)
		wantTargets[index] = target
		data, err := os.ReadFile(target)
		if err != nil {
			t.Fatalf("read %s: %v", target, err)
		}
		if !strings.Contains(string(data), commandPath) {
			t.Fatalf("%s does not contain command %q:\n%s", target, commandPath, data)
		}
	}
	gotTargets := strings.Fields(stdout.String())
	if strings.Join(gotTargets, "\x00") != strings.Join(wantTargets, "\x00") {
		t.Fatalf("setup output = %#v, want %#v", gotTargets, wantTargets)
	}
}

func TestSetupAllRejectsClientArgument(t *testing.T) {
	var stdout, stderr bytes.Buffer
	err := run([]string{"setup", "codex", "--all"}, &stdout, &stderr)
	if err == nil || !strings.Contains(err.Error(), "--all cannot be combined with a client") {
		t.Fatalf("error = %v, want --all conflict", err)
	}
}

func TestVersionForms(t *testing.T) {
	for _, args := range [][]string{{"version"}, {"--version"}, {"-version"}} {
		t.Run(strings.Join(args, "_"), func(t *testing.T) {
			var stdout, stderr bytes.Buffer
			if err := run(args, &stdout, &stderr); err != nil {
				t.Fatal(err)
			}
			if got, want := stdout.String(), mcpserver.Version+"\n"; got != want {
				t.Fatalf("version output = %q, want %q", got, want)
			}
			if stderr.Len() != 0 {
				t.Fatalf("unexpected stderr: %s", stderr.String())
			}
		})
	}
}

func TestPersistentConfigFlagBeforeSubcommands(t *testing.T) {
	path := filepath.Join(t.TempDir(), "config.json")
	var stdout, stderr bytes.Buffer
	if err := run([]string{"--config", path, "config", "init"}, &stdout, &stderr); err != nil {
		t.Fatalf("run: %v; stderr=%s", err, stderr.String())
	}
	if got := strings.TrimSpace(stdout.String()); got != path {
		t.Fatalf("config path = %q, want %q", got, path)
	}
	if _, err := config.Load(path); err != nil {
		t.Fatal(err)
	}
}

func TestConfigSetPersistsAllValues(t *testing.T) {
	path := filepath.Join(t.TempDir(), "config.json")
	var stdout, stderr bytes.Buffer
	err := run([]string{
		"config", "set", "--config", path,
		"--api-key", "api-key", "--token", "api-token",
		"--board-id", "board", "--workspace-id", "workspace",
		"--allowed-workspaces", "workspace,second",
		"--api-base-url", "https://example.test/1/", "--timeout-seconds", "45", "--max-upload-bytes", "2048",
	}, &stdout, &stderr)
	if err != nil {
		t.Fatalf("run: %v; stderr=%s", err, stderr.String())
	}
	cfg, err := config.Load(path)
	if err != nil {
		t.Fatal(err)
	}
	if cfg.APIKey != "api-key" || cfg.Token != "api-token" || cfg.DefaultBoardID != "board" || cfg.WorkspaceID != "workspace" {
		t.Fatalf("unexpected config: %#v", cfg)
	}
	if cfg.APIBaseURL != "https://example.test/1" || cfg.RequestTimeoutSecs != 45 || cfg.MaxUploadBytes != 2048 {
		t.Fatalf("unexpected settings: %#v", cfg)
	}
}

func TestConfigShowMasksSecrets(t *testing.T) {
	path := filepath.Join(t.TempDir(), "config.json")
	cfg := config.Default()
	cfg.APIKey = "abcdefghijk"
	cfg.Token = "super-secret-token"
	if err := config.Save(path, cfg); err != nil {
		t.Fatal(err)
	}
	var stdout, stderr bytes.Buffer
	if err := run([]string{"config", "show", "--config", path}, &stdout, &stderr); err != nil {
		t.Fatal(err)
	}
	for _, secret := range []string{cfg.APIKey, cfg.Token} {
		if strings.Contains(stdout.String(), secret) {
			t.Fatalf("output leaked %q: %s", secret, stdout.String())
		}
	}
}
