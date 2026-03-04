# Tools Overview

Tools are the actions that the agent can perform on your behalf. The LLM decides which tools to call based on your instructions.

## Available Tools

| Tool | Permission | Description |
|------|-----------|-------------|
| [`read_file`](./file-operations.md#read_file) | Read | Read file contents |
| [`edit_file`](./file-operations.md#edit_file) | Write | Make string replacements in a file |
| [`write_file`](./file-operations.md#write_file) | Write | Create or overwrite a file |
| [`find_files`](./search.md#find_files) | Read | Find files by glob pattern |
| [`search_contents`](./search.md#search_contents) | Read | Search file contents with regex |
| [`fetch_url`](./web.md#fetch_url) | Read | Fetch a web page as markdown |
| [`web_search`](./web.md#web_search) | Read | Search the web |
| [`execute_command`](./shell.md#execute_command) | Write | Run a shell command |

## Permission Requirements

Tools are grouped by the minimum permission level required:

**Read permission** (available in read and write modes):
- `read_file`, `find_files`, `search_contents`, `fetch_url`, `web_search`

**Write permission** (only available in write mode):
- `edit_file`, `write_file`, `execute_command`

In **none** mode, no tools are available. The agent can only respond with text.

## How Tool Calls Work

1. The agent receives your instruction and decides which tools to call
2. For each tool call, agsh checks the current permission level
3. If permitted, the tool executes and its output is fed back to the agent
4. The agent may make additional tool calls or respond with text
5. This loop continues until the agent has no more tool calls to make

Tool calls and their results are displayed in the terminal so you can see what the agent is doing.
