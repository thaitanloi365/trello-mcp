package trello

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"mime/multipart"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/thaitanloi365/trello-mcp/internal/config"
)

const maxResponseBytes = 16 << 20

// APIError is a sanitized Trello HTTP error. It deliberately excludes the
// request URL because Trello credentials are query parameters.
type APIError struct {
	Method     string
	Path       string
	StatusCode int
	Message    string
}

func (e *APIError) Error() string {
	return fmt.Sprintf("Trello %s %s returned HTTP %d: %s", e.Method, e.Path, e.StatusCode, e.Message)
}

// Client is a concurrency-safe Trello REST client backed by a live config
// manager. A conservative limiter stays below Trello's per-token limit.
type Client struct {
	config *config.Manager
	http   *http.Client

	limitMu sync.Mutex
	next    time.Time
}

func NewClient(manager *config.Manager) *Client {
	return &Client{config: manager, http: &http.Client{}}
}

func NewClientWithHTTP(manager *config.Manager, client *http.Client) *Client {
	if client == nil {
		client = &http.Client{}
	}
	return &Client{config: manager, http: client}
}

func (c *Client) Manager() *config.Manager { return c.config }

func (c *Client) wait(ctx context.Context) error {
	c.limitMu.Lock()
	now := time.Now()
	ready := c.next
	if ready.Before(now) {
		ready = now
	}
	c.next = ready.Add(105 * time.Millisecond)
	c.limitMu.Unlock()
	if delay := time.Until(ready); delay > 0 {
		timer := time.NewTimer(delay)
		defer timer.Stop()
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-timer.C:
		}
	}
	return nil
}

// Do invokes a Trello endpoint. params are encoded as query parameters; body
// is JSON when non-nil. The decoded JSON result is returned as ordinary Go
// values so MCP can preserve the native Trello response shape.
func (c *Client) Do(ctx context.Context, method, path string, params map[string]any, body any) (any, error) {
	cfg, err := c.config.Effective()
	if err != nil {
		return nil, err
	}
	if err := cfg.Validate(true); err != nil {
		return nil, err
	}
	requestURL, err := buildURL(cfg, path, params)
	if err != nil {
		return nil, err
	}
	var payload []byte
	if body != nil {
		payload, err = json.Marshal(body)
		if err != nil {
			return nil, fmt.Errorf("encode Trello request body: %w", err)
		}
	}
	return c.execute(ctx, cfg, method, path, requestURL, payload, "application/json")
}

func buildURL(cfg config.Config, path string, params map[string]any) (string, error) {
	base, err := url.Parse(strings.TrimRight(cfg.APIBaseURL, "/") + "/" + strings.TrimLeft(path, "/"))
	if err != nil {
		return "", fmt.Errorf("build Trello URL: %w", err)
	}
	query := base.Query()
	query.Set("key", cfg.APIKey)
	query.Set("token", cfg.Token)
	for name, value := range params {
		appendQueryValue(query, name, value)
	}
	base.RawQuery = query.Encode()
	return base.String(), nil
}

func appendQueryValue(query url.Values, name string, value any) {
	if value == nil {
		return
	}
	switch typed := value.(type) {
	case string:
		query.Set(name, typed)
	case bool:
		query.Set(name, strconv.FormatBool(typed))
	case int:
		query.Set(name, strconv.Itoa(typed))
	case int64:
		query.Set(name, strconv.FormatInt(typed, 10))
	case float64:
		query.Set(name, strconv.FormatFloat(typed, 'f', -1, 64))
	case []string:
		query.Set(name, strings.Join(typed, ","))
	default:
		query.Set(name, fmt.Sprint(value))
	}
}

