//! `spawn_agent` tool: delegates a self-contained research/exploration task
//! to a fresh sub-agent with its own conversation, returning the
//! sub-agent's final report as a single tool result.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::agent::{Agent, AgentOptions};
use crate::context::build_environment_context;
use crate::conversation::Conversation;
use crate::error::{AgshError, Result};
use crate::permission::{Permission, SharedPermission};
use crate::provider::{Provider, ToolDefinition};
use crate::session::SessionManager;

use super::{BuiltinToolFilter, Tool, ToolOutput, ToolRegistry};

/// Parameters needed to build a fresh ToolRegistry for sub-agents.
#[derive(Clone)]
pub struct ToolBuilderParams {
    pub web_client: crate::config::WebClientConfig,
    pub sandbox_enabled: bool,
    pub sandbox_capability: crate::sandbox::SandboxCapability,
    pub sandbox_backend: crate::config::SandboxBackend,
    pub backend_probe: crate::sandbox::BackendProbe,
    /// Parent's `[tools]` filter — sub-agents inherit it.
    pub builtin_filter: BuiltinToolFilter,
    /// Shared skill cache. Sub-agents read from the same cache as the
    /// parent so their system prompts stay consistent and pick up the
    /// same auto-reloads.
    pub skills: Arc<crate::skills::SkillCache>,
    /// Parent's MCP client manager, if any servers are configured. When
    /// `Some`, every `spawn_agent` invocation calls
    /// [`crate::mcp::McpClientManager::install_tools_on`] on the
    /// freshly-built sub-agent registry so sub-agents see the same MCP
    /// resource meta-tools and per-server adapters as the parent.
    /// `None` is the no-MCP-configured case.
    pub mcp_manager: Option<Arc<crate::mcp::McpClientManager>>,
    /// Shared `SessionManager` so sub-agents can create their own DB
    /// session at spawn time and persist their conversation under it.
    pub session_manager: SessionManager,
    /// Parent agent's session ID. Read at spawn time so the new sub-agent
    /// session's `parent_session_id` column points back here; cascade-on-
    /// delete in `SessionManager::delete_session` then sweeps sub-agent
    /// rows when the parent is deleted.
    pub parent_shared_session_id: Arc<RwLock<Option<Uuid>>>,
    /// Parent's session-level counters. Shared so sub-agent token usage
    /// rolls up into the same `/status` totals — operators see the full
    /// cost of a session including everything its sub-agents consumed.
    pub session_stats: Arc<crate::stats::SessionStats>,
    /// Parent's options, used to derive the sub-agent's inherited fields
    /// (`sandboxed_shell`, `context_messages`, `user_instructions`) inside
    /// [`Agent::new_subagent`].
    pub parent_options: AgentOptions,
}

pub struct SpawnAgentTool {
    pub provider: Arc<dyn Provider>,
    pub parent_permission: SharedPermission,
    pub tool_builder_params: ToolBuilderParams,
    pub user_instructions: Option<String>,
}

