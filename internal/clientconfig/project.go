package clientconfig

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
)

const (
	codexManagedBegin = "# BEGIN trello-mcp managed configuration"
	codexManagedEnd   = "# END trello-mcp managed configuration"
)

// ProjectConfigPath returns the client-recognized config path relative to a
// project root. Claude Desktop does not support project-scoped MCP config.
func ProjectConfigPath(client string) (string, error) {
	client, err := normalizeClient(client)
	if err != nil {
		return "", err
	}
	switch client {
	case ClientCodex:
		return filepath.Join(".codex", "config.toml"), nil
	case ClientClaudeCode:
		return ".mcp.json", nil
	case ClientAntigravity:
		return filepath.Join(".agents", "mcp_config.json"), nil
	case ClientOpenCode:
		return "opencode.json", nil
	case ClientClaudeDesktop:
		return "", fmt.Errorf("%s does not support project-scoped MCP configuration; use client-config %s and merge it into the desktop config", ClientClaudeDesktop, ClientClaudeDesktop)
	default:
		return "", fmt.Errorf("project configuration is not supported for %s", client)
	}
}

// SetupProject writes or updates trello-mcp in a client's project-scoped MCP
// configuration without replacing unrelated client settings or MCP servers.
func SetupProject(client, projectDir string, options Options) (string, error) {
	client, err := normalizeClient(client)
	if err != nil {
		return "", err
	}
	relativePath, err := ProjectConfigPath(client)
	if err != nil {
		return "", err
	}
	if strings.TrimSpace(projectDir) == "" {
		projectDir = "."
	}
	root, err := filepath.Abs(projectDir)
	if err != nil {
		return "", fmt.Errorf("resolve project directory: %w", err)
	}
	target := filepath.Join(root, relativePath)
	generated, err := Render(client, options)
	if err != nil {
		return "", err
	}
	existing, err := os.ReadFile(target)
	if err != nil && !errors.Is(err, fs.ErrNotExist) {
		return "", fmt.Errorf("read project config %s: %w", target, err)
	}

	var output []byte
	if client == ClientCodex {
		output, err = mergeCodex(existing, generated, target)
	} else {
		output, err = mergeJSON(existing, generated, client)
	}
	if err != nil {
		return "", err
	}
	if err := writeAtomic(target, output); err != nil {
		return "", err
	}
	return target, nil
}

func mergeCodex(existing []byte, generated, target string) ([]byte, error) {
	block := codexManagedBegin + "\n" + strings.TrimSpace(generated) + "\n" + codexManagedEnd
	content := string(existing)
	start := strings.Index(content, codexManagedBegin)
	end := strings.Index(content, codexManagedEnd)
	if (start >= 0) != (end >= 0) || (start >= 0 && end < start) {
		return nil, errors.New("existing Codex trello-mcp managed block is incomplete")
	}
	if start >= 0 {
		end += len(codexManagedEnd)
		return []byte(content[:start] + block + content[end:]), nil
	}
	const unmanagedSection = "[mcp_servers.trello]"
	if sectionOffset := strings.Index(content, unmanagedSection); sectionOffset >= 0 {
		line := strings.Count(content[:sectionOffset], "\n") + 1
		return nil, fmt.Errorf("%s:%d: existing un-managed %s section found; remove it or use client-config codex to merge manually", target, line, unmanagedSection)
	}
	content = strings.TrimRight(content, "\r\n")
	if content == "" {
		return []byte(block + "\n"), nil
	}
	return []byte(content + "\n\n" + block + "\n"), nil
}

func mergeJSON(existing []byte, generated, client string) ([]byte, error) {
	document := make(map[string]any)
	if len(bytes.TrimSpace(existing)) != 0 {
		if err := decodeJSON(existing, &document); err != nil {
			return nil, fmt.Errorf("parse existing %s project config: %w", client, err)
		}
	}
	generatedDocument := make(map[string]any)
	if err := decodeJSON([]byte(generated), &generatedDocument); err != nil {
		return nil, fmt.Errorf("parse generated %s project config: %w", client, err)
	}

	containerKey := "mcpServers"
	if client == ClientOpenCode {
		containerKey = "mcp"
		if _, exists := document["$schema"]; !exists {
			document["$schema"] = generatedDocument["$schema"]
		}
	}
	generatedContainer, ok := generatedDocument[containerKey].(map[string]any)
	if !ok {
		return nil, fmt.Errorf("generated %s config has invalid %s object", client, containerKey)
	}
	container, exists := document[containerKey]
	if !exists {
		container = make(map[string]any)
		document[containerKey] = container
	}
	mcpServers, ok := container.(map[string]any)
	if !ok {
		return nil, fmt.Errorf("existing %s config field %q is not an object", client, containerKey)
	}
	mcpServers["trello"] = generatedContainer["trello"]

	data, err := json.MarshalIndent(document, "", "  ")
	if err != nil {
		return nil, fmt.Errorf("encode %s project config: %w", client, err)
	}
	return append(data, '\n'), nil
}

func decodeJSON(data []byte, target any) error {
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.UseNumber()
	return decoder.Decode(target)
}

func writeAtomic(path string, data []byte) error {
	directory := filepath.Dir(path)
	if err := os.MkdirAll(directory, 0o755); err != nil {
		return fmt.Errorf("create project config directory: %w", err)
	}
	mode := fs.FileMode(0o644)
	if info, err := os.Stat(path); err == nil {
		mode = info.Mode().Perm()
	} else if !errors.Is(err, fs.ErrNotExist) {
		return fmt.Errorf("inspect project config %s: %w", path, err)
	}
	temporary, err := os.CreateTemp(directory, ".trello-mcp-config-*")
	if err != nil {
		return fmt.Errorf("create temporary project config: %w", err)
	}
	temporaryPath := temporary.Name()
	cleanup := func() { _ = os.Remove(temporaryPath) }
	defer cleanup()
	if err := temporary.Chmod(mode); err != nil {
		_ = temporary.Close()
		return fmt.Errorf("set temporary project config permissions: %w", err)
	}
	if _, err := temporary.Write(data); err != nil {
		_ = temporary.Close()
		return fmt.Errorf("write temporary project config: %w", err)
	}
	if err := temporary.Sync(); err != nil {
		_ = temporary.Close()
		return fmt.Errorf("sync temporary project config: %w", err)
	}
	if err := temporary.Close(); err != nil {
		return fmt.Errorf("close temporary project config: %w", err)
	}
	if err := os.Rename(temporaryPath, path); err != nil {
		return fmt.Errorf("replace project config %s: %w", path, err)
	}
	return nil
}
