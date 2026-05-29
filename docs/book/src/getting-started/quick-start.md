# Quick Start

## 1. Run the Setup Wizard

On first launch, meka automatically starts an interactive setup wizard:

```bash
meka
```

The wizard will guide you through:

1. **Provider selection**: Choose between `claude-oauth`, `claude-api`, or `openai-api`
2. **Authentication**: OAuth login (`claude-oauth`) or API key entry (`claude-api`, `openai-api`)
3. **Model selection**: Enter the model name to use
4. **Base URL**: Optionally set a custom API endpoint

The wizard writes your configuration to `~/.config/meka/config.toml`. You can re-run it at any time with `meka setup`.

> You can also create the config file manually or use environment variables (`OPENAI_API_KEY`, `MEKA_PROVIDER`, etc.) and CLI flags (`--provider`, `-m`) as overrides. See [Configuration](../configuration/overview.md) for all options.

## 2. Start Using meka

After setup, you will see a prompt:

```text
meka [r] >
```

You will see a prompt:

```text
meka [r] >
```

The `[r]` indicates **read** permission mode (the default). The agent can read files and search, but cannot write files or run commands.

## 3. Ask It Something

```text
meka [r] > what files are in the current directory?
```

The agent will use the `find_files` tool to list files and describe them.

## 4. Enable Write Mode

Press **Shift+Tab** to cycle the permission to write mode:

```text
meka [w] >
```

Now the agent can execute commands and modify files:

```text
meka [w] > create a file called hello.txt with the text "hello world"
```

## 5. One-Shot Mode

For quick tasks without entering the interactive shell:

```bash
meka "what is my current working directory?"
```

The process exits after the agent responds.

## 6. Continue a Previous Session

To pick up where you left off, continue the last session:

```bash
meka -c
```

Or resume a specific session by its UUID:

```bash
meka -c 550e8400-e29b-41d4-a716-446655440000
```

See [Sessions](../usage/sessions.md) for more details.
