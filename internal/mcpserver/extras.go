package mcpserver

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/thaitanloi365/trello-mcp/internal/config"
)

func (s *Server) persistBoard(boardID string) error {
	return s.client.Manager().Update(func(cfg *config.Config) error {
		cfg.DefaultBoardID = boardID
		return nil
	})
}

func (s *Server) persistWorkspace(workspaceID string) error {
	return s.client.Manager().Update(func(cfg *config.Config) error {
		cfg.WorkspaceID = workspaceID
		return nil
	})
}

func (s *Server) filterObjectsByWorkspace(value any, field string) any {
	cfg, err := s.client.Manager().Effective()
	if err != nil || len(cfg.AllowedWorkspaceIDs) == 0 {
		return value
	}
	items, ok := value.([]any)
	if !ok {
		return value
	}
	filtered := make([]any, 0, len(items))
	for _, item := range items {
		workspaceID := nestedString(item, field)
		if workspaceID != "" && s.client.WorkspaceAllowed(workspaceID) {
			filtered = append(filtered, item)
		}
	}
	return filtered
}

func (s *Server) filterByWorkspace(ctx context.Context, value any) (any, error) {
	cfg, err := s.client.Manager().Effective()
	if err != nil || len(cfg.AllowedWorkspaceIDs) == 0 {
		return value, err
	}
	items, ok := value.([]any)
	if !ok {
		return value, nil
	}
	allowedBoards := make(map[string]bool)
	filtered := make([]any, 0, len(items))
	for _, item := range items {
		boardID := nestedString(item, "idBoard")
		allowed, exists := allowedBoards[boardID]
		if !exists {
			allowed = s.client.EnsureBoardAllowed(ctx, boardID) == nil
			allowedBoards[boardID] = allowed
		}
		if allowed {
			filtered = append(filtered, item)
		}
	}
	return filtered, nil
}

func (s *Server) findChecklist(ctx context.Context, name, cardID, boardID string) (map[string]any, error) {
	name = strings.TrimSpace(name)
	if name == "" {
		return nil, errors.New("checklist name is required")
	}
	checklists, err := s.allChecklists(ctx, cardID, boardID)
	if err != nil {
		return nil, err
	}
	for _, checklist := range checklists {
		if strings.EqualFold(nestedString(checklist, "name"), name) {
			return checklist, nil
		}
	}
	return nil, fmt.Errorf("checklist %q was not found", name)
}

func (s *Server) allChecklists(ctx context.Context, cardID, boardID string) ([]map[string]any, error) {
	if cardID != "" {
		if err := s.ensureCardAllowed(ctx, cardID); err != nil {
			return nil, err
		}
		value, err := s.client.Do(ctx, http.MethodGet, "/cards/"+escape(cardID)+"/checklists", map[string]any{"checkItems": "all", "checkItem_fields": "all"}, nil)
		if err != nil {
			return nil, err
		}
		return objectSlice(value), nil
	}
	resolved, err := s.client.BoardID(boardID)
	if err != nil {
		return nil, err
	}
	if err := s.client.EnsureBoardAllowed(ctx, resolved); err != nil {
		return nil, err
	}
	value, err := s.client.Do(ctx, http.MethodGet, "/boards/"+escape(resolved)+"/cards", map[string]any{
		"filter": "visible", "fields": "id,name", "checklists": "all", "checkItem_fields": "all", "limit": 1000,
	}, nil)
	if err != nil {
		return nil, err
	}
	result := make([]map[string]any, 0)
	for _, rawCard := range objectSlice(value) {
		for _, rawChecklist := range objectSlice(rawCard["checklists"]) {
			if _, exists := rawChecklist["card"]; !exists {
				rawChecklist["card"] = map[string]any{"id": rawCard["id"], "name": rawCard["name"]}
			}
			result = append(result, rawChecklist)
		}
	}
	return result, nil
}

