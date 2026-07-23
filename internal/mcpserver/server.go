package mcpserver

import (
	"context"
	"encoding/json"
	"fmt"
	"math"
	"strings"

	"github.com/modelcontextprotocol/go-sdk/mcp"
	"github.com/thaitanloi365/trello-mcp/internal/trello"
)

const Version = "0.1.0"

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
}

type toolDefinition struct {
	Name         string
	Description  string
	Fields       []fieldDefinition
	OutputSchema any
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
		&mcp.ServerOptions{Instructions: "Use these tools to inspect and update Trello. Responses are optimized for LLM consumption. Prefer configured active board/workspace IDs when the caller does not supply one."},
	)
	for _, definition := range toolDefinitions() {
		instance.register(definition)
	}
	return instance
}

func (s *Server) Run(ctx context.Context) error {
	return s.mcp.Run(ctx, &mcp.StdioTransport{})
}

func (s *Server) MCP() *mcp.Server { return s.mcp }

func (s *Server) register(definition toolDefinition) {
	s.mcp.AddTool(
		&mcp.Tool{
			Name:         definition.Name,
			Description:  definition.Description,
			InputSchema:  inputSchema(definition.Fields),
			OutputSchema: definition.OutputSchema,
		},
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
			payload, err := json.Marshal(result)
			if err != nil {
				return toolError(fmt.Errorf("encode tool result: %w", err)), nil
			}
			response := &mcp.CallToolResult{
				Content: []mcp.Content{&mcp.TextContent{Text: string(payload)}},
			}
			if definition.OutputSchema != nil {
				if structured, ok := result.(map[string]any); ok {
					response.StructuredContent = structured
				}
			}
			return response, nil
		},
	)
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
	}
	return nil
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
