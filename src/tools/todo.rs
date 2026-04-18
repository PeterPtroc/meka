//! `todo_write` tool and the shared todo list state. Lets the agent track
//! multi-step plans and surface their progress in the system prompt and the
//! REPL display.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::error::{AgshError, Result};
use crate::permission::Permission;
use crate::provider::ToolDefinition;
use crate::render;

use super::{Tool, ToolOutput};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub description: String,
    pub status: String,
}

pub type SharedTodoList = Arc<RwLock<Vec<TodoItem>>>;

pub(super) struct TodoWriteTool {
    pub todo_list: SharedTodoList,
}

#[async_trait]
impl Tool for TodoWriteTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "todo_write".to_string(),
            description: "Create or update a structured task list. Replaces the entire list each \
                          call. Use this to break down multi-step work, track progress, and \
                          communicate status."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "tasks": {
                        "type": "array",
                        "description": "The complete task list (replaces any existing list)",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": {
                                    "type": "string",
                                    "description": "Unique task identifier"
                                },
                                "description": {
                                    "type": "string",
                                    "description": "What needs to be done"
                                },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "done"],
                                    "description": "Task status"
                                }
                            },
                            "required": ["id", "description", "status"]
                        }
                    }
                },
                "required": ["tasks"]
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
        let tasks_value = input.get("tasks").ok_or_else(|| AgshError::ToolExecution {
            tool_name: "todo_write".to_string(),
            message: "missing 'tasks' parameter".to_string(),
        })?;

        let tasks: Vec<TodoItem> =
            serde_json::from_value(tasks_value.clone()).map_err(|error| {
                AgshError::ToolExecution {
                    tool_name: "todo_write".to_string(),
                    message: format!("invalid tasks format: {}", error),
                }
            })?;

        let count = tasks.len();
        *self.todo_list.write().await = tasks.clone();

        render::render_todo_list(&tasks);

        Ok(ToolOutput::text(
            format!("Task list updated ({} tasks)", count),
            false,
        ))
    }
}

pub fn format_todo_for_context(items: &[TodoItem]) -> String {
    let mut output = String::from("[Current task list]\n");
    for item in items {
        let marker = match item.status.as_str() {
            "done" => "[x]",
            "in_progress" => "[~]",
            _ => "[ ]",
        };
        output.push_str(&format!("{} {} - {}\n", marker, item.id, item.description));
    }
    output.push('\n');
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ContentBlock;

    fn text_content(output: &ToolOutput) -> String {
        ContentBlock::tool_result_text_content(&output.content)
    }

    fn test_list() -> SharedTodoList {
        Arc::new(RwLock::new(Vec::new()))
    }

    #[tokio::test]
    async fn test_todo_write() {
        let list = test_list();
        let tool = TodoWriteTool {
            todo_list: list.clone(),
        };

        let result = tool
            .execute(
                serde_json::json!({
                    "tasks": [
                        {"id": "1", "description": "First task", "status": "pending"},
                        {"id": "2", "description": "Second task", "status": "done"}
                    ]
                }),
                CancellationToken::new(),
            )
            .await
            .expect("should succeed");

        assert!(!result.is_error);
        assert!(text_content(&result).contains("2 tasks"));

        let items = list.read().await;
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].status, "pending");
        assert_eq!(items[1].status, "done");
    }

    #[tokio::test]
    async fn test_todo_write_replaces_list() {
        let list = test_list();
        let tool = TodoWriteTool {
            todo_list: list.clone(),
        };

        tool.execute(
            serde_json::json!({
                "tasks": [{"id": "1", "description": "Old", "status": "pending"}]
            }),
            CancellationToken::new(),
        )
        .await
        .expect("should succeed");

        tool.execute(
            serde_json::json!({
                "tasks": [
                    {"id": "a", "description": "New A", "status": "in_progress"},
                    {"id": "b", "description": "New B", "status": "done"}
                ]
            }),
            CancellationToken::new(),
        )
        .await
        .expect("should succeed");

        let items = list.read().await;
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "a");
        assert_eq!(items[1].id, "b");
    }

    #[test]
    fn test_format_todo_for_context() {
        let items = vec![
            TodoItem {
                id: "1".to_string(),
                description: "Do something".to_string(),
                status: "pending".to_string(),
            },
            TodoItem {
                id: "2".to_string(),
                description: "Working on it".to_string(),
                status: "in_progress".to_string(),
            },
            TodoItem {
                id: "3".to_string(),
                description: "Already done".to_string(),
                status: "done".to_string(),
            },
        ];

        let output = format_todo_for_context(&items);
        assert!(output.contains("[ ] 1 - Do something"));
        assert!(output.contains("[~] 2 - Working on it"));
        assert!(output.contains("[x] 3 - Already done"));
    }

    #[test]
    fn test_format_todo_empty() {
        let output = format_todo_for_context(&[]);
        assert!(output.contains("[Current task list]"));
    }
}
