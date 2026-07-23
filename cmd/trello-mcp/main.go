package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/signal"
	"path/filepath"
	"strings"
	"syscall"

	"github.com/spf13/cobra"
	"github.com/thaitanloi365/trello-mcp/internal/clientconfig"
	"github.com/thaitanloi365/trello-mcp/internal/config"
	"github.com/thaitanloi365/trello-mcp/internal/mcpserver"
	"github.com/thaitanloi365/trello-mcp/internal/trello"
)

func main() {
	if err := run(os.Args[1:], os.Stdout, os.Stderr); err != nil {
		fmt.Fprintln(os.Stderr, "trello-mcp:", err)
		os.Exit(1)
	}
}

func run(args []string, stdout, stderr io.Writer) error {
	// Preserve the legacy single-dash version flag while Cobra handles the
	// conventional --version flag and version command.
	if len(args) > 0 && args[0] == "-version" {
		args = append([]string(nil), args...)
		args[0] = "--version"
	}

	root := newRootCommand(stdout, stderr)
	root.SetArgs(args)
	return root.ExecuteContext(context.Background())
}

func newRootCommand(stdout, stderr io.Writer) *cobra.Command {
	var configPath string

	root := &cobra.Command{
		Use:           "trello-mcp",
		Short:         "Run the Trello MCP server",
		SilenceErrors: true,
		SilenceUsage:  true,
		Args:          cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			return serve(cmd.Context(), configPath)
		},
	}
	root.SetOut(stdout)
	root.SetErr(stderr)
	root.Version = mcpserver.Version
	root.SetVersionTemplate("{{.Version}}\n")
	root.PersistentFlags().StringVar(&configPath, "config", "", "path to config file")

	root.AddCommand(
		newServeCommand(&configPath),
		newConfigCommand(&configPath),
		newClientConfigCommand(&configPath),
		newSetupCommand(&configPath),
		newVersionCommand(),
	)
	return root
}

func newServeCommand(configPath *string) *cobra.Command {
	return &cobra.Command{
		Use:   "serve",
		Short: "Run the MCP server over stdio",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			return serve(cmd.Context(), *configPath)
		},
	}
}

func serve(parent context.Context, configPath string) error {
	manager, err := config.NewManager(configPath)
	if err != nil {
		return err
	}
	effective, err := manager.Effective()
	if err != nil {
		return err
	}
	if err := effective.Validate(true); err != nil {
		return err
	}
	ctx, stop := signal.NotifyContext(parent, os.Interrupt, syscall.SIGTERM)
	defer stop()
	return mcpserver.New(trello.NewClient(manager)).Run(ctx)
}

func newConfigCommand(configPath *string) *cobra.Command {
	cmd := &cobra.Command{
		Use:   "config",
		Short: "Create, edit, show, and validate persistent configuration",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			cmd.SetOut(cmd.ErrOrStderr())
			if err := cmd.Help(); err != nil {
				return err
			}
			return errors.New("config subcommand is required")
		},
	}

	cmd.AddCommand(
		newConfigInitCommand(configPath),
		newConfigSetCommand(configPath),
		newConfigShowCommand(configPath),
		newConfigValidateCommand(configPath),
		newConfigPathCommand(configPath),
		newConfigHelpCommand(),
	)
	return cmd
}

func newConfigHelpCommand() *cobra.Command {
	return &cobra.Command{
		Use:    "help",
		Hidden: true,
		Args:   cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			return cmd.Parent().Help()
		},
	}
}

type configSetOptions struct {
	apiKey           string
	token            string
	boardID          string
	workspaceID      string
	allowed          string
	apiBaseURL       string
	timeout          int
	maxUpload        int64
	clearCredentials bool
	clearBoard       bool
	clearWorkspace   bool
	clearAllowed     bool
}

