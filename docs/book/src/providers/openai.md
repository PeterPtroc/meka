# OpenAI Provider

The OpenAI provider uses the [Chat Completions API](https://platform.openai.com/docs/api-reference/chat). It also works with any OpenAI-compatible API endpoint.

## Configuration

| Setting | Value |
|---------|-------|
| Provider name | `openai` |
| Default base URL | `https://api.openai.com/v1` |
| API key env var | `OPENAI_API_KEY` |
| Auth method | Bearer token (`Authorization: Bearer <key>`) |

### Minimal Setup

```bash
export AGSH_PROVIDER=openai
export AGSH_MODEL=gpt-4o
export OPENAI_API_KEY=sk-...
agsh
```

### Config File

```toml
[provider]
name = "openai"
model = "gpt-4o"
```

## Supported Models

Any model available through the OpenAI Chat Completions API (or compatible endpoint) that supports tool calling:

- `gpt-4o`, `gpt-4o-mini`
- `gpt-4-turbo`
- `o1`, `o3-mini`
- Third-party models via compatible APIs

## Custom Base URL

To use an OpenAI-compatible endpoint, set the base URL:

```bash
# Ollama
agsh --provider openai --model llama3 --base-url http://localhost:11434/v1

# OpenRouter
agsh --provider openai --model anthropic/claude-sonnet-4-20250514 --base-url https://openrouter.ai/api/v1
```

Or in the config file:

```toml
[provider]
name = "openai"
model = "llama3"
api_key = "unused"
base_url = "http://localhost:11434/v1"
```

## API Details

**Endpoint:** `POST {base_url}/chat/completions`

**Tool format:** Tools are sent as function definitions:

```json
{
  "type": "function",
  "function": {
    "name": "read_file",
    "description": "Read the contents of a file at the given path.",
    "parameters": { "type": "object", "properties": { ... } }
  }
}
```

**Tool results:** Sent back as messages with `role: "tool"` and the corresponding `tool_call_id`.

**Streaming:** Uses Server-Sent Events (SSE) with `data: {...}` lines. The stream ends with `data: [DONE]`.
