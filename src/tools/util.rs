//! Small shared helpers for tool-input parsing and validation.

use std::path::PathBuf;

use crate::error::{AgshError, Result};

pub(super) fn require_str(
    input: &serde_json::Value,
    field: &str,
    tool_name: &str,
) -> Result<String> {
    input[field]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| AgshError::ToolExecution {
            tool_name: tool_name.to_string(),
            message: format!("missing '{}' parameter", field),
        })
}

/// Compile an LLM-supplied regex pattern with bounded compile memory so a
/// pathological pattern like `a{10_000_000}` cannot exhaust the host's RAM
/// during compilation. The `regex` crate's NFA/DFA engines already avoid
/// catastrophic backtracking at *match* time; the remaining DoS surface is
/// the one-time cost of building the automaton, which this bounds.
pub(super) fn compile_user_regex(pattern: &str, tool_name: &str) -> Result<regex::Regex> {
    const PATTERN_SIZE_LIMIT: usize = 1 << 20;
    const DFA_SIZE_LIMIT: usize = 1 << 20;

    regex::RegexBuilder::new(pattern)
        .size_limit(PATTERN_SIZE_LIMIT)
        .dfa_size_limit(DFA_SIZE_LIMIT)
        .build()
        .map_err(|error| AgshError::ToolExecution {
            tool_name: tool_name.to_string(),
            message: format!("invalid or oversized regex '{}': {}", pattern, error),
        })
}

/// Resolve the path the LLM provided to a canonical absolute path, with all
/// symlink components pre-resolved. Used by file tools to close a TOCTOU
/// window where a symlink in the supplied path could be swapped between the
/// permission check and the actual I/O. Callers should use the returned
/// `PathBuf` for every subsequent filesystem operation; never re-open the
/// original raw string.
///
/// When canonicalization fails (e.g. the target doesn't exist, for
/// `write_file`), returns the raw path unchanged so the caller can attempt
/// the operation and surface a natural `NotFound`/`PermissionDenied` error.
pub(super) async fn canonicalize_for_tool(raw: &str) -> PathBuf {
    tokio::fs::canonicalize(raw)
        .await
        .unwrap_or_else(|_| PathBuf::from(raw))
}

pub(super) fn truncate_string(string: &str, max_length: usize) -> &str {
    if string.len() <= max_length {
        string
    } else {
        &string[..string.floor_char_boundary(max_length)]
    }
}

/// Whether the caller is redirecting this tool's output into the scratchpad
/// via the `scratchpad` parameter. Tools that internally cap result counts or
/// output length should lift those caps when this returns true, because the
/// scratchpad is an overflow buffer and truncation defeats its purpose.
pub(super) fn redirects_to_scratchpad(input: &serde_json::Value) -> bool {
    input
        .get("scratchpad")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_string() {
        assert_eq!(truncate_string("hello", 10), "hello");
        assert_eq!(truncate_string("hello world", 5), "hello");
    }

    #[test]
    fn test_compile_user_regex_rejects_oversized() {
        // Pattern that compiles to a gigantic automaton; must be rejected by
        // the size limit rather than consume host memory.
        let result = compile_user_regex("a{10000000}", "test_tool");
        assert!(result.is_err(), "oversized pattern should be rejected");
    }

    #[test]
    fn test_compile_user_regex_accepts_normal_pattern() {
        let re = compile_user_regex(r"\d+", "test_tool").expect("normal pattern compiles");
        assert!(re.is_match("abc 123"));
    }

    #[test]
    fn test_compile_user_regex_rejects_invalid_syntax() {
        let result = compile_user_regex("[invalid", "test_tool");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_canonicalize_for_tool_resolves_existing() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let file_path = temp_dir.path().join("a.txt");
        std::fs::write(&file_path, "x").expect("write");

        let canonical = canonicalize_for_tool(file_path.to_str().expect("utf-8 path")).await;
        assert_eq!(
            canonical,
            std::fs::canonicalize(&file_path).expect("canonical")
        );
    }

    #[tokio::test]
    async fn test_canonicalize_for_tool_falls_back_on_missing() {
        let canonical = canonicalize_for_tool("/this/path/definitely/does/not/exist-xyzzy").await;
        assert_eq!(
            canonical,
            std::path::PathBuf::from("/this/path/definitely/does/not/exist-xyzzy")
        );
    }

    #[test]
    fn test_redirects_to_scratchpad() {
        assert!(redirects_to_scratchpad(
            &serde_json::json!({ "scratchpad": "img" })
        ));
        assert!(!redirects_to_scratchpad(
            &serde_json::json!({ "scratchpad": "" })
        ));
        assert!(!redirects_to_scratchpad(&serde_json::json!({})));
        assert!(!redirects_to_scratchpad(
            &serde_json::json!({ "from_scratchpad": "img" })
        ));
    }
}
