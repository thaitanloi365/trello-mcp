package mcpserver

import (
	"context"
	"encoding/json"
	"fmt"
	"math"
	"strings"

	"github.com/modelcontextprotocol/go-sdk/mcp"
	"github.com/thaitanloi365/trello-mcp/internal/trello"
	"github.com/thaitanloi365/trello-mcp/internal/version"
)

const Version = version.Current

type fieldKind string

const (
	kindString      fieldKind = "string"
	kindNumber      fieldKind = "number"
	kindInteger     fieldKind = "integer"
	kindBoolean     fieldKind = "boolean"
	kindStringArray fieldKind = "string_array"
	kindObjectArray fieldKind = "object_array"
	kindObject      fieldKind = "object"
)

type fieldDefinition struct {
	Name        string
	Kind        fieldKind
	Description string
	Required    bool
	Enum        []string
	Default     any
	Items       []fieldDefinition // object_array element fields
}

type toolDefinition struct {
	Name        string
	Description string
	Fields      []fieldDefinition
	ReadOnly    bool
}

type rawTextResult string

// Server connects MCP tool calls to Trello's REST API.
type Server struct {
	client *trello.Client
	mcp    *mcp.Server
}

func New(client *trello.Client) *Server {
	instance := &Server{client: client}
	instance.mcp = mcp.NewServer(
		&mcp.Implementation{Name: "trello-mcp-go", Version: Version},
		&mcp.ServerOptions{Instructions: "Trello tools. Cards take an ID, short link, or URL; lists, labels, members, and custom fields take a name or ID. " +
			"Board tools default to the active board. Before any change other people see (comments, new or updated cards, checklists), " +
			"show the user a preview and wait for approval. Keep text short: main point first."},
	)
	for _, definition := range toolDefinitions() {
		instance.register(definition)
	}
	instance.mcp.AddPrompt(&mcp.Prompt{
		Name:        "draft_reply",
		Description: "Draft a reply comment for a Trello card and post it only after approval",
		Arguments: []*mcp.PromptArgument{
			{Name: "card", Description: "Card ID, short link, or Trello card URL", Required: true},
			{Name: "notes", Description: "What the reply should say"},
		},
	}, draftReplyPrompt)
	return instance
}

func draftReplyPrompt(_ context.Context, request *mcp.GetPromptRequest) (*mcp.GetPromptResult, error) {
	card := strings.TrimSpace(request.Params.Arguments["card"])
	if card == "" {
		return nil, fmt.Errorf("argument %q is required", "card")
	}
	text := "Draft a reply comment for Trello card " + card + ".\n" +
		"1. Read the card with get_card.\n" +
		"2. " + replyStyle + "\n" +
		"3. Show me the draft. Post it with add_comment only after I approve.\n"
	if notes := strings.TrimSpace(request.Params.Arguments["notes"]); notes != "" {
		text += "\nWhat to say: " + notes + "\n"
	}
	return &mcp.GetPromptResult{
		Description: "Draft a Trello reply",
		Messages:    []*mcp.PromptMessage{{Role: "user", Content: &mcp.TextContent{Text: text}}},
	}, nil
}

func (s *Server) Run(ctx context.Context) error {
	return s.mcp.Run(ctx, &mcp.StdioTransport{})
}

func (s *Server) MCP() *mcp.Server { return s.mcp }

func (s *Server) register(definition toolDefinition) {
	tool := &mcp.Tool{Name: definition.Name, Description: definition.Description, InputSchema: inputSchema(definition.Fields)}
	if definition.ReadOnly {
		tool.Annotations = &mcp.ToolAnnotations{ReadOnlyHint: true}
	}
	s.mcp.AddTool(
		tool,
		func(ctx context.Context, request *mcp.CallToolRequest) (*mcp.CallToolResult, error) {
			args := make(map[string]any)
			if request.Params.Arguments != nil {
				if err := json.Unmarshal(request.Params.Arguments, &args); err != nil {
					return toolError(fmt.Errorf("decode arguments: %w", err)), nil
				}
			}
			if err := validateArguments(definition.Fields, args); err != nil {
				return toolError(err), nil
			}
			result, err := s.handle(ctx, definition.Name, args)
			if err != nil {
				return toolError(err), nil
			}
			if text, ok := result.(rawTextResult); ok {
				return &mcp.CallToolResult{
					Content: []mcp.Content{&mcp.TextContent{Text: string(text)}},
				}, nil
			}
			payload, err := encodeResult(result)
			if err != nil {
				return toolError(fmt.Errorf("encode tool result: %w", err)), nil
			}
			return &mcp.CallToolResult{
				Content: []mcp.Content{&mcp.TextContent{Text: payload}},
			}, nil
		},
	)
}

