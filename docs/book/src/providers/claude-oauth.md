# Claude OAuth Provider

The `claude-oauth` provider authenticates via the Claude Code OAuth flow and mimics the Claude Code CLI's request shape, headers, and request signing. Use this when you have a Claude Code subscription rather than a per-token API key. For the direct Messages API, see [`claude-api`](./claude-api.md).

> **Note:** This provider replicates Claude Code's fingerprinting and attestation machinery exactly. Modifying the request body, headers, or OAuth flow will cause requests to be rejected by Anthropic. If you hit 401/403 errors, verify that no middleware is rewriting the request.

## Configuration

| Setting | Value |
|---------|-------|
| Provider name | `claude-oauth` |
| Default base URL | `https://api.anthropic.com` |
| OAuth token env var | `CLAUDE_OAUTH_TOKEN` |
| OAuth client ID env var | `CLAUDE_CLIENT_ID` (optional) |
| Auth method | `Authorization: Bearer <oauth_token>` |
| API version | `2023-06-01` |

### Quickest Start (Setup Wizard)

```bash
agsh setup
```

Pick **claude-oauth** when prompted. The wizard opens your browser, walks you through the authorization, and saves the tokens to the local database.

### Minimal Setup (Manual OAuth Token)

```bash
export AGSH_PROVIDER=claude-oauth
export AGSH_MODEL=claude-sonnet-4-20250514
export CLAUDE_OAUTH_TOKEN=sk-ant-oat01-...
agsh
```

On first run the OAuth token is saved to the database. Subsequent runs load it automatically; you no longer need the env var.

### Config File

```toml
[provider]
name = "claude-oauth"
model = "claude-sonnet-4-20250514"
```

## Authentication

### Setup Wizard (recommended)

`agsh setup` performs an OAuth Authorization Code flow with PKCE:

1. agsh generates a PKCE challenge and opens your browser to Claude's authorization page.
2. You authorize the application in your browser.
3. You paste the authorization code back into agsh.
4. agsh exchanges the code for access and refresh tokens.
5. Tokens are stored in the local database and refreshed automatically.

The OAuth client ID defaults to Claude Code's client ID but can be overridden via `CLAUDE_CLIENT_ID`.

### Token Lifecycle

1. Provide the initial token via setup wizard, env var, or config.
2. agsh saves it to the database on first use.
3. On subsequent launches the token is loaded from the database.
4. On expiry agsh refreshes the token automatically and updates the database.
5. Setting a new env var or config value replaces the stored token.

**Token refresh URL:** Defaults to `https://api.anthropic.com/v1/oauth/token`. Configurable via `provider.oauth_token_url` in the config file.

## Supported Models

Any model available through the Claude API:

- `claude-opus-4-20250514`
- `claude-sonnet-4-20250514`
- `claude-haiku-4-5-20251001`

## API Details

**Endpoint:** `POST {base_url}/v1/messages`

**Headers:**
- `Authorization: Bearer <oauth_token>`
- `anthropic-version: 2023-06-01`
- `anthropic-beta: claude-code-20250219,oauth-2025-04-20[,interleaved-thinking-2025-05-14]`
- `x-app: cli`
- `User-Agent: claude-cli/<version> (external, cli)`
- Stainless SDK identification headers
- `X-Claude-Code-Session-Id: <uuid>`

**System prompt:** Sent as an array of `text` blocks. The first block is a `x-anthropic-billing-header` with version, fingerprint, and an xxHash64 attestation token computed over the entire request body. The second is Claude Code's system prompt prefix; the third is your own prompt.

**Streaming:** Server-Sent Events with the same event types as [`claude-api`](./claude-api.md).
