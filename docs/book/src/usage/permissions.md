# Permissions

agsh uses a three-level permission system to control what tools the agent can use. This gives you control over the agent's capabilities and prevents accidental modifications.

## Permission Levels

| Level | Indicator | Allowed Tools |
|-------|-----------|---------------|
| **None** | `[n]` (green) | No tools. The agent can only respond with text. |
| **Read** | `[r]` (yellow) | Read-only tools: `read_file`, `find_files`, `search_contents`, `fetch_url`, `web_search` |
| **Write** | `[w]` (red) | All tools, including: `write_file`, `edit_file`, `execute_command` |

Each level includes all tools from the levels below it. Write mode includes all read tools.

## Default Permission

The default permission is **read**. You can change it with:

- CLI flag: `agsh -p write`
- Environment variable: `export AGSH_PERMISSION=write`

## Changing Permissions at Runtime

Press **Shift+Tab** to cycle through permission levels:

```text
none → read → write → none → ...
```

The prompt indicator updates immediately to reflect the new level. The agent is informed of the current permission level in its system prompt, so it knows which tools are available.

## How Permissions Work

When the agent attempts to use a tool, agsh checks whether the current permission level allows it:

- If allowed, the tool executes normally.
- If denied, agsh returns an error message to the agent explaining that the tool requires a higher permission level.

The agent is also instructed (via the system prompt) to inform you if it cannot perform a requested action due to permission restrictions and to suggest pressing Shift+Tab to change the level.

## Examples

### Read Mode (Default)

```text
agsh [r] > read the contents of main.rs
```

The agent uses `read_file` and shows the contents. If you ask it to modify a file:

```text
agsh [r] > add a comment to the top of main.rs
```

The agent will explain that it cannot write files in read mode and suggest switching to write mode.

### Write Mode

```text
agsh [w] > run cargo test and show me the output
```

The agent uses `execute_command` to run the tests and shows the results.
