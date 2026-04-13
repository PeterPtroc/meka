use std::sync::Arc;

use async_trait::async_trait;
use regex::Regex;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::{AgshError, Result};
use crate::permission::Permission;
use crate::provider::{ContentBlock, ToolDefinition, ToolResultContent};
use crate::session::SessionManager;

use super::{Tool, ToolOutput};

/// Tool result text blocks larger than this are persisted to the database and
/// replaced with a preview + handle in the conversation context.
pub const MAX_INLINE_RESULT_CHARS: usize = 30_000;

/// Number of characters included in the inline preview.
const PREVIEW_CHARS: usize = 2_000;

/// Default character limit when reading back a persisted output.
const DEFAULT_READ_LIMIT: usize = 30_000;

/// Maximum number of regex matches returned in search mode.
const MAX_SEARCH_MATCHES: usize = 100;

fn format_size(bytes: usize) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1_024 {
        format!("{:.1} KB", bytes as f64 / 1_024.0)
    } else {
        format!("{} bytes", bytes)
    }
}

/// Check each text block in tool results. If oversized, persist to DB
/// and replace with a preview + handle.
pub async fn persist_oversized_results(
    session_manager: &SessionManager,
    session_id: Uuid,
    results: &mut Vec<ContentBlock>,
) -> Result<()> {
    for block in results.iter_mut() {
        if let ContentBlock::ToolResult { content, .. } = block {
            for item in content.iter_mut() {
                if let ToolResultContent::Text { text } = item {
                    if text.len() <= MAX_INLINE_RESULT_CHARS {
                        continue;
                    }

                    let output_id = session_manager.save_tool_output(session_id, text).await?;

                    let size = text.len();
                    let preview_end = text.floor_char_boundary(PREVIEW_CHARS.min(size));
                    let preview = &text[..preview_end];
                    let has_more = preview_end < size;

                    let mut replacement = format!(
                        "<large-output id=\"{}\" size=\"{}\">\n\
                         Output too large ({}). Use read_stash to access full content.\n\n\
                         Preview (first {} characters):\n\
                         {}",
                        output_id,
                        size,
                        format_size(size),
                        preview_end,
                        preview,
                    );
                    if has_more {
                        replacement.push_str("\n...");
                    }
                    replacement.push_str("\n</large-output>");

                    *text = replacement;
                }
            }
        }
    }
    Ok(())
}

pub(super) struct ReadStashTool {
    pub session_manager: SessionManager,
    pub session_id: Arc<RwLock<Option<Uuid>>>,
}

#[async_trait]
impl Tool for ReadStashTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "read_stash".to_string(),
            description: "Read or search a large tool output that was persisted because it \
                exceeded the inline size limit. Use this to access content referenced by \
                <large-output> tags."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "integer",
                        "description": "The output ID from the <large-output> tag"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Character offset to start reading from. Default: 0."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum characters to return. Default: 30000."
                    },
                    "regex": {
                        "type": "string",
                        "description": "If provided, search the output with this regex pattern \
                            and return matching lines (max 100 matches) instead of a character range."
                    }
                },
                "required": ["id"]
            }),
        }
    }

    fn required_permission(&self) -> Permission {
        Permission::Read
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _cancellation: CancellationToken,
    ) -> Result<ToolOutput> {
        let output_id = input["id"]
            .as_i64()
            .ok_or_else(|| AgshError::ToolExecution {
                tool_name: "read_stash".to_string(),
                message: "missing or invalid 'id' parameter".to_string(),
            })?;

        let session_id = {
            let guard = self.session_id.read().await;
            guard.ok_or_else(|| AgshError::ToolExecution {
                tool_name: "read_stash".to_string(),
                message: "no active session".to_string(),
            })?
        };

        let content = self
            .session_manager
            .load_tool_output(session_id, output_id)
            .await?
            .ok_or_else(|| AgshError::ToolExecution {
                tool_name: "read_stash".to_string(),
                message: format!("output id {} not found in current session", output_id),
            })?;

        if let Some(pattern) = input.get("regex").and_then(|v| v.as_str()) {
            return search_mode(&content, pattern);
        }

        read_mode(&content, &input)
    }
}

fn read_mode(content: &str, input: &serde_json::Value) -> Result<ToolOutput> {
    let offset = input["offset"].as_u64().unwrap_or(0) as usize;
    let limit = input["limit"].as_u64().unwrap_or(DEFAULT_READ_LIMIT as u64) as usize;
    let total = content.len();

    if offset >= total {
        return Ok(ToolOutput::text(
            format!(
                "Offset {} exceeds content length ({} characters)",
                offset, total
            ),
            true,
        ));
    }

    let start = content.floor_char_boundary(offset);
    let end = content.floor_char_boundary((start + limit).min(total));
    let slice = &content[start..end];

    let result = format!(
        "{}\n\n(showing characters {}..{} of {})",
        slice, start, end, total
    );

    Ok(ToolOutput::text(result, false))
}

