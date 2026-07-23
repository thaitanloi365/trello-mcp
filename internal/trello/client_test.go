package trello

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"

	"github.com/thaitanloi365/trello-mcp/internal/config"
)

func testClient(t *testing.T, handler http.HandlerFunc) (*Client, *config.Manager) {
	t.Helper()
	server := httptest.NewServer(handler)
	t.Cleanup(server.Close)
	path := filepath.Join(t.TempDir(), "config.json")
	cfg := config.Default()
	cfg.APIKey = "test-key"
	cfg.Token = "test-token"
	cfg.APIBaseURL = server.URL
	if err := config.Save(path, cfg); err != nil {
		t.Fatal(err)
	}
	manager, err := config.NewManager(path)
	if err != nil {
		t.Fatal(err)
	}
	return NewClient(manager), manager
}

func TestDoAddsCredentialsAndDecodesResponse(t *testing.T) {
	client, _ := testClient(t, func(w http.ResponseWriter, r *http.Request) {
		if got := r.URL.Query().Get("key"); got != "test-key" {
			t.Errorf("key = %q", got)
		}
		if got := r.URL.Query().Get("token"); got != "test-token" {
			t.Errorf("token = %q", got)
		}
		if got := r.URL.Query().Get("limit"); got != "5" {
			t.Errorf("limit = %q", got)
		}
		if got := r.Header.Get("User-Agent"); got != userAgent {
			t.Errorf("User-Agent = %q, want %q", got, userAgent)
		}
		_ = json.NewEncoder(w).Encode(map[string]any{"id": "card-1"})
	})
	result, err := client.Do(context.Background(), http.MethodGet, "/cards/card-1", map[string]any{"limit": 5}, nil)
	if err != nil {
		t.Fatal(err)
	}
	if result.(map[string]any)["id"] != "card-1" {
		t.Fatalf("unexpected response: %#v", result)
	}
}

func TestAPIErrorDoesNotLeakCredentials(t *testing.T) {
	client, _ := testClient(t, func(w http.ResponseWriter, _ *http.Request) {
		http.Error(w, `{"message":"bad request"}`, http.StatusBadRequest)
	})
	_, err := client.Do(context.Background(), http.MethodGet, "/cards/nope", nil, nil)
	if err == nil {
		t.Fatal("expected an error")
	}
	for _, secret := range []string{"test-key", "test-token"} {
		if strings.Contains(err.Error(), secret) {
			t.Fatalf("error leaked %q: %v", secret, err)
		}
	}
}

func TestNonIdempotentRequestIsNotRetriedAfterServerError(t *testing.T) {
	var calls atomic.Int32
	client, _ := testClient(t, func(w http.ResponseWriter, _ *http.Request) {
		calls.Add(1)
		http.Error(w, "temporary", http.StatusInternalServerError)
	})
	_, err := client.Do(context.Background(), http.MethodPost, "/cards", map[string]any{"name": "card"}, nil)
	if err == nil {
		t.Fatal("expected an error")
	}
	if got := calls.Load(); got != 1 {
		t.Fatalf("POST was attempted %d times, want 1", got)
	}
}

func TestDownloadAttachmentUsesAuthenticatedTrelloDownloadEndpoint(t *testing.T) {
	client, _ := testClient(t, func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/cards/card/attachments/attachment":
			_ = json.NewEncoder(w).Encode(map[string]any{
				"id": "attachment", "name": "report.txt", "url": "http://127.0.0.1:1/should-not-be-fetched",
			})
		case "/cards/card/attachments/attachment/download/report.txt":
			if got := r.Header.Get("Authorization"); !strings.Contains(got, "test-key") || !strings.Contains(got, "test-token") {
				t.Errorf("missing OAuth credentials: %q", got)
			}
			if got := r.Header.Get("User-Agent"); got != userAgent {
				t.Errorf("User-Agent = %q, want %q", got, userAgent)
			}
			_, _ = w.Write([]byte("attachment contents"))
		default:
			http.NotFound(w, r)
		}
	})
	destination := filepath.Join(t.TempDir(), "download.txt")
	if _, err := client.DownloadAttachment(context.Background(), "card", "attachment", destination); err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(destination)
	if err != nil {
		t.Fatal(err)
	}
	if string(data) != "attachment contents" {
		t.Fatalf("download = %q", data)
	}
}
