---
name: pr-assistant
description: Drafts pull request descriptions. Use to see a skill that declares an MCP server and a rule as dependencies.
license: Apache-2.0
metadata:
  summary: PR descriptions (requires an MCP server and a rule)
  keywords: demo,dependencies,pr
  grim.requires.mcp.docs-search: "localhost:5050/grimoire/mcp/docs-search:1"
  grim.requires.rules.security-baseline: "localhost:5050/grimoire/rules/security-baseline:1"
---

# PR Assistant

Dependency demo. Installing this skill also installs the `docs-search` MCP
server and the `security-baseline` rule, even though the project never
declares them. Run `grim why mcp docs-search` to see the chain.