fn search_mode(content: &str, pattern: &str) -> Result<ToolOutput> {
    let re = Regex::new(pattern).map_err(|error| AgshError::ToolExecution {
        tool_name: "read_stash".to_string(),
        message: format!("invalid regex '{}': {}", pattern, error),
    })?;

    let mut matches = Vec::new();
    for (line_num, line) in content.lines().enumerate() {
        if re.is_match(line) {
            matches.push(format!("{}:{}", line_num + 1, line));
            if matches.len() >= MAX_SEARCH_MATCHES {
                break;
            }
        }
    }

    if matches.is_empty() {
        return Ok(ToolOutput::text(
            "No matches found for the given regex pattern.".to_string(),
            false,
        ));
    }

    let total_matches = if matches.len() >= MAX_SEARCH_MATCHES {
        let remaining: usize = content
            .lines()
            .skip(matches.len())
            .filter(|line| re.is_match(line))
            .count();
        matches.len() + remaining
    } else {
        matches.len()
    };

    let mut result = matches.join("\n");
    if total_matches > MAX_SEARCH_MATCHES {
        result.push_str(&format!(
            "\n\n... (showing first {} of {} matches)",
            MAX_SEARCH_MATCHES, total_matches,
        ));
    }

    Ok(ToolOutput::text(result, false))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::provider::ContentBlock;

    fn text_content(output: &ToolOutput) -> String {
        ContentBlock::tool_result_text_content(&output.content)
    }

    async fn test_manager() -> SessionManager {
        SessionManager::open(Some(Path::new(":memory:")))
            .await
            .expect("failed to open in-memory database")
    }

    #[tokio::test]
    async fn test_persist_oversized_results_replaces_large_text() {
        let manager = test_manager().await;
        let session_id = manager.create_session().await.expect("create session");

        let large_text = "x".repeat(MAX_INLINE_RESULT_CHARS + 1000);
        let mut results = vec![ContentBlock::ToolResult {
            tool_use_id: "test-id".to_string(),
            content: vec![ToolResultContent::Text {
                text: large_text.clone(),
            }],
            is_error: false,
        }];

        persist_oversized_results(&manager, session_id, &mut results)
            .await
            .expect("persist");

        if let ContentBlock::ToolResult { content, .. } = &results[0] {
            let text = ContentBlock::tool_result_text_content(content);
            assert!(text.contains("<large-output"));
            assert!(text.contains("</large-output>"));
            assert!(text.contains("read_stash"));
            assert!(!text.contains(&large_text));
        } else {
            panic!("expected ToolResult");
        }

        // Verify the full content was stored in DB
        let stored = manager
            .load_all_tool_outputs(session_id)
            .await
            .expect("load");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].1, large_text);
    }

    #[tokio::test]
    async fn test_persist_oversized_results_leaves_small_text() {
        let manager = test_manager().await;
        let session_id = manager.create_session().await.expect("create session");

        let small_text = "hello world".to_string();
        let mut results = vec![ContentBlock::ToolResult {
            tool_use_id: "test-id".to_string(),
            content: vec![ToolResultContent::Text {
                text: small_text.clone(),
            }],
            is_error: false,
        }];

        persist_oversized_results(&manager, session_id, &mut results)
            .await
            .expect("persist");

        if let ContentBlock::ToolResult { content, .. } = &results[0] {
            let text = ContentBlock::tool_result_text_content(content);
            assert_eq!(text, small_text);
        } else {
            panic!("expected ToolResult");
        }
    }

    #[tokio::test]
    async fn test_read_stash_read_mode() {
        let manager = test_manager().await;
        let session_id = manager.create_session().await.expect("create session");

        let content = "line1\nline2\nline3\nline4\nline5\n";
        let output_id = manager
            .save_tool_output(session_id, content)
            .await
            .expect("save");

        let tool = ReadStashTool {
            session_manager: manager,
            session_id: Arc::new(RwLock::new(Some(session_id))),
        };

        let result = tool
            .execute(
                serde_json::json!({"id": output_id}),
                CancellationToken::new(),
            )
            .await
            .expect("execute");

        assert!(!result.is_error);
        let text = text_content(&result);
        assert!(text.contains("line1"));
        assert!(text.contains("line5"));
    }

    #[tokio::test]
    async fn test_read_stash_with_offset_and_limit() {
        let manager = test_manager().await;
        let session_id = manager.create_session().await.expect("create session");

        let content = "abcdefghij".to_string();
        let output_id = manager
            .save_tool_output(session_id, &content)
            .await
            .expect("save");

        let tool = ReadStashTool {
            session_manager: manager,
            session_id: Arc::new(RwLock::new(Some(session_id))),
        };

        let result = tool
            .execute(
                serde_json::json!({"id": output_id, "offset": 3, "limit": 4}),
                CancellationToken::new(),
            )
            .await
            .expect("execute");

        let text = text_content(&result);
        assert!(text.contains("defg"));
        assert!(text.contains("showing characters 3..7 of 10"));
    }

    #[tokio::test]
    async fn test_read_stash_search_mode() {
        let manager = test_manager().await;
        let session_id = manager.create_session().await.expect("create session");

        let content = "apple\nbanana\napricot\ncherry\navocado\n";
        let output_id = manager
            .save_tool_output(session_id, content)
            .await
            .expect("save");

        let tool = ReadStashTool {
            session_manager: manager,
            session_id: Arc::new(RwLock::new(Some(session_id))),
        };

        let result = tool
            .execute(
                serde_json::json!({"id": output_id, "regex": "^a"}),
                CancellationToken::new(),
            )
            .await
            .expect("execute");

        let text = text_content(&result);
        assert!(text.contains("1:apple"));
        assert!(text.contains("3:apricot"));
        assert!(text.contains("5:avocado"));
        assert!(!text.contains("banana"));
        assert!(!text.contains("cherry"));
    }

    #[tokio::test]
    async fn test_read_stash_invalid_id() {
        let manager = test_manager().await;
        let session_id = manager.create_session().await.expect("create session");

        let tool = ReadStashTool {
            session_manager: manager,
            session_id: Arc::new(RwLock::new(Some(session_id))),
        };

        let result = tool.execute(serde_json::json!({"id": 99999}), CancellationToken::new());
        assert!(result.await.is_err());
    }

    #[tokio::test]
    async fn test_delete_session_removes_tool_outputs() {
        let manager = test_manager().await;
        let session_id = manager.create_session().await.expect("create session");

        manager
            .save_tool_output(session_id, "stored output")
            .await
            .expect("save");

        manager.delete_session(session_id).await.expect("delete");

        let outputs = manager
            .load_all_tool_outputs(session_id)
            .await
            .expect("load");
        assert!(outputs.is_empty());
    }

    #[tokio::test]
    async fn test_clear_messages_removes_tool_outputs() {
        let manager = test_manager().await;
        let session_id = manager.create_session().await.expect("create session");

        manager
            .save_tool_output(session_id, "stored output")
            .await
            .expect("save");

        manager.clear_messages(session_id).await.expect("clear");

        let outputs = manager
            .load_all_tool_outputs(session_id)
            .await
            .expect("load");
        assert!(outputs.is_empty());

        // Session itself should still exist
        assert!(manager.session_exists(session_id).await.expect("exists"));
    }

    #[tokio::test]
    async fn test_save_and_load_tool_output_roundtrip() {
        let manager = test_manager().await;
        let session_id = manager.create_session().await.expect("create session");

        let content = "test output content";
        let output_id = manager
            .save_tool_output(session_id, content)
            .await
            .expect("save");

        let loaded = manager
            .load_tool_output(session_id, output_id)
            .await
            .expect("load");
        assert_eq!(loaded, Some(content.to_string()));
    }

    #[tokio::test]
    async fn test_load_all_tool_outputs() {
        let manager = test_manager().await;
        let session_id = manager.create_session().await.expect("create session");

        let id1 = manager
            .save_tool_output(session_id, "first")
            .await
            .expect("save");
        let id2 = manager
            .save_tool_output(session_id, "second")
            .await
            .expect("save");

        let all = manager
            .load_all_tool_outputs(session_id)
            .await
            .expect("load");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0], (id1, "first".to_string()));
        assert_eq!(all[1], (id2, "second".to_string()));
    }

    #[tokio::test]
    async fn test_load_tool_output_wrong_session() {
        let manager = test_manager().await;
        let session1 = manager.create_session().await.expect("create");
        let session2 = manager.create_session().await.expect("create");

        let output_id = manager
            .save_tool_output(session1, "belongs to session1")
            .await
            .expect("save");

        let loaded = manager
            .load_tool_output(session2, output_id)
            .await
            .expect("load");
        assert_eq!(loaded, None);
    }
}