func newConfigSetCommand(configPath *string) *cobra.Command {
	var options configSetOptions
	cmd := &cobra.Command{
		Use:   "set",
		Short: "Persist one or more configuration values",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			manager, err := config.NewManager(*configPath)
			if err != nil {
				return err
			}
			if err := manager.Update(func(cfg *config.Config) error {
				if options.clearCredentials {
					cfg.APIKey, cfg.Token = "", ""
				}
				if options.clearBoard {
					cfg.DefaultBoardID = ""
				}
				if options.clearWorkspace {
					cfg.WorkspaceID = ""
				}
				if options.clearAllowed {
					cfg.AllowedWorkspaceIDs = nil
				}
				if cmd.Flags().Changed("api-key") {
					cfg.APIKey = options.apiKey
				}
				if cmd.Flags().Changed("token") {
					cfg.Token = options.token
				}
				if cmd.Flags().Changed("board-id") {
					cfg.DefaultBoardID = options.boardID
				}
				if cmd.Flags().Changed("workspace-id") {
					cfg.WorkspaceID = options.workspaceID
				}
				if cmd.Flags().Changed("allowed-workspaces") {
					cfg.AllowedWorkspaceIDs = config.ParseWorkspaceIDs(options.allowed)
				}
				if cmd.Flags().Changed("api-base-url") {
					cfg.APIBaseURL = options.apiBaseURL
				}
				if cmd.Flags().Changed("timeout-seconds") {
					cfg.RequestTimeoutSecs = options.timeout
				}
				if cmd.Flags().Changed("max-upload-bytes") {
					cfg.MaxUploadBytes = options.maxUpload
				}
				return cfg.Validate(false)
			}); err != nil {
				return err
			}
			fmt.Fprintln(cmd.OutOrStdout(), manager.Path())
			return nil
		},
	}

	flags := cmd.Flags()
	flags.StringVar(&options.apiKey, "api-key", "", "Trello API key")
	flags.StringVar(&options.token, "token", "", "Trello API token")
	flags.StringVar(&options.boardID, "board-id", "", "default/active board ID")
	flags.StringVar(&options.workspaceID, "workspace-id", "", "active workspace ID")
	flags.StringVar(&options.allowed, "allowed-workspaces", "", "comma-separated allowed workspace IDs")
	flags.StringVar(&options.apiBaseURL, "api-base-url", "", "Trello API base URL")
	flags.IntVar(&options.timeout, "timeout-seconds", 0, "HTTP request timeout in seconds")
	flags.Int64Var(&options.maxUpload, "max-upload-bytes", 0, "maximum upload/download size")
	flags.BoolVar(&options.clearCredentials, "clear-credentials", false, "remove API key and token from the config file")
	flags.BoolVar(&options.clearBoard, "clear-board", false, "clear the active board")
	flags.BoolVar(&options.clearWorkspace, "clear-workspace", false, "clear the active workspace")
	flags.BoolVar(&options.clearAllowed, "clear-allowed-workspaces", false, "remove the workspace allow-list")
	return cmd
}

func newConfigShowCommand(configPath *string) *cobra.Command {
	var reveal, effective bool
	cmd := &cobra.Command{
		Use:   "show",
		Short: "Print configuration (credentials are masked by default)",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			manager, err := config.NewManager(*configPath)
			if err != nil {
				return err
			}
			cfg := manager.File()
			if effective {
				cfg, err = manager.Effective()
				if err != nil {
					return err
				}
			}
			if !reveal {
				cfg.APIKey = mask(cfg.APIKey)
				cfg.Token = mask(cfg.Token)
			}
			data, err := json.MarshalIndent(cfg, "", "  ")
			if err != nil {
				return err
			}
			fmt.Fprintln(cmd.OutOrStdout(), string(data))
			return nil
		},
	}
	cmd.Flags().BoolVar(&reveal, "reveal-secrets", false, "show API key and token")
	cmd.Flags().BoolVar(&effective, "effective", false, "include environment overrides")
	return cmd
}

func newConfigInitCommand(configPath *string) *cobra.Command {
	return &cobra.Command{
		Use:   "init",
		Short: "Create the config file with defaults",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			manager, err := config.NewManager(*configPath)
			if err != nil {
				return err
			}
			if err := config.Save(manager.Path(), manager.File()); err != nil {
				return err
			}
			fmt.Fprintln(cmd.OutOrStdout(), manager.Path())
			return nil
		},
	}
}

func newConfigValidateCommand(configPath *string) *cobra.Command {
	var withoutCredentials bool
	cmd := &cobra.Command{
		Use:   "validate",
		Short: "Validate effective file and environment configuration",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			manager, err := config.NewManager(*configPath)
			if err != nil {
				return err
			}
			cfg, err := manager.Effective()
			if err != nil {
				return err
			}
			if err := cfg.Validate(!withoutCredentials); err != nil {
				return err
			}
			fmt.Fprintln(cmd.OutOrStdout(), "ok")
			return nil
		},
	}
	cmd.Flags().BoolVar(&withoutCredentials, "without-credentials", false, "validate structure without requiring credentials")
	return cmd
}