func (s *Server) findChecklistItems(ctx context.Context, query, cardID, boardID string) (any, error) {
	query = strings.ToLower(strings.TrimSpace(query))
	if query == "" {
		return nil, errors.New("description is required")
	}
	checklists, err := s.allChecklists(ctx, cardID, boardID)
	if err != nil {
		return nil, err
	}
	matches := make([]any, 0)
	for _, checklist := range checklists {
		for _, item := range objectSlice(checklist["checkItems"]) {
			if strings.Contains(strings.ToLower(nestedString(item, "name")), query) {
				matches = append(matches, map[string]any{
					"checklist_id": checklist["id"], "checklist_name": checklist["name"],
					"card": checklist["card"], "item": item,
				})
			}
		}
	}
	return matches, nil
}

func objectSlice(value any) []map[string]any {
	if objects, ok := value.([]map[string]any); ok {
		return objects
	}
	items, ok := value.([]any)
	if !ok {
		return nil
	}
	result := make([]map[string]any, 0, len(items))
	for _, item := range items {
		if object, ok := item.(map[string]any); ok {
			result = append(result, object)
		}
	}
	return result
}

func (s *Server) addCards(ctx context.Context, listID string, cards []any) (any, error) {
	if err := s.ensureListAllowed(ctx, listID); err != nil {
		return nil, err
	}
	created := make([]any, 0, len(cards))
	for index, raw := range cards {
		card, ok := raw.(map[string]any)
		if !ok {
			return nil, fmt.Errorf("cards[%d] must be an object", index)
		}
		name, _ := card["name"].(string)
		if strings.TrimSpace(name) == "" {
			return nil, fmt.Errorf("cards[%d].name is required", index)
		}
		params := map[string]any{"idList": listID, "name": name}
		for input, output := range map[string]string{"description": "desc", "due": "due", "start": "start", "position": "pos"} {
			if value, exists := card[input]; exists {
				params[output] = value
			}
		}
		result, err := s.client.Do(ctx, http.MethodPost, "/cards", params, nil)
		if err != nil {
			return nil, fmt.Errorf("create cards[%d]: %w", index, err)
		}
		created = append(created, result)
	}
	return map[string]any{"created": created, "count": len(created)}, nil
}

func (s *Server) updateCustomField(ctx context.Context, args map[string]any) (any, error) {
	cardID := text(args, "cardId")
	if err := s.ensureCardAllowed(ctx, cardID); err != nil {
		return nil, err
	}
	fieldID, kind := text(args, "customFieldId"), text(args, "type")
	path := "/cards/" + escape(cardID) + "/customField/" + escape(fieldID) + "/item"
	if kind == "clear" {
		return s.client.Do(ctx, http.MethodDelete, path, nil, nil)
	}
	value := text(args, "value")
	if value == "" {
		return nil, errors.New("value is required unless type is clear")
	}
	var body map[string]any
	switch kind {
	case "text":
		body = map[string]any{"value": map[string]any{"text": value}}
	case "number":
		if _, err := strconv.ParseFloat(value, 64); err != nil {
			return nil, fmt.Errorf("number custom field value: %w", err)
		}
		body = map[string]any{"value": map[string]any{"number": value}}
	case "checkbox":
		checked, err := strconv.ParseBool(value)
		if err != nil {
			return nil, errors.New("checkbox value must be true or false")
		}
		body = map[string]any{"value": map[string]any{"checked": strconv.FormatBool(checked)}}
	case "date":
		body = map[string]any{"value": map[string]any{"date": value}}
	case "list":
		body = map[string]any{"idValue": value}
	default:
		return nil, fmt.Errorf("unsupported custom field type %q", kind)
	}
	return s.client.Do(ctx, http.MethodPut, path, nil, body)
}

type getCardOptions struct {
	detailLevel     string
	commentsLimit   int
	format          string
	delivery        string
	includeMarkdown bool
	outputDir       string
	fileThreshold   int
}

const defaultCardFileThreshold = 16 * 1024

