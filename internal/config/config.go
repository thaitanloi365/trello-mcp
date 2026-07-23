package config

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
)

const (
	defaultAPIBaseURL         = "https://api.trello.com/1"
	defaultTimeoutSeconds     = 30
	defaultMaxUploadBytes     = 10 << 20
	configEnvironmentVariable = "TRELLO_MCP_CONFIG"
)

// Config contains every persistent server setting. Credentials are stored in a
// file that is created with mode 0600; callers should still treat that file as
// a secret.
type Config struct {
	APIKey              string   `json:"api_key,omitempty"`
	Token               string   `json:"token,omitempty"`
	DefaultBoardID      string   `json:"default_board_id,omitempty"`
	WorkspaceID         string   `json:"workspace_id,omitempty"`
	AllowedWorkspaceIDs []string `json:"allowed_workspace_ids,omitempty"`
	APIBaseURL          string   `json:"api_base_url"`
	RequestTimeoutSecs  int      `json:"request_timeout_seconds"`
	MaxUploadBytes      int64    `json:"max_upload_bytes"`
}

// Default returns safe defaults for settings that do not contain credentials.
func Default() Config {
	return Config{
		APIBaseURL:         defaultAPIBaseURL,
		RequestTimeoutSecs: defaultTimeoutSeconds,
		MaxUploadBytes:     defaultMaxUploadBytes,
	}
}

// DefaultPath resolves the config path. TRELLO_MCP_CONFIG takes precedence
// over the conventional ~/.trello-mcp/config.json location.
func DefaultPath() (string, error) {
	if value := strings.TrimSpace(os.Getenv(configEnvironmentVariable)); value != "" {
		return expandHome(value)
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "", fmt.Errorf("resolve home directory: %w", err)
	}
	return filepath.Join(home, ".trello-mcp", "config.json"), nil
}

func expandHome(path string) (string, error) {
	if path == "~" || strings.HasPrefix(path, "~/") {
		home, err := os.UserHomeDir()
		if err != nil {
			return "", fmt.Errorf("resolve home directory: %w", err)
		}
		if path == "~" {
			return home, nil
		}
		return filepath.Join(home, strings.TrimPrefix(path, "~/")), nil
	}
	return filepath.Abs(path)
}

// ResolvePath uses the supplied path when non-empty, otherwise DefaultPath.
func ResolvePath(path string) (string, error) {
	if strings.TrimSpace(path) == "" {
		return DefaultPath()
	}
	return expandHome(path)
}

// Load reads a config file. A missing file is equivalent to an empty file and
// returns Default().
func Load(path string) (Config, error) {
	cfg := Default()
	data, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return cfg, nil
	}
	if err != nil {
		return Config{}, fmt.Errorf("read config %s: %w", path, err)
	}
	if err := json.Unmarshal(data, &cfg); err != nil {
		return Config{}, fmt.Errorf("parse config %s: %w", path, err)
	}
	cfg.normalize()
	return cfg, nil
}

// Save atomically persists cfg with restrictive permissions.
func Save(path string, cfg Config) error {
	cfg.normalize()
	if err := cfg.Validate(false); err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return fmt.Errorf("create config directory: %w", err)
	}
	data, err := json.MarshalIndent(cfg, "", "  ")
	if err != nil {
		return fmt.Errorf("encode config: %w", err)
	}
	data = append(data, '\n')
	tmp, err := os.CreateTemp(filepath.Dir(path), ".config-*.tmp")
	if err != nil {
		return fmt.Errorf("create temporary config: %w", err)
	}
	tmpName := tmp.Name()
	cleanup := func() { _ = os.Remove(tmpName) }
	if err := tmp.Chmod(0o600); err != nil {
		_ = tmp.Close()
		cleanup()
		return fmt.Errorf("secure temporary config: %w", err)
	}
	if _, err := tmp.Write(data); err != nil {
		_ = tmp.Close()
		cleanup()
		return fmt.Errorf("write temporary config: %w", err)
	}
	if err := tmp.Sync(); err != nil {
		_ = tmp.Close()
		cleanup()
		return fmt.Errorf("sync temporary config: %w", err)
	}
	if err := tmp.Close(); err != nil {
		cleanup()
		return fmt.Errorf("close temporary config: %w", err)
	}
	if err := os.Rename(tmpName, path); err != nil {
		cleanup()
		return fmt.Errorf("replace config: %w", err)
	}
	return os.Chmod(path, 0o600)
}

