# Permissions

agsh uses a four-level permission system to control what tools the agent can use. This gives you control over the agent's capabilities and prevents accidental modifications.

## Permission Levels

| Level | Indicator | Allowed Tools |
|-------|-----------|---------------|
| **None** | `[n]` (green) | No tools. The agent can only respond with text. |
| **Read** | `[r]` (yellow) | Read-only tools: `read_file`, `find_files`, `search_contents`, `fetch_url`, `web_search`, `execute_command` (sandboxed), `todo_write`, `spawn_agent`, scratchpad tools |
| **Ask** | `[a]` (magenta) | All tools, but each call requires user approval (Y/n prompt) |
| **Write** | `[w]` (red) | All tools without restrictions: `write_file`, `edit_file`, `execute_command` (unsandboxed) |

Each level includes all tools from the levels below it. Write mode includes all read tools.

## Default Permission

The default permission is **read**. You can change it with:

- CLI flag: `agsh --permission write`
- Environment variable: `export AGSH_PERMISSION=write`

## Changing Permissions at Runtime

Press **Shift+Tab** to cycle through permission levels:

```text
none → read → ask → write → none → ...
```

Or use the `/permission` slash command:

```text
/permission write
/permission ask
```

The prompt indicator updates immediately to reflect the new level. The agent is informed of the current permission level in its system prompt, so it knows which tools are available.

## Ask Mode

In ask mode, the agent has access to all tools, but each tool call is paused for your approval:

```text
[ask] Shell ls -la (Y/n)
```

Press **Enter** or **y** to approve, or **n** to deny. If denied, the agent receives an error and may try an alternative approach.

This mode is useful when you want the agent to have full capabilities but want to review each action before it executes.

## How Permissions Work

When the agent attempts to use a tool, agsh checks whether the current permission level allows it:

- If allowed, the tool executes normally.
- In ask mode, you are prompted to approve or deny.
- If denied, agsh returns an error message to the agent explaining that the tool requires a higher permission level.

The agent is also instructed (via the system prompt) to inform you if it cannot perform a requested action due to permission restrictions and to suggest pressing Shift+Tab to change the level.

## Examples

### Read Mode (Default)

```text
agsh [r] > read the contents of main.rs
```

The agent uses `read_file` and shows the contents. Shell commands also work in read mode, but run in a **read-only sandbox** -- the filesystem is physically write-protected for the child process:

```text
agsh [r] > list the files in this directory
agsh [r] > show me the git log
```

Commands like `ls`, `cat`, `git log`, `df`, `ps`, and `uname` work normally. Commands that attempt to write to the filesystem (e.g., `touch`, `rm`, `mkdir`) will fail with a permission error.

If you ask the agent to modify a file:

```text
agsh [r] > add a comment to the top of main.rs
```

The agent will explain that it cannot write files in read mode and suggest switching to write mode.

> **Note:** The read-only sandbox uses Landlock on Linux (kernel 5.13+) and sandbox-exec on macOS. On platforms where sandboxing is unavailable, shell commands are not available in read mode. You can disable sandboxed shell execution by setting `sandbox = false` under `[shell]` in the config file (see [Config File](../configuration/config-file.md)).

### Write Mode

```text
agsh [w] > run cargo test and show me the output
```

The agent uses `execute_command` to run the tests and shows the results.