func (options getCardOptions) normalized() (getCardOptions, error) {
	options.detailLevel = strings.ToLower(strings.TrimSpace(options.detailLevel))
	if options.detailLevel == "" {
		options.detailLevel = "compact"
	}
	if options.detailLevel != "compact" && options.detailLevel != "full" {
		return options, errors.New("detailLevel must be compact or full")
	}
	if options.commentsLimit < 0 || options.commentsLimit > 100 {
		return options, errors.New("commentsLimit must be between 0 and 100")
	}
	options.format = strings.ToLower(strings.TrimSpace(options.format))
	if options.includeMarkdown {
		options.format = "markdown"
	}
	if options.format == "" {
		options.format = "markdown"
	}
	if options.format != "json" && options.format != "markdown" {
		return options, errors.New("format must be json or markdown")
	}
	options.delivery = strings.ToLower(strings.TrimSpace(options.delivery))
	if options.delivery == "" {
		options.delivery = "auto"
	}
	if options.delivery != "inline" && options.delivery != "file" && options.delivery != "auto" {
		return options, errors.New("delivery must be inline, file, or auto")
	}
	if options.fileThreshold <= 0 {
		options.fileThreshold = defaultCardFileThreshold
	}
	return options, nil
}

func normalizeCardRef(value string) (string, error) {
	value = strings.TrimSpace(value)
	if value == "" {
		return "", errors.New("card reference is required")
	}
	lower := strings.ToLower(value)
	if strings.HasPrefix(lower, "trello.com/") || strings.HasPrefix(lower, "www.trello.com/") {
		value = "https://" + value
	}
	if parsed, err := url.Parse(value); err == nil && parsed.Host != "" {
		if parsed.Scheme != "http" && parsed.Scheme != "https" {
			return "", errors.New("Trello card URL must use http or https")
		}
		host := strings.ToLower(parsed.Hostname())
		if host != "trello.com" && host != "www.trello.com" {
			return "", fmt.Errorf("unsupported Trello card URL host %q", parsed.Hostname())
		}
		parts := strings.Split(strings.Trim(parsed.EscapedPath(), "/"), "/")
		if len(parts) < 2 || !strings.EqualFold(parts[0], "c") {
			return "", errors.New("Trello card URL must have the form trello.com/c/{shortLink}")
		}
		decoded, err := url.PathUnescape(parts[1])
		if err != nil {
			return "", fmt.Errorf("decode Trello short link: %w", err)
		}
		value = decoded
	}
	if strings.ContainsAny(value, "/?#") {
		return "", errors.New("card reference must be a card ID, short link, or Trello card URL")
	}
	return value, nil
}

func (s *Server) getCard(ctx context.Context, cardRef string, options getCardOptions) (any, error) {
	cardID, err := normalizeCardRef(cardRef)
	if err != nil {
		return nil, err
	}
	options, err = options.normalized()
	if err != nil {
		return nil, err
	}
	if err := s.ensureCardAllowed(ctx, cardID); err != nil {
		return nil, err
	}
	params := cardRequestParams(options)
	card, err := s.client.Do(ctx, http.MethodGet, "/cards/"+escape(cardID), params, nil)
	if err != nil {
		return nil, err
	}
	object, ok := card.(map[string]any)
	if !ok {
		return nil, errors.New("Trello returned an invalid card response")
	}
	comments := objectSlice(object["actions"])
	delete(object, "actions")
	object["comments"] = comments
	object["commentsTruncated"] = options.commentsLimit > 0 && len(comments) == options.commentsLimit
	if options.detailLevel == "compact" {
		object = compactCard(object)
	}
	return deliverCard(cardID, object, options)
}

