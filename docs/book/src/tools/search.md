# Search Tools

## `find_files`

Find files matching a glob pattern.

**Permission:** Read

### Parameters

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `pattern` | string | yes | Glob pattern to match files against |
| `path` | string | no | Directory to search in (defaults to current directory) |

### Glob Patterns

| Pattern | Matches |
|---------|---------|
| `*.rs` | All `.rs` files in the current directory |
| `**/*.rs` | All `.rs` files recursively |
| `src/*.txt` | All `.txt` files in `src/` |
| `test_*` | All files starting with `test_` |

### Examples

```text
agsh [r] > find all Rust source files in this project
```

```text
agsh [r] > find all configuration files (*.toml, *.yaml, *.json) in this repo
```

---

## `search_contents`

Search file contents using a regex pattern. Powered by the ripgrep library.

**Permission:** Read

### Parameters

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `pattern` | string | yes | Regex pattern to search for |
| `path` | string | no | File or directory to search in (defaults to current directory) |
| `glob` | string | no | Glob pattern to filter which files are searched (e.g., `*.rs`) |

### Behavior

- Searches recursively through directories.
- Skips hidden files (starting with `.`) and common non-text directories (`target`, `node_modules`).
- Results are limited to 100 matches to avoid overwhelming the LLM context.
- Each result includes the file path, line number, and matching line.

### Examples

```text
agsh [r] > search for all TODO comments in this project
```

```text
agsh [r] > find all uses of "unwrap()" in Rust files
```

The agent will call `search_contents` with `pattern: "unwrap\\(\\)"` and `glob: "*.rs"`.

```text
agsh [r] > search for the string "API_KEY" in all .env files
```