// encodeResult writes compact JSON without HTML escaping: json.Marshal would
// turn every &, <, and > into a six-character \u escape the model must decode.
func encodeResult(result any) (string, error) {
	var out strings.Builder
	encoder := json.NewEncoder(&out)
	encoder.SetEscapeHTML(false)
	if err := encoder.Encode(result); err != nil {
		return "", err
	}
	return strings.TrimSuffix(out.String(), "\n"), nil
}

func toolError(err error) *mcp.CallToolResult {
	var result mcp.CallToolResult
	result.SetError(err)
	return &result
}

func inputSchema(fields []fieldDefinition) map[string]any {
	properties := make(map[string]any, len(fields))
	required := make([]string, 0)
	for _, field := range fields {
		property := map[string]any{"description": field.Description}
		switch field.Kind {
		case kindStringArray:
			property["type"] = "array"
			property["items"] = map[string]any{"type": "string"}
		case kindObjectArray:
			property["type"] = "array"
			property["items"] = map[string]any{"type": "object"}
			if len(field.Items) > 0 {
				property["items"] = inputSchema(field.Items)
			}
		case kindObject:
			property["type"] = "object"
			property["additionalProperties"] = true
		default:
			property["type"] = string(field.Kind)
		}
		if len(field.Enum) > 0 {
			property["enum"] = field.Enum
		}
		if field.Default != nil {
			property["default"] = field.Default
		}
		properties[field.Name] = property
		if field.Required {
			required = append(required, field.Name)
		}
	}
	schema := map[string]any{
		"type":                 "object",
		"properties":           properties,
		"additionalProperties": false,
	}
	if len(required) > 0 {
		schema["required"] = required
	}
	return schema
}

func validateArguments(fields []fieldDefinition, args map[string]any) error {
	known := make(map[string]fieldDefinition, len(fields))
	for _, field := range fields {
		known[field.Name] = field
		value, exists := args[field.Name]
		if field.Required && (!exists || value == nil || strings.TrimSpace(fmt.Sprint(value)) == "") {
			return fmt.Errorf("argument %q is required", field.Name)
		}
	}
	for name, value := range args {
		field, exists := known[name]
		if !exists {
			return fmt.Errorf("unknown argument %q", name)
		}
		if value == nil {
			continue
		}
		if err := validateKind(field, value); err != nil {
			return err
		}
		for index, item := range objectItems(field, value) {
			if err := validateArguments(field.Items, item); err != nil {
				return fmt.Errorf("%s[%d]: %w", field.Name, index, err)
			}
		}
	}
	return nil
}

// objectItems returns the elements of an object_array that declares Items.
// validateKind has already confirmed value is an array.
func objectItems(field fieldDefinition, value any) []map[string]any {
	if len(field.Items) == 0 {
		return nil
	}
	items := value.([]any)
	result := make([]map[string]any, len(items))
	for index, item := range items {
		object, _ := item.(map[string]any)
		result[index] = object
	}
	return result
}

func validateKind(field fieldDefinition, value any) error {
	valid := false
	switch field.Kind {
	case kindString:
		_, valid = value.(string)
	case kindBoolean:
		_, valid = value.(bool)
	case kindNumber, kindInteger:
		number, ok := value.(float64)
		valid = ok
		if ok && field.Kind == kindInteger && math.Trunc(number) != number {
			return fmt.Errorf("argument %q must be an integer", field.Name)
		}
	case kindStringArray, kindObjectArray:
		_, valid = value.([]any)
	case kindObject:
		_, valid = value.(map[string]any)
	}
	if !valid {
		return fmt.Errorf("argument %q must be %s", field.Name, field.Kind)
	}
	if len(field.Enum) > 0 {
		text, _ := value.(string)
		for _, candidate := range field.Enum {
			if text == candidate {
				return nil
			}
		}
		return fmt.Errorf("argument %q must be one of %s", field.Name, strings.Join(field.Enum, ", "))
	}
	return nil
}