func newConfigPathCommand(configPath *string) *cobra.Command {
	return &cobra.Command{
		Use:   "path",
		Short: "Print the resolved config path",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			resolved, err := config.ResolvePath(*configPath)
			if err != nil {
				return err
			}
			fmt.Fprintln(cmd.OutOrStdout(), resolved)
			return nil
		},
	}
}

func newClientConfigCommand(configPath *string) *cobra.Command {
	var commandPath string
	cmd := &cobra.Command{
		Use:       "client-config <client>",
		Short:     "Generate MCP configuration for an AI coding client",
		Args:      cobra.ExactArgs(1),
		ValidArgs: clientconfig.SupportedClients(),
		RunE: func(cmd *cobra.Command, args []string) error {
			options, err := resolveClientOptions(commandPath, *configPath)
			if err != nil {
				return err
			}
			output, err := clientconfig.Render(args[0], options)
			if err != nil {
				return err
			}
			fmt.Fprint(cmd.OutOrStdout(), output)
			return nil
		},
	}
	cmd.Flags().StringVar(&commandPath, "command", "", "path clients should use to start trello-mcp")
	return cmd
}

func newSetupCommand(configPath *string) *cobra.Command {
	var commandPath, scope, projectDir string
	var all bool
	cmd := &cobra.Command{
		Use:       "setup [client]",
		Short:     "Configure trello-mcp in an AI coding client",
		ValidArgs: clientconfig.ProjectClients(),
		Args: func(cmd *cobra.Command, args []string) error {
			if all {
				if len(args) != 0 {
					return errors.New("--all cannot be combined with a client")
				}
				return nil
			}
			return cobra.ExactArgs(1)(cmd, args)
		},
		RunE: func(cmd *cobra.Command, args []string) error {
			if scope != "project" {
				return fmt.Errorf("unsupported setup scope %q (choose project)", scope)
			}
			options, err := resolveClientOptions(commandPath, *configPath)
			if err != nil {
				return err
			}
			clients := args
			if all {
				clients = clientconfig.ProjectClients()
			}
			for _, client := range clients {
				target, err := clientconfig.SetupProject(client, projectDir, options)
				if err != nil {
					return err
				}
				fmt.Fprintln(cmd.OutOrStdout(), target)
			}
			return nil
		},
	}
	cmd.Flags().BoolVar(&all, "all", false, "configure every project-scoped AI coding client")
	cmd.Flags().StringVar(&scope, "scope", "project", "configuration scope (project)")
	cmd.Flags().StringVar(&projectDir, "project-dir", ".", "project root to configure")
	cmd.Flags().StringVar(&commandPath, "command", "", "path clients should use to start trello-mcp")
	return cmd
}

func resolveClientOptions(commandPath, configPath string) (clientconfig.Options, error) {
	if commandPath == "" {
		var err error
		commandPath, err = os.Executable()
		if err != nil {
			return clientconfig.Options{}, fmt.Errorf("resolve trello-mcp executable: %w", err)
		}
	}
	absoluteCommand, err := filepath.Abs(commandPath)
	if err != nil {
		return clientconfig.Options{}, fmt.Errorf("resolve trello-mcp command: %w", err)
	}
	resolvedConfig := ""
	if strings.TrimSpace(configPath) != "" {
		resolvedConfig, err = config.ResolvePath(configPath)
		if err != nil {
			return clientconfig.Options{}, err
		}
	}
	return clientconfig.Options{Command: absoluteCommand, ConfigPath: resolvedConfig}, nil
}

func newVersionCommand() *cobra.Command {
	return &cobra.Command{
		Use:   "version",
		Short: "Print the version",
		Args:  cobra.NoArgs,
		Run: func(cmd *cobra.Command, _ []string) {
			fmt.Fprintln(cmd.OutOrStdout(), mcpserver.Version)
		},
	}
}

func mask(value string) string {
	if value == "" {
		return ""
	}
	if len(value) <= 8 {
		return "********"
	}
	return value[:4] + "..." + value[len(value)-4:]
}