func (c *Client) execute(
	ctx context.Context,
	cfg config.Config,
	method, safePath, requestURL string,
	payload []byte,
	contentType string,
) (any, error) {
	var lastErr error
	for attempt := 0; attempt < 3; attempt++ {
		if err := c.wait(ctx); err != nil {
			return nil, err
		}
		requestCtx, cancel := context.WithTimeout(ctx, time.Duration(cfg.RequestTimeoutSecs)*time.Second)
		req, err := http.NewRequestWithContext(requestCtx, method, requestURL, bytes.NewReader(payload))
		if err != nil {
			cancel()
			return nil, fmt.Errorf("create Trello request: %w", err)
		}
		if len(payload) > 0 {
			req.Header.Set("Content-Type", contentType)
		}
		req.Header.Set("Accept", "application/json")
		req.Header.Set("User-Agent", "trello-mcp-go/0.1.0")
		resp, err := c.http.Do(req)
		if err != nil {
			cancel()
			if requestCtx.Err() != nil {
				lastErr = requestCtx.Err()
			} else {
				lastErr = err
			}
			if attempt < 2 && idempotent(method) {
				continue
			}
			return nil, fmt.Errorf("call Trello %s %s: %w", method, safePath, lastErr)
		}
		data, readErr := io.ReadAll(io.LimitReader(resp.Body, maxResponseBytes+1))
		_ = resp.Body.Close()
		cancel()
		if readErr != nil {
			return nil, fmt.Errorf("read Trello response: %w", readErr)
		}
		if len(data) > maxResponseBytes {
			return nil, errors.New("Trello response exceeded 16 MiB limit")
		}
		retryableStatus := resp.StatusCode == http.StatusTooManyRequests || (resp.StatusCode >= 500 && idempotent(method))
		if retryableStatus {
			if attempt < 2 {
				delay := retryDelay(resp.Header.Get("Retry-After"), attempt)
				select {
				case <-ctx.Done():
					return nil, ctx.Err()
				case <-time.After(delay):
				}
				continue
			}
		}
		if resp.StatusCode < 200 || resp.StatusCode >= 300 {
			return nil, &APIError{
				Method: method, Path: safePath, StatusCode: resp.StatusCode,
				Message: responseMessage(data, resp.StatusCode),
			}
		}
		if len(bytes.TrimSpace(data)) == 0 {
			return map[string]any{"ok": true}, nil
		}
		var decoded any
		if err := json.Unmarshal(data, &decoded); err != nil {
			return map[string]any{"ok": true, "response": string(data)}, nil
		}
		return decoded, nil
	}
	return nil, lastErr
}

func idempotent(method string) bool {
	switch method {
	case http.MethodGet, http.MethodHead, http.MethodOptions, http.MethodPut, http.MethodDelete:
		return true
	default:
		return false
	}
}

func retryDelay(value string, attempt int) time.Duration {
	if seconds, err := strconv.Atoi(strings.TrimSpace(value)); err == nil && seconds > 0 {
		return time.Duration(seconds) * time.Second
	}
	return time.Duration(1<<attempt) * 250 * time.Millisecond
}

func responseMessage(data []byte, statusCode int) string {
	trimmed := strings.TrimSpace(string(data))
	if trimmed == "" {
		return http.StatusText(statusCode)
	}
	var object map[string]any
	if json.Unmarshal(data, &object) == nil {
		for _, key := range []string{"message", "error"} {
			if value, ok := object[key].(string); ok && value != "" {
				return value
			}
		}
	}
	if len(trimmed) > 500 {
		return trimmed[:500] + "..."
	}
	return trimmed
}

// UploadFile attaches a local file to a card. File size is checked before any
// network request, and the configured limit defaults to 10 MiB.
func (c *Client) UploadFile(ctx context.Context, cardID, path, name string) (any, error) {
	cfg, err := c.config.Effective()
	if err != nil {
		return nil, err
	}
	info, err := os.Stat(path)
	if err != nil {
		return nil, fmt.Errorf("open attachment: %w", err)
	}
	if !info.Mode().IsRegular() {
		return nil, errors.New("attachment path must be a regular file")
	}
	if info.Size() > cfg.MaxUploadBytes {
		return nil, fmt.Errorf("attachment is %d bytes; configured limit is %d", info.Size(), cfg.MaxUploadBytes)
	}
	file, err := os.Open(path)
	if err != nil {
		return nil, fmt.Errorf("open attachment: %w", err)
	}
	defer file.Close()
	if name == "" {
		name = filepath.Base(path)
	}
	return c.upload(ctx, cfg, cardID, name, file, info.Size())
}