func deliverCard(cardID string, card map[string]any, options getCardOptions) (any, error) {
	var (
		payload []byte
		inline  any
		err     error
	)
	if options.format == "markdown" {
		text := renderCardMarkdown(card)
		payload = []byte(text)
		inline = rawTextResult(text)
	} else {
		payload, err = json.Marshal(card)
		if err != nil {
			return nil, fmt.Errorf("encode card output: %w", err)
		}
		inline = card
	}
	if options.delivery == "inline" || (options.delivery == "auto" && len(payload) <= options.fileThreshold) {
		return inline, nil
	}
	path, err := writeCardOutput(cardID, options.format, payload, options.outputDir)
	if err != nil {
		return nil, err
	}
	return map[string]any{
		"delivery":    "file",
		"format":      options.format,
		"path":        path,
		"bytes":       len(payload),
		"generatedAt": time.Now().UTC().Format(time.RFC3339),
		"card": map[string]any{
			"id":        card["id"],
			"shortLink": card["shortLink"],
			"name":      card["name"],
			"url":       card["url"],
		},
		"instruction": "Read the file at path to load the complete card output.",
	}, nil
}

func writeCardOutput(cardID, format string, payload []byte, outputDir string) (string, error) {
	if outputDir == "" {
		cacheDir, err := os.UserCacheDir()
		if err != nil {
			return "", fmt.Errorf("resolve Trello MCP cache directory: %w", err)
		}
		outputDir = filepath.Join(cacheDir, "trello-mcp", "outputs")
	}
	if err := os.MkdirAll(outputDir, 0o700); err != nil {
		return "", fmt.Errorf("create Trello MCP output directory: %w", err)
	}
	if err := os.Chmod(outputDir, 0o700); err != nil {
		return "", fmt.Errorf("secure Trello MCP output directory: %w", err)
	}

	extension := "json"
	if format == "markdown" {
		extension = "md"
	}
	temp, err := os.CreateTemp(outputDir, ".card-*.tmp")
	if err != nil {
		return "", fmt.Errorf("create temporary Trello MCP output: %w", err)
	}
	tempPath := temp.Name()
	cleanup := func() { _ = os.Remove(tempPath) }
	if err := temp.Chmod(0o600); err != nil {
		_ = temp.Close()
		cleanup()
		return "", fmt.Errorf("secure temporary Trello MCP output: %w", err)
	}
	if _, err := temp.Write(append(payload, '\n')); err != nil {
		_ = temp.Close()
		cleanup()
		return "", fmt.Errorf("write temporary Trello MCP output: %w", err)
	}
	if err := temp.Sync(); err != nil {
		_ = temp.Close()
		cleanup()
		return "", fmt.Errorf("sync temporary Trello MCP output: %w", err)
	}
	if err := temp.Close(); err != nil {
		cleanup()
		return "", fmt.Errorf("close temporary Trello MCP output: %w", err)
	}

	randomPart := strings.TrimSuffix(strings.TrimPrefix(filepath.Base(tempPath), ".card-"), ".tmp")
	filename := fmt.Sprintf("card-%s-%s-%s.%s",
		safeFilePart(cardID), time.Now().UTC().Format("20060102T150405Z"), randomPart, extension)
	path := filepath.Join(outputDir, filename)
	if err := os.Rename(tempPath, path); err != nil {
		cleanup()
		return "", fmt.Errorf("save Trello MCP output: %w", err)
	}
	if err := os.Chmod(path, 0o600); err != nil {
		return "", fmt.Errorf("secure Trello MCP output: %w", err)
	}
	absolute, err := filepath.Abs(path)
	if err != nil {
		return "", fmt.Errorf("resolve Trello MCP output path: %w", err)
	}
	return absolute, nil
}

func safeFilePart(value string) string {
	var out strings.Builder
	for _, char := range value {
		switch {
		case char >= 'a' && char <= 'z',
			char >= 'A' && char <= 'Z',
			char >= '0' && char <= '9',
			char == '-', char == '_':
			out.WriteRune(char)
		default:
			out.WriteByte('-')
		}
		if out.Len() >= 48 {
			break
		}
	}
	result := strings.Trim(out.String(), "-")
	if result == "" {
		return "card"
	}
	return result
}

