package config

import (
	"os"
	"path/filepath"
	"reflect"
	"testing"
)

func TestSaveLoadAndPermissions(t *testing.T) {
	path := filepath.Join(t.TempDir(), "nested", "config.json")
	want := Default()
	want.APIKey = "key"
	want.Token = "token"
	want.AllowedWorkspaceIDs = []string{"b", "a", "b"}
	if err := Save(path, want); err != nil {
		t.Fatal(err)
	}
	got, err := Load(path)
	if err != nil {
		t.Fatal(err)
	}
	if got.APIKey != want.APIKey || got.Token != want.Token {
		t.Fatalf("credentials did not round trip: %#v", got)
	}
	if !reflect.DeepEqual(got.AllowedWorkspaceIDs, []string{"b", "a"}) {
		t.Fatalf("unexpected workspaces: %#v", got.AllowedWorkspaceIDs)
	}
	info, err := os.Stat(path)
	if err != nil {
		t.Fatal(err)
	}
	if gotMode := info.Mode().Perm(); gotMode != 0o600 {
		t.Fatalf("config mode = %o, want 600", gotMode)
	}
}

func TestEnvironmentOverridesFile(t *testing.T) {
	t.Setenv("TRELLO_API_KEY", "env-key")
	t.Setenv("TRELLO_ALLOWED_WORKSPACES", "one, two,one")
	cfg := Default()
	cfg.APIKey = "file-key"
	got, err := ApplyEnvironment(cfg)
	if err != nil {
		t.Fatal(err)
	}
	if got.APIKey != "env-key" {
		t.Fatalf("APIKey = %q", got.APIKey)
	}
	if !reflect.DeepEqual(got.AllowedWorkspaceIDs, []string{"one", "two"}) {
		t.Fatalf("unexpected workspaces: %#v", got.AllowedWorkspaceIDs)
	}
}

func TestManagerUpdateDoesNotPersistEnvironment(t *testing.T) {
	path := filepath.Join(t.TempDir(), "config.json")
	if err := Save(path, Default()); err != nil {
		t.Fatal(err)
	}
	t.Setenv("TRELLO_API_KEY", "secret-from-env")
	m, err := NewManager(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := m.Update(func(cfg *Config) error {
		cfg.DefaultBoardID = "board"
		return nil
	}); err != nil {
		t.Fatal(err)
	}
	stored, err := Load(path)
	if err != nil {
		t.Fatal(err)
	}
	if stored.APIKey != "" {
		t.Fatalf("environment secret was persisted: %q", stored.APIKey)
	}
	if stored.DefaultBoardID != "board" {
		t.Fatalf("board was not persisted: %q", stored.DefaultBoardID)
	}
}
