# One-Shot Mode

One-shot mode runs a single prompt and exits, similar to `bash -c`:

```bash
agsh -p "your prompt here"
```

The agent processes the prompt (including any tool calls), prints its response, and the process terminates. The session UUID is printed to stderr on exit.

## Examples

```bash
# Simple question
agsh -p "what is my current working directory?"

# File operations (requires write permission)
agsh --permission write -p "create a file called notes.txt with today's date"

# Search
agsh -p "find all TODO comments in this project"

# Web search
agsh -p "search the web for the latest Rust release"
```

## Combining with Other Flags

All configuration flags work in one-shot mode:

```bash
# Use a specific provider and model
agsh --provider anthropic -m claude-sonnet-4-20250514 -p "explain this codebase"

# With write permission
agsh --permission write -p "run 'cargo test' and summarize the results"

# Disable streaming
agsh --no-stream -p "read README.md and summarize it"
```

## Session Behavior

One-shot mode creates a new session for each invocation. The session UUID is printed to stderr when the run completes:

```text
Session: 550e8400-e29b-41d4-a716-446655440000
```

You can resume this session later in interactive mode:

```bash
agsh -s 550e8400-e29b-41d4-a716-446655440000
```