func (c *Config) normalize() {
	c.APIKey = strings.TrimSpace(c.APIKey)
	c.Token = strings.TrimSpace(c.Token)
	c.DefaultBoardID = strings.TrimSpace(c.DefaultBoardID)
	c.WorkspaceID = strings.TrimSpace(c.WorkspaceID)
	c.APIBaseURL = strings.TrimRight(strings.TrimSpace(c.APIBaseURL), "/")
	if c.APIBaseURL == "" {
		c.APIBaseURL = defaultAPIBaseURL
	}
	if c.RequestTimeoutSecs <= 0 {
		c.RequestTimeoutSecs = defaultTimeoutSeconds
	}
	if c.MaxUploadBytes <= 0 {
		c.MaxUploadBytes = defaultMaxUploadBytes
	}
	c.AllowedWorkspaceIDs = splitAndClean(strings.Join(c.AllowedWorkspaceIDs, ","))
}

// Validate checks configuration. When forServer is true credentials are
// required as well.
func (c Config) Validate(forServer bool) error {
	if forServer && (c.APIKey == "" || c.Token == "") {
		return errors.New("Trello credentials are required; run `trello-mcp config set --api-key ... --token ...` or set TRELLO_API_KEY and TRELLO_TOKEN")
	}
	if !strings.HasPrefix(c.APIBaseURL, "https://") && !strings.HasPrefix(c.APIBaseURL, "http://") {
		return errors.New("api_base_url must start with http:// or https://")
	}
	if c.RequestTimeoutSecs <= 0 {
		return errors.New("request_timeout_seconds must be greater than zero")
	}
	if c.MaxUploadBytes <= 0 {
		return errors.New("max_upload_bytes must be greater than zero")
	}
	return nil
}

// ApplyEnvironment overlays environment variables without mutating the file
// on disk. This keeps runtime overrides out of persistent storage.
func ApplyEnvironment(cfg Config) (Config, error) {
	setString := func(name string, dst *string) {
		if value, ok := os.LookupEnv(name); ok {
			*dst = strings.TrimSpace(value)
		}
	}
	setString("TRELLO_API_KEY", &cfg.APIKey)
	setString("TRELLO_TOKEN", &cfg.Token)
	setString("TRELLO_BOARD_ID", &cfg.DefaultBoardID)
	setString("TRELLO_WORKSPACE_ID", &cfg.WorkspaceID)
	setString("TRELLO_API_BASE_URL", &cfg.APIBaseURL)
	if value, ok := os.LookupEnv("TRELLO_ALLOWED_WORKSPACES"); ok {
		cfg.AllowedWorkspaceIDs = splitAndClean(value)
	}
	if value, ok := os.LookupEnv("TRELLO_REQUEST_TIMEOUT_SECONDS"); ok {
		parsed, err := strconv.Atoi(strings.TrimSpace(value))
		if err != nil {
			return Config{}, fmt.Errorf("parse TRELLO_REQUEST_TIMEOUT_SECONDS: %w", err)
		}
		cfg.RequestTimeoutSecs = parsed
	}
	if value, ok := os.LookupEnv("TRELLO_MAX_UPLOAD_BYTES"); ok {
		parsed, err := strconv.ParseInt(strings.TrimSpace(value), 10, 64)
		if err != nil {
			return Config{}, fmt.Errorf("parse TRELLO_MAX_UPLOAD_BYTES: %w", err)
		}
		cfg.MaxUploadBytes = parsed
	}
	cfg.normalize()
	return cfg, cfg.Validate(false)
}

func splitAndClean(value string) []string {
	seen := make(map[string]struct{})
	result := make([]string, 0)
	for _, item := range strings.Split(value, ",") {
		item = strings.TrimSpace(item)
		if item == "" {
			continue
		}
		if _, exists := seen[item]; exists {
			continue
		}
		seen[item] = struct{}{}
		result = append(result, item)
	}
	return result
}

// ParseWorkspaceIDs accepts the comma-separated representation used by the CLI.
func ParseWorkspaceIDs(value string) []string { return splitAndClean(value) }

// Manager synchronizes access to the persistent file and produces effective
// runtime config with environment variables applied.
type Manager struct {
	mu   sync.RWMutex
	path string
	file Config
}

func NewManager(path string) (*Manager, error) {
	resolved, err := ResolvePath(path)
	if err != nil {
		return nil, err
	}
	cfg, err := Load(resolved)
	if err != nil {
		return nil, err
	}
	return &Manager{path: resolved, file: cfg}, nil
}

func (m *Manager) Path() string { return m.path }

// File returns the persistent values without environment overrides.
func (m *Manager) File() Config {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return m.file
}

// Effective returns file settings overlaid with environment variables.
func (m *Manager) Effective() (Config, error) {
	m.mu.RLock()
	cfg := m.file
	m.mu.RUnlock()
	return ApplyEnvironment(cfg)
}

// Update atomically updates the persistent configuration.
func (m *Manager) Update(update func(*Config) error) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	next := m.file
	if err := update(&next); err != nil {
		return err
	}
	next.normalize()
	if err := Save(m.path, next); err != nil {
		return err
	}
	m.file = next
	return nil
}
