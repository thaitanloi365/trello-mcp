package clientconfig

import (
	"encoding/json"
	"strings"
	"testing"
)

const (
	testCommand = "/opt/trello/bin/trello-mcp"
	testConfig  = "/home/user/.trello-mcp/config.json"
)

func TestRenderCodex(t *testing.T) {
	got, err := Render(ClientCodex, Options{Command: testCommand, ConfigPath: testConfig})
	if err != nil {
		t.Fatal(err)
	}
	want := "[mcp_servers.trello]\n" +
		"command = \"/opt/trello/bin/trello-mcp\"\n" +
		"args = [\"serve\", \"--config\", \"/home/user/.trello-mcp/config.json\"]\n"
	if got != want {
		t.Fatalf("configuration:\n%s\nwant:\n%s", got, want)
	}
}

func TestRenderUsesDefaultConfigWithoutArgument(t *testing.T) {
	for _, client := range supportedClients {
		t.Run(client, func(t *testing.T) {
			output, err := Render(client, Options{Command: testCommand})
			if err != nil {
				t.Fatal(err)
			}
			if strings.Contains(output, "--config") || strings.Contains(output, testConfig) {
				t.Fatalf("default config path should not be included:\n%s", output)
			}
			if !strings.Contains(output, "serve") {
				t.Fatalf("serve argument is missing:\n%s", output)
			}
		})
	}
}

func TestRenderJSONClients(t *testing.T) {
	tests := []struct {
		client   string
		wantType string
	}{
		{client: ClientClaudeCode, wantType: "stdio"},
		{client: ClientClaudeDesktop},
		{client: ClientAntigravity},
	}
	for _, test := range tests {
		t.Run(test.client, func(t *testing.T) {
			output, err := Render(test.client, Options{Command: testCommand, ConfigPath: testConfig})
			if err != nil {
				t.Fatal(err)
			}
			var got jsonConfig
			if err := json.Unmarshal([]byte(output), &got); err != nil {
				t.Fatalf("invalid JSON: %v\n%s", err, output)
			}
			server, ok := got.MCPServers["trello"]
			if !ok {
				t.Fatal("trello server is missing")
			}
			if server.Type != test.wantType || server.Command != testCommand {
				t.Fatalf("unexpected server: %#v", server)
			}
			wantArgs := []string{"serve", "--config", testConfig}
			if strings.Join(server.Args, "\x00") != strings.Join(wantArgs, "\x00") {
				t.Fatalf("args = %#v, want %#v", server.Args, wantArgs)
			}
		})
	}
}

func TestRenderAliases(t *testing.T) {
	options := Options{Command: testCommand, ConfigPath: testConfig}
	for alias, canonical := range map[string]string{
		"claude":    ClientClaudeCode,
		"antigrav":  ClientAntigravity,
		"open-code": ClientOpenCode,
	} {
		got, err := Render(alias, options)
		if err != nil {
			t.Fatal(err)
		}
		want, err := Render(canonical, options)
		if err != nil {
			t.Fatal(err)
		}
		if got != want {
			t.Fatalf("alias %q differs from %q", alias, canonical)
		}
	}
}

func TestRenderOpenCode(t *testing.T) {
	output, err := Render(ClientOpenCode, Options{Command: testCommand, ConfigPath: testConfig})
	if err != nil {
		t.Fatal(err)
	}
	var got openCodeConfig
	if err := json.Unmarshal([]byte(output), &got); err != nil {
		t.Fatalf("invalid JSON: %v\n%s", err, output)
	}
	if got.Schema != "https://opencode.ai/config.json" {
		t.Fatalf("schema = %q", got.Schema)
	}
	server, ok := got.MCP["trello"]
	if !ok {
		t.Fatal("trello server is missing")
	}
	wantCommand := []string{testCommand, "serve", "--config", testConfig}
	if server.Type != "local" || !server.Enabled || strings.Join(server.Command, "\x00") != strings.Join(wantCommand, "\x00") {
		t.Fatalf("unexpected server: %#v", server)
	}
}

func TestRenderRejectsUnsupportedClient(t *testing.T) {
	_, err := Render("other", Options{Command: testCommand, ConfigPath: testConfig})
	if err == nil || !strings.Contains(err.Error(), "unsupported client") {
		t.Fatalf("error = %v", err)
	}
}