func cardRequestParams(options getCardOptions) map[string]any {
	params := map[string]any{
		"attachments": true, "members": true, "checklists": "all", "customFieldItems": true,
	}
	if options.detailLevel == "full" {
		params["fields"] = "all"
		params["attachment_fields"] = "all"
		params["member_fields"] = "id,fullName,username,initials,avatarUrl"
		params["checkItem_fields"] = "all"
	} else {
		params["fields"] = "id,idBoard,idList,idShort,shortLink,name,desc,closed,due,dueComplete,start,dueReminder,dateLastActivity,url,shortUrl,labels,idAttachmentCover,coordinates,address,locationName,isTemplate"
		params["attachment_fields"] = "id,name,url,mimeType,date,bytes"
		params["member_fields"] = "id,fullName,username"
		params["checkItem_fields"] = "id,name,state,pos,due,dueReminder,idMember"
	}
	if options.commentsLimit > 0 {
		params["actions"] = "commentCard"
		params["actions_limit"] = options.commentsLimit
		if options.detailLevel == "compact" {
			params["action_fields"] = "id,idMemberCreator,data,type,date"
		}
	}
	return params
}

func compactCard(card map[string]any) map[string]any {
	result := make(map[string]any)
	copyPresent(result, card,
		"id", "idBoard", "idList", "idShort", "shortLink", "name", "desc", "closed",
		"due", "dueComplete", "start", "dueReminder", "dateLastActivity", "url", "shortUrl",
		"idAttachmentCover", "coordinates", "address", "locationName", "isTemplate",
	)
	result["labels"] = compactObjects(card["labels"], "id", "name", "color")
	result["members"] = compactObjects(card["members"], "id", "fullName", "username")
	result["attachments"] = compactObjects(card["attachments"], "id", "name", "url", "mimeType", "date", "bytes")
	result["customFieldItems"] = compactObjects(card["customFieldItems"], "id", "idCustomField", "idValue", "value")

	checklists := make([]any, 0)
	for _, checklist := range objectSlice(card["checklists"]) {
		compact := make(map[string]any)
		copyPresent(compact, checklist, "id", "name")
		compact["checkItems"] = compactObjects(checklist["checkItems"], "id", "name", "state", "pos", "due", "dueReminder", "idMember")
		checklists = append(checklists, compact)
	}
	result["checklists"] = checklists
	result["comments"] = compactComments(card["comments"])
	if truncated, ok := card["commentsTruncated"].(bool); ok {
		result["commentsTruncated"] = truncated
	}
	return result
}

func compactComments(value any) []any {
	result := make([]any, 0)
	for _, action := range objectSlice(value) {
		comment := make(map[string]any)
		copyPresent(comment, action, "id", "idMemberCreator", "date")
		if text := nestedString(action, "data", "text"); text != "" {
			comment["text"] = text
		}
		author := make(map[string]any)
		if creator, ok := action["memberCreator"].(map[string]any); ok {
			copyPresent(author, creator, "id", "fullName", "username")
		}
		if len(author) > 0 {
			comment["author"] = author
		}
		result = append(result, comment)
	}
	return result
}

func compactObjects(value any, fields ...string) []any {
	result := make([]any, 0)
	for _, object := range objectSlice(value) {
		compact := make(map[string]any)
		copyPresent(compact, object, fields...)
		result = append(result, compact)
	}
	return result
}

func copyPresent(target, source map[string]any, fields ...string) {
	for _, field := range fields {
		value, exists := source[field]
		if !exists || value == nil {
			continue
		}
		if text, ok := value.(string); ok && text == "" {
			continue
		}
		target[field] = value
	}
}

