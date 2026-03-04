# Web Tools

## `fetch_url`

Fetch a web page and return its content as markdown text.

**Permission:** Read

### Parameters

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `url` | string | yes | The URL to fetch |

### Behavior

- Fetches the page via HTTP GET.
- Converts HTML to Markdown using `fast_html2md`.
- Truncates the output to 50,000 characters if the page is very large.
- HTTP timeout: 30 seconds.
- Returns the HTTP status code as an error if the request fails (e.g., 404, 500).

### Examples

```text
agsh [r] > fetch the Rust homepage and summarize what's new
```

```text
agsh [r] > read the documentation at https://docs.rs/tokio/latest/tokio/
```

---

## `web_search`

Search the web and return results. Supports multiple search engines.

**Permission:** Read

### Parameters

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `query` | string | yes | The search query |
| `engine` | string | no | Search engine to use (default: `duckduckgo`) |

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
- HTTP timeout: 15 seconds.

### Examples

```text
agsh [r] > search the web for "rust async tutorial"
```

```text
agsh [r] > search google for the latest news about WebAssembly
```

```text
agsh [r] > use bing to search for "tokio vs async-std comparison"
```
