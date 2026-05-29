//! The `todo` tool and the shared task-list state. A single tool the agent uses to track multi-step
//! work: it both mutates the list and echoes the canonical state back on every call, so the model
//! always has authoritative task numbers for its next update. The list is also surfaced in the
//! per-turn context block and the REPL display.

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use super::{Tool, ToolOutput};
use crate::{error::Result, permission::Permission, provider::ToolDefinition};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    #[serde(
        alias = "wip",
        alias = "in-progress",
        alias = "in progress",
        alias = "started"
    )]
    InProgress,
    #[serde(alias = "done", alias = "complete", alias = "finished")]
    Completed,
    #[serde(
        alias = "canceled",
        alias = "skipped",
        alias = "dropped",
        alias = "wontfix"
    )]
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub text: String,
    pub status: TodoStatus,
}

/// The full task-list state: a `title` (set by the agent when it builds the list and rendered as
/// the heading) and the ordered items. Task numbers are positional (1-based) and owned by the tool,
/// so they are derived from order rather than stored on the item.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TodoState {
    pub title: Option<String>,
    pub items: Vec<TodoItem>,
}

pub type SharedTodoList = Arc<RwLock<TodoState>>;

/// One element of the `items` array: either a bare task string (status defaults to `pending`) or an
/// object carrying an explicit status. `Text` must come first so a JSON string matches it before
/// the object variant is tried.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum TodoItemInput {
    Text(String),
    Full {
        text: String,
        #[serde(default)]
        status: Option<TodoStatus>,
    },
}

impl From<TodoItemInput> for TodoItem {
    fn from(input: TodoItemInput) -> Self {
        match input {
            TodoItemInput::Text(text) => TodoItem {
                text,
                status: TodoStatus::Pending,
            },
            TodoItemInput::Full { text, status } => TodoItem {
                text,
                status: status.unwrap_or(TodoStatus::Pending),
            },
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TodoInput {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    items: Option<Vec<TodoItemInput>>,
    #[serde(default)]
    set: Option<BTreeMap<String, TodoStatus>>,
}

pub(super) struct TodoTool {
    pub todo_list: SharedTodoList,
}

#[async_trait]
impl Tool for TodoTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "todo".to_string(),
            description: "Track and display progress on multi-step work. The full list (with task \
                          numbers) is returned after every call, so you never need a separate \
                          read.\n\
                          - Set up or restructure: pass `items` to (re)create the whole list, \
                          together with a `title` summarizing the overall goal (the `title` is \
                          required whenever you pass `items`). Each entry is a task string (status \
                          defaults to pending) or an object {\"text\":..., \"status\":...}. Tasks \
                          are numbered 1..N in order.\n\
                          - As you work: pass `set` to flip statuses by task number, e.g. \
                          {\"1\":\"completed\",\"2\":\"in_progress\"} — this is the common case; do \
                          it as you start and finish each step.\n\
                          Call with no arguments to just read the current list. Keep exactly one \
                          task in_progress; mark a task completed only when truly done, or \
                          cancelled if you drop it."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "A short heading summarizing the overall goal of the list. \
                                        Required whenever you pass `items`; persists across later \
                                        `set` updates."
                    },
                    "items": {
                        "type": "array",
                        "description": "Replace the whole list. Each entry is a task string \
                                        (status defaults to pending) or an object {text, status}. \
                                        Tasks are numbered 1..N in order.",
                        "items": {
                            "anyOf": [
                                { "type": "string" },
                                {
                                    "type": "object",
                                    "properties": {
                                        "text": {
                                            "type": "string",
                                            "description": "What needs to be done"
                                        },
                                        "status": {
                                            "type": "string",
                                            "enum": ["pending", "in_progress", "completed", "cancelled"]
                                        }
                                    },
                                    "required": ["text"]
                                }
                            ]
                        }
                    },
                    "set": {
                        "type": "object",
                        "description": "Sparse status update keyed by 1-based task number, e.g. \
                                        {\"1\":\"completed\",\"2\":\"in_progress\"}.",
                        "additionalProperties": {
                            "type": "string",
                            "enum": ["pending", "in_progress", "completed", "cancelled"]
                        }
                    }
                }
            }),
            ..Default::default()
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
        let parsed: TodoInput = match serde_json::from_value(input) {
            Ok(parsed) => parsed,
            Err(error) => {
                return Ok(ToolOutput::text(
                    format!("invalid todo input: {}", error),
                    true,
                ));
            }
        };

        let mut state = self.todo_list.write().await;

        let title = parsed
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty());

        // Precedence: items (replace) -> set (patch by resulting position).
        if let Some(items) = parsed.items {
            let new_items: Vec<TodoItem> = items.into_iter().map(TodoItem::from).collect();
            // A non-empty list needs a heading; the model must name what it's working towards.
            if !new_items.is_empty() && title.is_none() {
                return Ok(ToolOutput::text(
                    "todo: a `title` is required when creating or replacing the task list"
                        .to_string(),
                    true,
                ));
            }
            state.title = title.map(str::to_string);
            state.items = new_items;
        } else if let Some(title) = title {
            // Rename without rebuilding the list.
            state.title = Some(title.to_string());
        }

        if let Some(set) = parsed.set {
            let len = state.items.len();
            // Validate every key before mutating so a single bad id leaves the list untouched.
            let mut patches = Vec::with_capacity(set.len());
            for (key, status) in set {
                match key.parse::<usize>() {
                    Ok(id) if (1..=len).contains(&id) => patches.push((id, status)),
                    _ => {
                        return Ok(ToolOutput::text(
                            format!(
                                "todo set: '{}' is not a valid task number (the list has {} \
                                 task{})",
                                key,
                                len,
                                if len == 1 { "" } else { "s" }
                            ),
                            true,
                        ));
                    }
                }
            }
            for (id, status) in patches {
                if let Some(item) = state.items.get_mut(id - 1) {
                    item.status = status;
                }
            }
        }

        Ok(ToolOutput::text(format_todo_state(&state), false))
    }
}