func renderCardMarkdown(card map[string]any) string {
	var out strings.Builder
	out.WriteString("# " + nestedString(card, "name") + "\n\n")
	if description := nestedString(card, "desc"); description != "" {
		out.WriteString(description + "\n\n")
	}
	if due := nestedString(card, "due"); due != "" {
		out.WriteString("- Due: " + due + "\n")
	}
	if closed, _ := card["closed"].(bool); closed {
		out.WriteString("- Status: Archived\n")
	} else {
		out.WriteString("- Status: Open\n")
	}
	if labels := compactNames(card["labels"], "name"); labels != "" {
		out.WriteString("- Labels: " + labels + "\n")
	}
	if members := compactNames(card["members"], "fullName"); members != "" {
		out.WriteString("- Members: " + members + "\n")
	}
	if link := nestedString(card, "url"); link != "" {
		out.WriteString("- URL: " + link + "\n")
	}
	for _, checklist := range objectSlice(card["checklists"]) {
		out.WriteString("\n## " + nestedString(checklist, "name") + "\n\n")
		for _, item := range objectSlice(checklist["checkItems"]) {
			mark := " "
			if nestedString(item, "state") == "complete" {
				mark = "x"
			}
			out.WriteString(fmt.Sprintf("- [%s] %s\n", mark, nestedString(item, "name")))
		}
	}
	if attachments := objectSlice(card["attachments"]); len(attachments) > 0 {
		out.WriteString("\n## Attachments\n\n")
		for _, attachment := range attachments {
			out.WriteString(fmt.Sprintf("- [%s](%s)\n", nestedString(attachment, "name"), nestedString(attachment, "url")))
		}
	}
	if comments := objectSlice(card["comments"]); len(comments) > 0 {
		out.WriteString("\n## Recent comments\n")
		for _, comment := range comments {
			author := nestedString(comment, "author", "fullName")
			if author == "" {
				author = nestedString(comment, "author", "username")
			}
			out.WriteString("\n### " + strings.TrimSpace(author+" "+nestedString(comment, "date")) + "\n\n")
			out.WriteString(nestedString(comment, "text") + "\n")
		}
	}
	return strings.TrimSpace(out.String())
}

func compactNames(value any, field string) string {
	names := make([]string, 0)
	for _, object := range objectSlice(value) {
		if name := nestedString(object, field); name != "" {
			names = append(names, name)
		}
	}
	return strings.Join(names, ", ")
}

func (s *Server) health(ctx context.Context, detailed bool) (any, error) {
	start := time.Now()
	_, apiErr := s.client.Do(ctx, http.MethodGet, "/members/me", map[string]any{"fields": "id,username"}, nil)
	cfg, cfgErr := s.client.Manager().Effective()
	result := map[string]any{
		"ok":         apiErr == nil && cfgErr == nil,
		"trello_api": map[string]any{"ok": apiErr == nil, "latency_ms": time.Since(start).Milliseconds(), "error": errorText(apiErr)},
		"config":     map[string]any{"ok": cfgErr == nil, "path": s.client.Manager().Path(), "error": errorText(cfgErr)},
	}
	if detailed && cfgErr == nil {
		result["settings"] = map[string]any{
			"credentials_configured": cfg.APIKey != "" && cfg.Token != "", "active_board_id": cfg.DefaultBoardID,
			"active_workspace_id": cfg.WorkspaceID, "allowed_workspace_count": len(cfg.AllowedWorkspaceIDs),
			"api_base_url": cfg.APIBaseURL, "request_timeout_seconds": cfg.RequestTimeoutSecs, "max_upload_bytes": cfg.MaxUploadBytes,
		}
	}
	return result, nil
}

func (s *Server) healthMetadata() (any, error) {
	cfg, err := s.client.Manager().Effective()
	if err != nil {
		return nil, err
	}
	return map[string]any{
		"name": "trello-mcp-go", "version": Version, "transport": "stdio", "config_path": s.client.Manager().Path(),
		"active_board_id": cfg.DefaultBoardID, "active_workspace_id": cfg.WorkspaceID,
	}, nil
}

func (s *Server) repair() (any, error) {
	if err := s.client.Manager().Update(func(cfg *config.Config) error { return cfg.Validate(false) }); err != nil {
		return nil, err
	}
	cfg, err := s.client.Manager().Effective()
	if err != nil {
		return nil, err
	}
	if err := cfg.Validate(true); err != nil {
		return nil, err
	}
	return map[string]any{"ok": true, "config_path": s.client.Manager().Path(), "actions": []string{"normalized config", "validated credentials and limits"}}, nil
}