#[async_trait]
impl Tool for SpawnAgentTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "spawn_agent".to_string(),
            description: "Spawn a sub-agent to perform a research, analysis, or delegated task. \
                          The sub-agent inherits the parent's permission level, has its own \
                          private todo list and scratchpad, and returns a single text report. \
                          Multiple spawn_agent calls in one turn run in parallel. Use \
                          `inherit_scratchpad` to grant read-only access to specific parent \
                          scratchpad entries by name so the sub-agent can consume large captured \
                          output via `scratchpad_read` without you re-inlining it in the prompt. \
                          Tip: when you expect to hand output to a sub-agent later, set the \
                          `scratchpad` parameter on the originating tool call (e.g. \
                          `execute_command({command: \"...\", scratchpad: \"build_log\"})`) so \
                          the entry has a semantic name you can pass through \
                          `inherit_scratchpad`."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The task description for the sub-agent"
                    },
                    "scratchpad": {
                        "type": "string",
                        "description": "If provided, save the sub-agent's final report to the \
                                        parent's scratchpad under this name instead of returning \
                                        it inline."
                    },
                    "inherit_scratchpad": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Names of the parent's scratchpad entries the sub-agent \
                                        is allowed to read. The sub-agent's `scratchpad_read` \
                                        falls back to the parent for these names; \
                                        `scratchpad_list` shows them with origin `inherited`. \
                                        Read-only: `scratchpad_write` / `_edit` / `_delete` \
                                        targeting an inherited name return an error so the \
                                        sub-agent can't silently shadow your copy. Names that \
                                        don't exist in the parent are silently skipped."
                    }
                },
                "required": ["prompt"]
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
        cancellation: CancellationToken,
    ) -> Result<ToolOutput> {
        let prompt = input["prompt"]
            .as_str()
            .ok_or_else(|| AgshError::ToolExecution {
                tool_name: "spawn_agent".to_string(),
                message: "missing 'prompt' parameter".to_string(),
            })?
            .to_string();

        // `inherit_scratchpad`: optional array of parent-scratchpad
        // names. Non-string entries are silently skipped so a partially-
        // malformed array doesn't tank the whole spawn.
        let inherited_scratchpad: Vec<String> = input
            .get("inherit_scratchpad")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        // Snapshot the parent's permission. Demote `Ask` to `Read`: the
        // sub-agent runs with `approval_sender: None`, so every Ask-mode
        // tool dispatch would otherwise fail with the
        // "Ask mode requires interactive shell for tool approval" error
        // (see `Agent::execute_with_approval`). Read keeps the sub-agent
        // useful for the common research/exploration case; forwarding
        // approvals through the parent's REPL is future work.
        let sub_perm = match self.parent_permission.get() {
            Permission::Ask => Permission::Read,
            other => other,
        };

        // Resolve parent session ID. By the time a tool runs,
        // `Agent::run_turn` has already written `shared_session_id` before
        // dispatching tools. A missing value here means an agent ran a
        // tool without first creating its session — an internal invariant
        // break worth surfacing rather than silently producing an orphan.
        let parent_sid = self
            .tool_builder_params
            .parent_shared_session_id
            .read()
            .await
            .ok_or_else(|| AgshError::ToolExecution {
                tool_name: "spawn_agent".to_string(),
                message: "parent session ID not yet assigned (run_turn invariant)".to_string(),
            })?;

        // Create the sub-agent's own DB session, linked back to the parent
        // via `parent_session_id`. Cascade-on-delete in `delete_session`
        // sweeps it when the parent is removed.
        let sub_session_id = self
            .tool_builder_params
            .session_manager
            .create_child_session(parent_sid)
            .await
            .map_err(|error| AgshError::ToolExecution {
                tool_name: "spawn_agent".to_string(),
                message: format!("failed to create sub-agent session: {}", error),
            })?;
        let sub_shared_session_id: Arc<RwLock<Option<Uuid>>> =
            Arc::new(RwLock::new(Some(sub_session_id)));
        tracing::info!(
            "spawning sub-agent: parent={} child={}",
            parent_sid,
            sub_session_id
        );

        // Build a sub-agent tool registry: no `spawn_agent` (no recursive
        // spawning) and a fresh, private todo list so the sub-agent's
        // todo_write / todo_read calls don't touch the parent's task
        // tracking. Scratchpad and render_image use the new sub-session ID.
        let sub_shared_perm = SharedPermission::new(sub_perm, self.parent_permission.enabled());
        let sub_todo_list: super::todo::SharedTodoList =
            Arc::new(tokio::sync::RwLock::new(Vec::new()));
        let sub_registry = ToolRegistry::build_for_subagent(
            self.tool_builder_params.web_client.clone(),
            sub_shared_perm.clone(),
            self.tool_builder_params.sandbox_enabled,
            self.tool_builder_params.sandbox_capability.clone(),
            self.tool_builder_params.sandbox_backend,
            self.tool_builder_params.backend_probe.clone(),
            self.tool_builder_params.builtin_filter.clone(),
            sub_todo_list.clone(),
            self.tool_builder_params.session_manager.clone(),
            sub_shared_session_id.clone(),
            self.tool_builder_params.skills.clone(),
            if inherited_scratchpad.is_empty() {
                None
            } else {
                Some(parent_sid)
            },
            inherited_scratchpad.clone(),
        )
        .map_err(|error| AgshError::ToolExecution {
            tool_name: "spawn_agent".to_string(),
            message: format!("failed to build sub-agent tool registry: {}", error),
        })?;

        // Inherit the parent's MCP toolset. Skipped silently when no MCP
        // manager is attached (no servers configured) or when the parent's
        // servers are still Pending / Failed at spawn time. `install_tools_on`
        // is non-spawning and idempotent — see `src/mcp.rs:install_tools_on`.
        if let Some(manager) = self.tool_builder_params.mcp_manager.as_ref() {
            manager.install_tools_on(&sub_registry).await;
        }

        // Build the sub-agent's system prompt against the fully-loaded
        // registry (registry now includes MCP adapters). The override on
        // `AgentOptions` is static, so this single build captures the
        // full tool catalogue visible to the sub-agent.
        let tools = sub_registry.definitions_for_permission(sub_perm);
        let sub_system_prompt = build_subagent_system_prompt(
            sub_perm,
            &tools,
            self.user_instructions.as_deref(),
            &inherited_scratchpad,
        );

        let environment_context = build_environment_context(sub_perm);
        let augmented_prompt = format!("{}\n{}", environment_context, prompt);

        let sub_agent = Agent::new_subagent(
            Arc::clone(&self.provider),
            sub_registry,
            self.tool_builder_params.session_manager.clone(),
            sub_shared_perm,
            &self.tool_builder_params.parent_options,
            sub_system_prompt,
            sub_todo_list,
            sub_shared_session_id,
            self.tool_builder_params.skills.clone(),
            self.tool_builder_params.session_stats.clone(),
        );

        // Run the sub-agent's single turn via the shared `Agent::run_turn`
        // path. Conversation persistence (user message, assistant
        // messages, tool results) happens inside `run_turn` against the
        // sub-session, so the audit trail is identical to a primary
        // agent's. Silent rendering and the omitted MCP gate are baked
        // into the options via `new_subagent`.
        let mut messages = Conversation::new();
        let mut session_id_opt = Some(sub_session_id);
        sub_agent
            .run_turn(
                &mut session_id_opt,
                &mut messages,
                augmented_prompt,
                cancellation,
            )
            .await?;

        let report = messages
            .last_assistant_text()
            .unwrap_or_else(|| "(sub-agent produced no final text)".to_string());
        Ok(ToolOutput::text(report, false))
    }
}

