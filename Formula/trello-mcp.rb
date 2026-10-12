class TrelloMcp < Formula
  desc "Model Context Protocol server for Trello"
  homepage "https://github.com/thaitanloi365/trello-mcp"
  license "MIT"
  head "https://github.com/thaitanloi365/trello-mcp.git", branch: "master"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_match "2.0.0", shell_output("#{bin}/trello-mcp version")
    assert_match "Run the Trello MCP server", shell_output("#{bin}/trello-mcp --help")
  end
end
