class TrelloMcp < Formula
  desc "Model Context Protocol server for Trello"
  homepage "https://github.com/thaitanloi365/trello-mcp"
  license "MIT"
  head "https://github.com/thaitanloi365/trello-mcp.git", branch: "master"

  depends_on "go" => :build

  def install
    system "go", "build", *std_go_args(ldflags: "-s -w"), "./cmd/trello-mcp"
  end

  test do
    assert_match "1.0.0", shell_output("#{bin}/trello-mcp version")
    assert_match "Run Trello MCP server", shell_output("#{bin}/trello-mcp --help")
  end
end
