# Anthropic Provider

The Anthropic provider uses the [Messages API](https://docs.anthropic.com/en/api/messages).

## Configuration

| Setting | Value |
|---------|-------|
| Provider name | `anthropic` |
| Default base URL | `https://api.anthropic.com` |
| API key env var | `ANTHROPIC_API_KEY` |
| Auth method | `x-api-key` header |
| API version | `2023-06-01` |
| Max tokens | `8192` |

### Minimal Setup

```bash
export AGSH_PROVIDER=anthropic
export AGSH_MODEL=claude-sonnet-4-20250514
export ANTHROPIC_API_KEY=sk-ant-...
agsh
```

### Config File

```toml
[provider]
name = "anthropic"
model = "claude-sonnet-4-20250514"
```

## Supported Models

Any model available through the Anthropic Messages API:

- `claude-opus-4-20250514`
- `claude-sonnet-4-20250514`
- `claude-haiku-4-5-20251001`

## Custom Base URL

To use an Anthropic-compatible proxy or gateway:

```bash
agsh --provider anthropic --model claude-sonnet-4-20250514 --base-url https://my-proxy.example.com
```

## API Details

**Endpoint:** `POST {base_url}/v1/messages`

**Headers:**
- `x-api-key: <api_key>`
- `anthropic-version: 2023-06-01`
- `content-type: application/json`

**System prompt:** Sent as a top-level `system` field in the request body (not as a message).

**Tool format:** Tools are defined with `input_schema` instead of `parameters`:

```json
{
  "name": "read_file",
  "description": "Read the contents of a file at the given path.",
  "input_schema": { "type": "object", "properties": { ... } }
}
```

**Tool use and results:** Expressed as content blocks within messages:

- Tool use: `{"type": "tool_use", "id": "...", "name": "...", "input": {...}}`
- Tool result: `{"type": "tool_result", "tool_use_id": "...", "content": "..."}`

**Streaming:** Uses Server-Sent Events with named event types:

| Event | Description |
|-------|-------------|
| `message_start` | Message initialization |
| `content_block_start` | Begin a text or tool_use block |
| `content_block_delta` | Incremental text (`text_delta`) or tool input (`input_json_delta`) |
| `content_block_stop` | End of a content block |
| `message_delta` | Final metadata including `stop_reason` |
| `message_stop` | Stream complete |
| `ping` | Keep-alive |