/// Render the task list as plain text (no ANSI), used both as the `todo` tool result echoed to the
/// model and in the per-turn / post-compaction context blocks. Terminal colouring lives separately
/// in `render::render_todo_list`.
pub fn format_todo_state(state: &TodoState) -> String {
    if state.items.is_empty() {
        return "(no tasks)\n".to_string();
    }

    // Heading is `TODO: <title>` (defensive fallback when somehow absent), followed by a blank line
    // and the tasks as a markdown checklist.
    let title = state.title.as_deref().unwrap_or("Tasks");
    let mut output = format!("TODO: {}\n\n", title);

    for (index, item) in state.items.iter().enumerate() {
        let number = index + 1;
        let marker = match item.status {
            TodoStatus::Pending => "[ ]",
            TodoStatus::InProgress => "[~]",
            TodoStatus::Completed => "[x]",
            TodoStatus::Cancelled => "[-]",
        };
        if item.status == TodoStatus::Cancelled {
            output.push_str(&format!(
                "- {} {} (cancelled) {}\n",
                marker, number, item.text
            ));
        } else {
            output.push_str(&format!("- {} {} {}\n", marker, number, item.text));
        }
    }

    // Soft-invariant footer: report violations of the "exactly one in_progress" convention without
    // blocking the call. The model self-corrects on its next update.
    let in_progress: Vec<usize> = state
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.status == TodoStatus::InProgress)
        .map(|(index, _)| index + 1)
        .collect();
    if in_progress.len() > 1 {
        let ids = in_progress
            .iter()
            .map(|id| format!("#{}", id))
            .collect::<Vec<_>>()
            .join(", ");
        output.push_str(&format!(
            "(!) tasks {} are all in_progress — keep exactly one in_progress at a time\n",
            ids
        ));
    } else if in_progress.is_empty()
        && state
            .items
            .iter()
            .any(|item| item.status == TodoStatus::Pending)
    {
        output.push_str("(!) no task in_progress — set the next task to in_progress\n");
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::tests::text_content;

    fn test_list() -> SharedTodoList {
        Arc::new(RwLock::new(TodoState::default()))
    }

    fn tool() -> (TodoTool, SharedTodoList) {
        let list = test_list();
        (
            TodoTool {
                todo_list: list.clone(),
            },
            list,
        )
    }

    #[tokio::test]
    async fn test_empty_call_reads() {
        let (tool, _list) = tool();
        let result = tool
            .execute(serde_json::json!({}), CancellationToken::new())
            .await
            .expect("empty call should succeed");
        assert!(!result.is_error);
        assert!(text_content(&result).contains("(no tasks)"));
    }

    #[tokio::test]
    async fn test_items_as_strings_default_pending() {
        let (tool, list) = tool();
        tool.execute(
            serde_json::json!({ "title": "Setup", "items": ["First", "Second"] }),
            CancellationToken::new(),
        )
        .await
        .expect("should succeed");

        let state = list.read().await;
        assert_eq!(state.title.as_deref(), Some("Setup"));
        assert_eq!(state.items.len(), 2);
        assert_eq!(state.items[0].text, "First");
        assert_eq!(state.items[0].status, TodoStatus::Pending);
        assert_eq!(state.items[1].status, TodoStatus::Pending);
    }

    #[tokio::test]
    async fn test_items_as_objects_honor_status() {
        let (tool, list) = tool();
        tool.execute(
            serde_json::json!({
                "title": "Work",
                "items": [
                    {"text": "A", "status": "in_progress"},
                    {"text": "B"}
                ]
            }),
            CancellationToken::new(),
        )
        .await
        .expect("should succeed");

        let state = list.read().await;
        assert_eq!(state.items[0].status, TodoStatus::InProgress);
        assert_eq!(state.items[1].status, TodoStatus::Pending);
    }

    #[tokio::test]
    async fn test_status_aliases() {
        let (tool, list) = tool();
        tool.execute(
            serde_json::json!({
                "title": "Work",
                "items": [
                    {"text": "A", "status": "done"},
                    {"text": "B", "status": "wip"},
                    {"text": "C", "status": "skipped"}
                ]
            }),
            CancellationToken::new(),
        )
        .await
        .expect("should succeed");

        let state = list.read().await;
        assert_eq!(state.items[0].status, TodoStatus::Completed);
        assert_eq!(state.items[1].status, TodoStatus::InProgress);
        assert_eq!(state.items[2].status, TodoStatus::Cancelled);
    }

    #[tokio::test]
    async fn test_title_required_when_building() {
        let (tool, list) = tool();
        let result = tool
            .execute(
                serde_json::json!({ "items": ["A", "B"] }),
                CancellationToken::new(),
            )
            .await
            .expect("call returns is_error, not Err");
        assert!(result.is_error);
        assert!(text_content(&result).contains("title"));
        assert!(list.read().await.items.is_empty(), "list must be untouched");
    }

    #[tokio::test]
    async fn test_title_persists_across_set() {
        let (tool, list) = tool();
        tool.execute(
            serde_json::json!({ "title": "Refactor", "items": ["A", "B"] }),
            CancellationToken::new(),
        )
        .await
        .expect("should succeed");
        tool.execute(
            serde_json::json!({ "set": {"1": "completed"} }),
            CancellationToken::new(),
        )
        .await
        .expect("should succeed");
        assert_eq!(list.read().await.title.as_deref(), Some("Refactor"));
    }

    #[tokio::test]
    async fn test_set_patches_by_position() {
        let (tool, list) = tool();
        tool.execute(
            serde_json::json!({ "title": "Work", "items": ["A", "B", "C"] }),
            CancellationToken::new(),
        )
        .await
        .expect("should succeed");
        tool.execute(
            serde_json::json!({ "set": {"1": "completed", "2": "in_progress"} }),
            CancellationToken::new(),
        )
        .await
        .expect("should succeed");

        let state = list.read().await;
        assert_eq!(state.items[0].status, TodoStatus::Completed);
        assert_eq!(state.items[1].status, TodoStatus::InProgress);
        assert_eq!(state.items[2].status, TodoStatus::Pending);
    }

    #[tokio::test]
    async fn test_set_out_of_range_is_error_and_no_mutation() {
        let (tool, list) = tool();
        tool.execute(
            serde_json::json!({ "title": "Work", "items": ["A"] }),
            CancellationToken::new(),
        )
        .await
        .expect("should succeed");
        let result = tool
            .execute(
                serde_json::json!({ "set": {"9": "completed"} }),
                CancellationToken::new(),
            )
            .await
            .expect("call returns is_error, not Err");

        assert!(result.is_error);
        assert!(text_content(&result).contains("valid task number"));
        assert_eq!(list.read().await.items[0].status, TodoStatus::Pending);
    }

    #[tokio::test]
    async fn test_set_non_numeric_key_is_error() {
        let (tool, _list) = tool();
        tool.execute(
            serde_json::json!({ "title": "Work", "items": ["A"] }),
            CancellationToken::new(),
        )
        .await
        .expect("should succeed");
        let result = tool
            .execute(
                serde_json::json!({ "set": {"abc": "completed"} }),
                CancellationToken::new(),
            )
            .await
            .expect("call returns is_error, not Err");
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_items_then_set_precedence_in_one_call() {
        let (tool, list) = tool();
        tool.execute(
            serde_json::json!({
                "title": "Work",
                "items": ["A", "B", "C"],
                "set": {"2": "completed"}
            }),
            CancellationToken::new(),
        )
        .await
        .expect("should succeed");

        let state = list.read().await;
        assert_eq!(state.items.len(), 3);
        assert_eq!(state.items[1].status, TodoStatus::Completed);
    }

    #[tokio::test]
    async fn test_rejects_unknown_status() {
        let (tool, list) = tool();
        let result = tool
            .execute(
                serde_json::json!({ "title": "Work", "items": [{"text": "x", "status": "bogus"}] }),
                CancellationToken::new(),
            )
            .await
            .expect("call returns is_error, not Err");
        assert!(result.is_error);
        assert!(list.read().await.items.is_empty(), "list must be untouched");
    }

    #[test]
    fn test_format_heading_and_markers() {
        let state = TodoState {
            title: Some("My tasks".to_string()),
            items: vec![
                TodoItem {
                    text: "Pending one".to_string(),
                    status: TodoStatus::Pending,
                },
                TodoItem {
                    text: "Working".to_string(),
                    status: TodoStatus::InProgress,
                },
                TodoItem {
                    text: "Finished".to_string(),
                    status: TodoStatus::Completed,
                },
                TodoItem {
                    text: "Dropped".to_string(),
                    status: TodoStatus::Cancelled,
                },
            ],
        };
        let output = format_todo_state(&state);
        // Heading is `TODO: <title>` on its own line, followed by a blank line; tasks are a
        // markdown checklist with no progress count.
        assert!(output.starts_with("TODO: My tasks\n\n"));
        assert!(!output.contains("done"));
        assert!(output.contains("- [ ] 1 Pending one"));
        assert!(output.contains("- [~] 2 Working"));
        assert!(output.contains("- [x] 3 Finished"));
        assert!(output.contains("- [-] 4 (cancelled) Dropped"));
    }

    #[test]
    fn test_format_soft_invariant_multiple_in_progress() {
        let state = TodoState {
            items: vec![
                TodoItem {
                    text: "A".to_string(),
                    status: TodoStatus::InProgress,
                },
                TodoItem {
                    text: "B".to_string(),
                    status: TodoStatus::InProgress,
                },
            ],
            ..Default::default()
        };
        let output = format_todo_state(&state);
        assert!(output.contains("(!) tasks #1, #2 are all in_progress"));
    }

    #[test]
    fn test_format_soft_invariant_none_in_progress() {
        let state = TodoState {
            items: vec![TodoItem {
                text: "A".to_string(),
                status: TodoStatus::Pending,
            }],
            ..Default::default()
        };
        assert!(format_todo_state(&state).contains("(!) no task in_progress"));
    }

    #[test]
    fn test_format_empty() {
        assert!(format_todo_state(&TodoState::default()).contains("(no tasks)"));
    }
}