// UploadData attaches in-memory data while enforcing the same configured size
// limit as file uploads.
func (c *Client) UploadData(ctx context.Context, cardID, name string, data []byte) (any, error) {
	cfg, err := c.config.Effective()
	if err != nil {
		return nil, err
	}
	if int64(len(data)) > cfg.MaxUploadBytes {
		return nil, fmt.Errorf("attachment is %d bytes; configured limit is %d", len(data), cfg.MaxUploadBytes)
	}
	return c.upload(ctx, cfg, cardID, name, bytes.NewReader(data), int64(len(data)))
}

// DownloadAttachment resolves an attachment through Trello, downloads it with
// Trello OAuth credentials, and atomically writes the explicit destination.
// The configured max_upload_bytes value is also used as the download limit.
func (c *Client) DownloadAttachment(ctx context.Context, cardID, attachmentID, destination string) (any, error) {
	metadata, err := c.Do(ctx, http.MethodGet, "/cards/"+url.PathEscape(cardID)+"/attachments/"+url.PathEscape(attachmentID), nil, nil)
	if err != nil {
		return nil, err
	}
	object, ok := metadata.(map[string]any)
	if !ok {
		return nil, errors.New("Trello returned invalid attachment metadata")
	}
	fileName, _ := object["name"].(string)
	if strings.TrimSpace(fileName) == "" {
		fileName = attachmentID
	}
	cfg, err := c.config.Effective()
	if err != nil {
		return nil, err
	}
	downloadPath := "/cards/" + url.PathEscape(cardID) + "/attachments/" + url.PathEscape(attachmentID) + "/download/" + url.PathEscape(fileName)
	downloadURL, err := buildURL(cfg, downloadPath, nil)
	if err != nil {
		return nil, err
	}
	requestCtx, cancel := context.WithTimeout(ctx, time.Duration(cfg.RequestTimeoutSecs)*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(requestCtx, http.MethodGet, downloadURL, nil)
	if err != nil {
		return nil, fmt.Errorf("create attachment download: %w", err)
	}
	req.Header.Set("Authorization", fmt.Sprintf(`OAuth oauth_consumer_key="%s", oauth_token="%s"`, cfg.APIKey, cfg.Token))
	req.Header.Set("User-Agent", "trello-mcp-go/0.1.0")
	resp, err := c.http.Do(req)
	if err != nil {
		return nil, fmt.Errorf("download Trello attachment: %w", err)
	}
	defer resp.Body.Close()
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return nil, &APIError{Method: http.MethodGet, Path: "/attachments/download", StatusCode: resp.StatusCode, Message: http.StatusText(resp.StatusCode)}
	}
	absDestination, err := filepath.Abs(destination)
	if err != nil {
		return nil, fmt.Errorf("resolve attachment destination: %w", err)
	}
	parent := filepath.Dir(absDestination)
	info, err := os.Stat(parent)
	if err != nil || !info.IsDir() {
		return nil, fmt.Errorf("attachment destination directory does not exist: %s", parent)
	}
	tmp, err := os.CreateTemp(parent, ".trello-attachment-*.tmp")
	if err != nil {
		return nil, fmt.Errorf("create temporary attachment: %w", err)
	}
	tmpName := tmp.Name()
	cleanup := func() { _ = os.Remove(tmpName) }
	if err := tmp.Chmod(0o600); err != nil {
		_ = tmp.Close()
		cleanup()
		return nil, fmt.Errorf("secure temporary attachment: %w", err)
	}
	written, copyErr := io.Copy(tmp, io.LimitReader(resp.Body, cfg.MaxUploadBytes+1))
	if copyErr != nil {
		_ = tmp.Close()
		cleanup()
		return nil, fmt.Errorf("write attachment: %w", copyErr)
	}
	if written > cfg.MaxUploadBytes {
		_ = tmp.Close()
		cleanup()
		return nil, fmt.Errorf("attachment exceeds configured limit of %d bytes", cfg.MaxUploadBytes)
	}
	if err := tmp.Sync(); err != nil {
		_ = tmp.Close()
		cleanup()
		return nil, fmt.Errorf("sync attachment: %w", err)
	}
	if err := tmp.Close(); err != nil {
		cleanup()
		return nil, fmt.Errorf("close attachment: %w", err)
	}
	if err := os.Rename(tmpName, absDestination); err != nil {
		cleanup()
		return nil, fmt.Errorf("save attachment: %w", err)
	}
	if err := os.Chmod(absDestination, 0o600); err != nil {
		return nil, fmt.Errorf("secure attachment: %w", err)
	}
	return map[string]any{"ok": true, "path": absDestination, "bytes": written, "attachment": metadata}, nil
}