fn build_subagent_system_prompt(
    permission: Permission,
    tools: &[ToolDefinition],
    user_instructions: Option<&str>,
    inherited_scratchpad: &[String],
) -> String {
    let mut prompt = String::new();
    prompt.push_str(
        "You are a research sub-agent. Complete the assigned task using the \
         available tools, then produce a concise final report summarizing your \
         findings. Do not ask follow-up questions — work with what you have. \
         For multi-step work, use `todo_write` to plan and `todo_read` to \
         check progress — your todo list is private to this sub-agent.\n\n",
    );

    prompt.push_str(&format!("## Permission Level: {}\n\n", permission));

    if let Some(instructions) = user_instructions
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        prompt.push_str("## User Instructions\n\n");
        prompt.push_str(
            "These are installation-specific rules set by the user. Treat them as \
             hard constraints unless they conflict with safety requirements.\n\n",
        );
        prompt.push_str(instructions);
        prompt.push_str("\n\n");
    }

    if !inherited_scratchpad.is_empty() {
        prompt.push_str("## Inherited Scratchpad Entries\n\n");
        prompt.push_str(
            "Your parent agent has granted you read-only access to the following \
             scratchpad entries from its own session. Use `scratchpad_read` with \
             the exact names below to load them on demand — do not assume their \
             contents without reading. `scratchpad_write`, `_edit`, and `_delete` \
             against these names will return an error; if you need to derive new \
             state, save it under a different name (e.g. `<name>_local`).\n\n",
        );
        for name in inherited_scratchpad {
            prompt.push_str(&format!("- {}\n", name));
        }
        prompt.push('\n');
    }

    if !tools.is_empty() {
        prompt.push_str("## Available Tools\n\n");
        for tool in tools {
            prompt.push_str(&format!("- **{}**: {}\n", tool.name, tool.description));
        }
        prompt.push('\n');
    }

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subagent_system_prompt_reflects_inherited_permission() {
        let prompt = build_subagent_system_prompt(Permission::Write, &[], None, &[]);
        assert!(
            prompt.contains(&format!("## Permission Level: {}", Permission::Write)),
            "expected Write level in prompt, got: {}",
            prompt
        );

        let read_prompt = build_subagent_system_prompt(Permission::Read, &[], None, &[]);
        assert!(read_prompt.contains(&format!("## Permission Level: {}", Permission::Read)));
    }

    #[test]
    fn test_subagent_system_prompt_mentions_todo_tools() {
        let prompt = build_subagent_system_prompt(Permission::Read, &[], None, &[]);
        assert!(
            prompt.contains("todo_write") && prompt.contains("todo_read"),
            "expected todo_write/todo_read mention in prompt, got: {}",
            prompt
        );
    }

    #[test]
    fn test_subagent_system_prompt_omits_inheritance_section_when_empty() {
        let prompt = build_subagent_system_prompt(Permission::Read, &[], None, &[]);
        assert!(
            !prompt.contains("Inherited Scratchpad"),
            "no inherited section expected for empty allowlist, got: {}",
            prompt
        );
    }

    #[test]
    fn test_subagent_system_prompt_lists_inherited_names() {
        let names = vec!["captured_output".to_string(), "research_notes".to_string()];
        let prompt = build_subagent_system_prompt(Permission::Read, &[], None, &names);
        assert!(prompt.contains("## Inherited Scratchpad Entries"));
        assert!(prompt.contains("- captured_output"));
        assert!(prompt.contains("- research_notes"));
        assert!(prompt.contains("scratchpad_read"));
    }

    #[test]
    fn test_subagent_system_prompt_warns_inherited_writes_will_error() {
        let names = vec!["build_log".to_string()];
        let prompt = build_subagent_system_prompt(Permission::Read, &[], None, &names);
        assert!(
            prompt.contains("will return an error"),
            "expected write-rejection wording, got: {}",
            prompt,
        );
        assert!(
            prompt.contains("_local"),
            "expected naming suggestion, got: {}",
            prompt,
        );
    }

    async fn test_session_manager() -> SessionManager {
        SessionManager::open(Some(std::path::Path::new(":memory:")))
            .await
            .expect("in-memory session manager")
    }

    // (Permission gating and "Unknown tool" fold-into-ToolOutput
    // semantics that used to live in `run_subagent_tool` are now
    // exercised by the shared `Agent::run_turn` path's tool-dispatch
    // logic — covered by `src/agent.rs` and `src/tools.rs` test suites.)

    #[tokio::test]
    async fn test_subagent_registry_has_independent_todo_list() {
        use crate::sandbox::{BackendProbe, SandboxCapability};
        use crate::tools::BuiltinToolFilter;

        let parent_list: super::super::todo::SharedTodoList =
            Arc::new(tokio::sync::RwLock::new(Vec::new()));
        let sub_list: super::super::todo::SharedTodoList =
            Arc::new(tokio::sync::RwLock::new(Vec::new()));

        let sub_registry = ToolRegistry::build_for_subagent(
            crate::config::WebClientConfig::default(),
            SharedPermission::new(Permission::Read, crate::permission::EnabledPermissions::ALL),
            true,
            SandboxCapability::Unavailable,
            crate::config::SandboxBackend::Landlock,
            BackendProbe::Missing {
                reason: "test fixture".to_string(),
            },
            BuiltinToolFilter::default(),
            sub_list.clone(),
            test_session_manager().await,
            Arc::new(tokio::sync::RwLock::new(None)),
            crate::skills::SkillCache::for_root(None),
            None,
            Vec::new(),
        )
        .expect("subagent registry should build");

        let todo_write = sub_registry
            .get("todo_write")
            .expect("subagent should have todo_write");
        todo_write
            .execute(
                serde_json::json!({
                    "tasks": [{"id": "1", "description": "sub task", "status": "pending"}]
                }),
                CancellationToken::new(),
            )
            .await
            .expect("todo_write should succeed");

        assert_eq!(sub_list.read().await.len(), 1);
        assert!(
            parent_list.read().await.is_empty(),
            "parent list must remain untouched"
        );
    }
}
