# CLI Options

```text
agsh [OPTIONS] [PROMPT]
agsh <COMMAND>
```

## Commands

### `setup`

Run the interactive configuration wizard. Prompts for provider, authentication, model, and base URL, then writes the configuration to `~/.config/agsh/config.toml`.

```bash
agsh setup
```

This wizard also runs automatically on first launch when no config file exists.

### `export`

Export a session as Markdown.

```bash
agsh export <SESSION_ID> [-o <PATH>]
```

Use `-o -` to print to stdout. See [Sessions](../usage/sessions.md#exporting-a-session) for details.

### `delete`

Delete one or more sessions by UUID, or all sessions with `--all`.

```bash
agsh delete <SESSION_ID>...
agsh delete --all
```

### `list`

List past sessions with ID, last update time, and a preview.

```bash
agsh list [-n <LIMIT>]
```

Default limit: 20.

## Arguments

### `[PROMPT]`

Run a one-shot prompt and exit. The agent processes the prompt, prints its response, and the process terminates.

```bash
agsh "list all files larger than 1MB in the current directory"
```

When omitted, agsh starts in interactive mode.

## Options

### `-s`, `--session <UUID>`

Resume an existing session by its UUID. The session's message history is loaded and the conversation continues.

```bash
agsh -s 550e8400-e29b-41d4-a716-446655440000
```

Errors if the session does not exist or is locked by another agsh instance.

### `-c`, `--continue`

Resume the most recently updated session. Equivalent to `-s` with the last session's UUID.

```bash
agsh -c
```

### `--permission <MODE>`

Set the initial permission mode. Accepts `none` (or `n`), `read` (or `r`), `ask` (or `a`), `write` (or `w`). Case-insensitive.

```bash
agsh --permission write
agsh --permission ask
```

Default: `read`.

### `--provider <NAME>`

Set the LLM provider. Overrides `AGSH_PROVIDER` and the config file.

```bash
agsh --provider claude
```

Supported values: `openai`, `claude`.

### `-m`, `--model <MODEL>`

Set the model name. Overrides `AGSH_MODEL` and the config file.

```bash
agsh -m gpt-4o-mini
```

### `--base-url <URL>`

Set a custom API base URL. Overrides `OPENAI_BASE_URL` and the config file.

```bash
agsh --base-url http://localhost:11434/v1
```

### `--no-stream`

Disable streaming mode. The agent waits for the complete response before displaying it. By default, responses are streamed token-by-token.

```bash
agsh --no-stream
```

### `--render-mode <MODE>`

Set the output render mode. Accepts `bat` (default), `termimad` (or `rich`), or `raw`.

- `bat`: Syntax-highlighted markdown output via bat.
- `termimad`: Full terminal formatting (box-drawn code blocks, reflowed paragraphs, formatted tables).
- `raw`: Raw markdown printed verbatim with aligned tables.

```bash
agsh --render-mode raw
```

Can also be set permanently via `display.render_mode` in the config file.

### `--thinking`

Enable extended thinking (Claude provider only).

```bash
agsh --thinking
```

### `--thinking-budget <TOKENS>`

Set the extended thinking token budget. Implies `--thinking`.

```bash
agsh --thinking-budget 20000
```

### `-v`, `--verbose`

Increase log verbosity. Can be repeated up to three times.

```bash
agsh -v      # info
agsh -vv     # debug
agsh -vvv    # trace
```

### `--help`

Print help information.

### `--version`

Print version information.
