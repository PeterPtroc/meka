use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::error::{AgshError, Result};
use crate::permission::Permission;
use crate::provider::ToolDefinition;

use super::util::require_str;
use super::{Tool, ToolOutput};

pub(super) struct FindFilesTool;

#[async_trait]
impl Tool for FindFilesTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "find_files".to_string(),
            description: "Find files matching a glob pattern (e.g., '**/*.rs', 'src/*.txt'). \
                          Avoid overly broad searches: scanning a large tree can take \
                          a long time and will hit many directories the user has no \
                          read permission for, producing noisy errors. Start with the \
                          smallest `path` and most specific pattern that plausibly \
                          contains the answer; if that returns nothing, widen the \
                          `path` by one level or loosen the pattern, and repeat. Only \
                          fall back to a tree-wide scan if targeted attempts have all \
                          failed."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern to match files against. Prefer narrow patterns over broad ones like `**/*`."
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search in. Defaults to current directory. Prefer the smallest subtree that can answer the question."
                    },
                    "scratchpad": {
                        "type": "string",
                        "description": "If provided, save the output to the scratchpad under this name instead of returning it inline."
                    }
                },
                "required": ["pattern"]
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
        let pattern = require_str(&input, "pattern", "find_files")?;
        let base_path = input["path"].as_str().map(|s| s.to_string());

        let full_pattern = match &base_path {
            Some(base) => format!("{}/{}", base.trim_end_matches('/'), pattern),
            None => pattern.clone(),
        };

        const MAX_RESULTS: usize = 200;

        let result = tokio::task::spawn_blocking(move || {
            let mut matches = Vec::new();
            let mut truncated = false;
            match glob::glob(&full_pattern) {
                Ok(paths) => {
                    for entry in paths {
                        match entry {
                            Ok(path) => {
                                matches.push(path.display().to_string());
                                if matches.len() >= MAX_RESULTS {
                                    truncated = true;
                                    break;
                                }
                            }
                            Err(error) => {
                                tracing::warn!("glob error: {}", error);
                            }
                        }
                    }
                }
                Err(error) => {
                    return Err(AgshError::ToolExecution {
                        tool_name: "find_files".to_string(),
                        message: format!("invalid glob pattern '{}': {}", full_pattern, error),
                    });
                }
            }
            Ok((matches, truncated))
        })
        .await
        .map_err(|error| AgshError::ToolExecution {
            tool_name: "find_files".to_string(),
            message: format!("task join error: {}", error),
        })??;

        let (matches, truncated) = result;
        if matches.is_empty() {
            Ok(ToolOutput::text(
                "No files found matching the pattern.".to_string(),
                false,
            ))
        } else {
            let mut output = matches.join("\n");
            if truncated {
                output.push_str(&format!(
                    "\n\n... (truncated, showing first {} results)",
                    MAX_RESULTS
                ));
            }
            Ok(ToolOutput::text(output, false))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ContentBlock;

    fn text_content(output: &ToolOutput) -> String {
        ContentBlock::tool_result_text_content(&output.content)
    }

    #[tokio::test]
    async fn test_find_files() {
        let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
        std::fs::write(temp_dir.path().join("a.txt"), "").expect("failed");
        std::fs::write(temp_dir.path().join("b.txt"), "").expect("failed");
        std::fs::write(temp_dir.path().join("c.rs"), "").expect("failed");

        let tool = FindFilesTool;
        let result = tool
            .execute(
                serde_json::json!({
                    "pattern": "*.txt",
                    "path": temp_dir.path().to_str().expect("path")
                }),
                CancellationToken::new(),
            )
            .await
            .expect("should succeed");

        assert!(!result.is_error);
        assert!(text_content(&result).contains("a.txt"));
        assert!(text_content(&result).contains("b.txt"));
        assert!(!text_content(&result).contains("c.rs"));
    }
}
