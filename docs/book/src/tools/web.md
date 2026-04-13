# Web Tools

## `fetch_url`

Fetch a web page and return its content as markdown text.

**Permission:** Read

### Parameters

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `url` | string | yes | The URL to fetch |
| `max_length` | integer | no | Maximum characters to return (default: 30000, 0 for no limit) |
| `headers` | object | no | Custom HTTP headers (overrides defaults like User-Agent) |
| `regex` | string | no | If provided, return only matching content (matches joined by newlines) |
| `raw` | boolean | no | Return raw HTML instead of converting to markdown (default: false) |
| `scratchpad` | string | no | Save output to the scratchpad under this name |

### Behavior

- Fetches the page via HTTP GET.
- Converts HTML to Markdown using `fast_html2md` (unless `raw` is true).
- Truncates the output to `max_length` characters (default: 30,000).
- HTTP timeout: 30 seconds.
- Returns the HTTP status code as an error if the request fails (e.g., 404, 500).

---

## `web_search`

Search the web and return results. Supports multiple search engines.

**Permission:** Read

### Parameters

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `query` | string | yes | The search query |
| `engine` | string | no | Search engine to use (default: `duckduckgo`) |
| `headers` | object | no | Custom HTTP headers |
| `scratchpad` | string | no | Save output to the scratchpad under this name |

### Search Engines

| Value | Engine |
|-------|--------|
| `duckduckgo` | DuckDuckGo (default) |
| `google` | Google Search |
| `bing` | Bing Search |

### Behavior

- Returns up to 10 results per search.
- Each result includes the title, URL, and a snippet (when available).
- Uses HTML scraping (no API keys required for any search engine).
- HTTP timeout: 30 seconds.