func (c *Client) upload(ctx context.Context, cfg config.Config, cardID, name string, source io.Reader, size int64) (any, error) {
	if err := cfg.Validate(true); err != nil {
		return nil, err
	}
	var body bytes.Buffer
	writer := multipart.NewWriter(&body)
	part, err := writer.CreateFormFile("file", name)
	if err != nil {
		return nil, fmt.Errorf("create attachment form: %w", err)
	}
	if _, err := io.CopyN(part, source, size); err != nil {
		return nil, fmt.Errorf("read attachment: %w", err)
	}
	if err := writer.Close(); err != nil {
		return nil, fmt.Errorf("finalize attachment form: %w", err)
	}
	path := "/cards/" + url.PathEscape(cardID) + "/attachments"
	requestURL, err := buildURL(cfg, path, nil)
	if err != nil {
		return nil, err
	}
	return c.execute(ctx, cfg, http.MethodPost, path, requestURL, body.Bytes(), writer.FormDataContentType())
}

// BoardID resolves an explicit board ID or the configured active/default ID.
func (c *Client) BoardID(explicit string) (string, error) {
	if id := strings.TrimSpace(explicit); id != "" {
		return id, nil
	}
	cfg, err := c.config.Effective()
	if err != nil {
		return "", err
	}
	if cfg.DefaultBoardID == "" {
		return "", errors.New("boardId is required because no active board is configured")
	}
	return cfg.DefaultBoardID, nil
}

func (c *Client) WorkspaceAllowed(id string) bool {
	cfg, err := c.config.Effective()
	if err != nil || len(cfg.AllowedWorkspaceIDs) == 0 {
		return err == nil
	}
	for _, allowed := range cfg.AllowedWorkspaceIDs {
		if id == allowed {
			return true
		}
	}
	return false
}

func (c *Client) RequireWorkspaceAllowed(id string) error {
	if id == "" {
		return errors.New("workspaceId is required")
	}
	if !c.WorkspaceAllowed(id) {
		return fmt.Errorf("workspace %q is not allowed by configuration", id)
	}
	return nil
}

// EnsureBoardAllowed verifies the board's workspace when an allow-list is set.
func (c *Client) EnsureBoardAllowed(ctx context.Context, boardID string) error {
	cfg, err := c.config.Effective()
	if err != nil {
		return err
	}
	if len(cfg.AllowedWorkspaceIDs) == 0 {
		return nil
	}
	board, err := c.Do(ctx, http.MethodGet, "/boards/"+url.PathEscape(boardID), map[string]any{"fields": "id,idOrganization,name"}, nil)
	if err != nil {
		return err
	}
	object, ok := board.(map[string]any)
	if !ok {
		return errors.New("Trello returned an invalid board response")
	}
	workspace, _ := object["idOrganization"].(string)
	if !c.WorkspaceAllowed(workspace) {
		return fmt.Errorf("board %q belongs to workspace %q, which is not allowed", boardID, workspace)
	}
	return nil
}
