// Package clientconfig renders MCP client configuration for trello-mcp.
package clientconfig

import (
	"encoding/json"
	"fmt"
	"strings"
)

const (
	ClientCodex         = "codex"
	ClientClaudeCode    = "claude-code"
	ClientClaudeDesktop = "claude-desktop"
	ClientAntigravity   = "antigravity"
	ClientOpenCode      = "opencode"
)

var supportedClients = []string{
	ClientCodex,
	ClientClaudeCode,
	ClientClaudeDesktop,
	ClientAntigravity,
	ClientOpenCode,
}

var projectClients = []string{
	ClientCodex,
	ClientClaudeCode,
	ClientAntigravity,
	ClientOpenCode,
}

// Options contains the local paths a client uses to start trello-mcp.
// ConfigPath is optional; when empty, trello-mcp resolves its default config
// path at startup.
type Options struct {
	Command    string
	ConfigPath string
}

type jsonConfig struct {
	MCPServers map[string]stdioServer `json:"mcpServers"`
}

type stdioServer struct {
	Type    string   `json:"type,omitempty"`
	Command string   `json:"command"`
	Args    []string `json:"args"`
}

type openCodeConfig struct {
	Schema string                    `json:"$schema"`
	MCP    map[string]openCodeServer `json:"mcp"`
}

type openCodeServer struct {
	Type    string   `json:"type"`
	Command []string `json:"command"`
	Enabled bool     `json:"enabled"`
}

// SupportedClients returns the canonical client names accepted by Render.
func SupportedClients() []string {
	return append([]string(nil), supportedClients...)
}

// ProjectClients returns the clients that support project-scoped MCP
// configuration.
func ProjectClients() []string {
	return append([]string(nil), projectClients...)
}

// Render produces a complete MCP configuration document for a supported
// client. Aliases "claude" and "antigrav" are accepted for convenience.
func Render(client string, options Options) (string, error) {
	client, err := normalizeClient(client)
	if err != nil {
		return "", err
	}
	if strings.TrimSpace(options.Command) == "" {
		return "", fmt.Errorf("server command is required")
	}

	args := []string{"serve"}
	if strings.TrimSpace(options.ConfigPath) != "" {
		args = append(args, "--config", options.ConfigPath)
	}
	if client == ClientCodex {
		return renderCodex(options.Command, args), nil
	}
	if client == ClientOpenCode {
		payload := openCodeConfig{
			Schema: "https://opencode.ai/config.json",
			MCP: map[string]openCodeServer{
				"trello": {
					Type:    "local",
					Command: append([]string{options.Command}, args...),
					Enabled: true,
				},
			},
		}
		return renderJSON(client, payload)
	}

	server := stdioServer{Command: options.Command, Args: args}
	if client == ClientClaudeCode {
		server.Type = "stdio"
	}
	payload := jsonConfig{MCPServers: map[string]stdioServer{"trello": server}}
	return renderJSON(client, payload)
}

func renderJSON(client string, payload any) (string, error) {
	data, err := json.MarshalIndent(payload, "", "  ")
	if err != nil {
		return "", fmt.Errorf("encode %s configuration: %w", client, err)
	}
	return string(data) + "\n", nil
}

func normalizeClient(client string) (string, error) {
	switch strings.ToLower(strings.TrimSpace(client)) {
	case ClientCodex:
		return ClientCodex, nil
	case "claude", ClientClaudeCode:
		return ClientClaudeCode, nil
	case ClientClaudeDesktop:
		return ClientClaudeDesktop, nil
	case "antigrav", ClientAntigravity:
		return ClientAntigravity, nil
	case "open-code", ClientOpenCode:
		return ClientOpenCode, nil
	default:
		return "", fmt.Errorf("unsupported client %q (choose %s)", client, strings.Join(supportedClients, ", "))
	}
}

func renderCodex(command string, args []string) string {
	return fmt.Sprintf(
		"[mcp_servers.trello]\ncommand = %s\nargs = [%s]\n",
		quote(command),
		quoteList(args),
	)
}

func quoteList(values []string) string {
	quoted := make([]string, len(values))
	for i, value := range values {
		quoted[i] = quote(value)
	}
	return strings.Join(quoted, ", ")
}

// JSON string quoting is compatible with TOML basic strings for local paths
// and command arguments while correctly escaping quotes and backslashes.
func quote(value string) string {
	data, _ := json.Marshal(value)
	return string(data)
}
