//! `execute_command` tool. Spawns a shell process, optionally constrained by
//! the platform sandbox (Landlock/sandbox-exec) when permissions are
//! read-only, and streams stdout/stderr back to the agent.

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::error::{AgshError, Result};
use crate::permission::Permission;
use crate::provider::ToolDefinition;

use super::util::require_str;
use super::{Tool, ToolOutput};

pub(super) struct ExecuteCommandTool {
    pub sandbox_capability: crate::sandbox::SandboxCapability,
    pub shared_permission: crate::permission::SharedPermission,
    pub sandbox_enabled: bool,
}

#[async_trait]
impl Tool for ExecuteCommandTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "execute_command".to_string(),
            description: "Execute a shell command and return its output. In read mode, \
                the command runs in a read-only sandbox where filesystem writes are blocked."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute"
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": "Timeout in milliseconds. Defaults to 30000 (30 seconds)."
                    },
                    "scratchpad": {
                        "type": "string",
                        "description": "If provided, save the output to the scratchpad under this name instead of returning it inline."
                    }
                },
                "required": ["command"]
            }),
        }
    }

    fn required_permission(&self) -> Permission {
        if self.sandbox_enabled
            && !matches!(
                self.sandbox_capability,
                crate::sandbox::SandboxCapability::Unavailable
            )
        {
            Permission::Read
        } else {
            Permission::Write
        }
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        cancellation: CancellationToken,
    ) -> Result<ToolOutput> {
        let command = require_str(&input, "command", "execute_command")?;
        let timeout_ms = input["timeout_ms"].as_u64().unwrap_or(30000);
        let permission = self.shared_permission.get();
        let sandboxed = self.sandbox_enabled && permission != Permission::Write;

        if sandboxed
            && matches!(
                self.sandbox_capability,
                crate::sandbox::SandboxCapability::Unavailable
            )
        {
            return Ok(ToolOutput::text(
                "Shell command execution in read mode is not available on this \
                    platform because filesystem sandboxing is not supported. Switch to \
                    write mode (Shift+Tab) to execute commands without sandboxing."
                    .to_string(),
                true,
            ));
        }

        // Windows + sandboxed: spawn directly via CreateProcessAsUserW with a
        // Low-integrity token. This path can't go through tokio::process
        // because the stdlib gives no hook for injecting a custom token.
        #[cfg(windows)]
        if sandboxed
            && matches!(
                self.sandbox_capability,
                crate::sandbox::SandboxCapability::LowIntegrity
            )
        {
            return run_windows_low_integrity(&command, timeout_ms, cancellation).await;
        }

        #[cfg(windows)]
        let mut command_builder = {
            let mut cmd = tokio::process::Command::new("powershell.exe");
            cmd.arg("-NoProfile")
                .arg("-NonInteractive")
                .arg("-Command")
                .arg(&command);
            cmd
        };

        #[cfg(target_os = "macos")]
        let mut command_builder = if sandboxed
            && matches!(
                self.sandbox_capability,
                crate::sandbox::SandboxCapability::SandboxExec
            ) {
            let mut cmd = tokio::process::Command::new("sandbox-exec");
            cmd.arg("-p")
                .arg(crate::sandbox::SANDBOX_PROFILE_READONLY)
                .arg("sh")
                .arg("-c")
                .arg(&command);
            cmd
        } else {
            let mut cmd = tokio::process::Command::new("sh");
            cmd.arg("-c").arg(&command);
            cmd
        };

        #[cfg(target_os = "linux")]
        let mut command_builder = {
            let mut cmd = tokio::process::Command::new("sh");
            cmd.arg("-c").arg(&command);
            if sandboxed
                && let crate::sandbox::SandboxCapability::Landlock { abi_version } =
                    self.sandbox_capability
            {
                unsafe {
                    cmd.pre_exec(move || {
                        crate::sandbox::apply_landlock_readonly(abi_version)
                            .map_err(std::io::Error::from_raw_os_error)
                    });
                }
            }
            cmd
        };

        let mut child = command_builder
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| AgshError::ToolExecution {
                tool_name: "execute_command".to_string(),
                message: format!("failed to spawn command: {}", error),
            })?;

        let timeout_duration = std::time::Duration::from_millis(timeout_ms);

        // wait_with_output() consumes the child, so use wait() + manual
        // stdout/stderr reading instead to allow kill on cancellation.
        tokio::select! {
            _ = cancellation.cancelled() => {
                if let Err(error) = child.kill().await {
                    tracing::debug!("failed to kill child process: {}", error);
                }
                Err(AgshError::Interrupted)
            }
            _ = tokio::time::sleep(timeout_duration) => {
                if let Err(error) = child.kill().await {
                    tracing::debug!("failed to kill child process: {}", error);
                }
                Ok(ToolOutput::text(
                    format!("Command timed out after {}ms", timeout_ms),
                    true,
                ))
            }
            status = child.wait() => {
                let status = status.map_err(|error| AgshError::ToolExecution {
                    tool_name: "execute_command".to_string(),
                    message: format!("failed to wait for command: {}", error),
                })?;

                let exit_code = status.code().unwrap_or(-1);
                let stdout_content = read_to_string_best_effort(child.stdout.take()).await;
                let stderr_content = read_to_string_best_effort(child.stderr.take()).await;

                // No output-length truncation here: the agent layer's
                // `persist_oversized_results` auto-persists any oversized
                // result to the scratchpad losslessly. Truncating here would
                // corrupt binary-in-base64 pipelines (see #1 in the trial
                // feedback).
                Ok(assemble_command_output(&stdout_content, &stderr_content, exit_code))
            }
        }
    }
}

async fn read_to_string_best_effort<R>(reader: Option<R>) -> String
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut content = String::new();
    if let Some(mut reader) = reader
        && let Err(error) = reader.read_to_string(&mut content).await
    {
        tracing::debug!("failed to read child output: {}", error);
    }
    content
}

fn assemble_command_output(stdout: &str, stderr: &str, exit_code: i32) -> ToolOutput {
    let mut result_text = String::new();
    if !stdout.is_empty() {
        result_text.push_str(stdout);
    }
    if !stderr.is_empty() {
        if !result_text.is_empty() {
            result_text.push_str("\n--- stderr ---\n");
        }
        result_text.push_str(stderr);
    }
    if exit_code != 0 {
        result_text.push_str(&format!("\nExit code: {}", exit_code));
    }

    ToolOutput::text(
        if result_text.is_empty() {
            "(no output)".to_string()
        } else {
            result_text
        },
        exit_code != 0,
    )
}

/// Windows-only: spawn via `CreateProcessAsUserW` with a Low-integrity token,
/// read stdout/stderr from the pipe `File`s, and wait/kill through blocking
/// tasks. Mirrors the timeout/cancellation semantics of the standard path.
///
/// Stdout/stderr are drained on dedicated tasks that start *before* the child
/// wait begins. Without that, a child that writes more than the pipe buffer
/// (1 MiB hinted; smaller if the kernel rounds down) before anyone reads will
/// block in `WriteFile`, the wait never returns, and the whole call times out
/// with truncated output. After the child exits or is killed, the pipe write
/// ends close and the drain tasks terminate at EOF.
///
/// # Drain timeouts
///
/// On Windows there is no atomic "kill process tree" primitive available in
/// this code path (a future refactor could wrap the child in a Job Object
/// with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`). Consequently, a grandchild
/// that inherits the pipe write handles can keep the pipe alive past the
/// direct child's exit — the drain tasks would then block on `ReadFile`
/// until the grandchild finally exits. To bound the tool-call wall time we
/// cap every drain await with [`DRAIN_TIMEOUT`]; on timeout the drain task
/// is aborted, any output already read is lost, and we attach a diagnostic
/// note so the model can reason about truncation.
#[cfg(windows)]
async fn run_windows_low_integrity(
    command: &str,
    timeout_ms: u64,
    cancellation: CancellationToken,
) -> Result<ToolOutput> {
    use std::sync::Arc;
    use std::time::Duration;

    // Bound the post-kill cleanup wait so a stuck `TerminateProcess` or a
    // drain task that somehow fails to reach EOF can't hang the tool
    // indefinitely. Two seconds is generous for kernel-side teardown.
    const POST_KILL_TIMEOUT: Duration = Duration::from_secs(2);
    // Bound the post-exit drain on the happy path. Anything longer than
    // this is almost certainly a grandchild holding the pipe open; we'd
    // rather return quickly with a truncation note than block the whole
    // tool call indefinitely.
    const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

    let mut sandboxed = crate::sandbox::spawn_low_integrity_command(command).map_err(|error| {
        AgshError::ToolExecution {
            tool_name: "execute_command".to_string(),
            message: format!("failed to spawn sandboxed command: {}", error),
        }
    })?;

    let stdout = sandboxed.take_stdout().map(tokio::fs::File::from_std);
    let stderr = sandboxed.take_stderr().map(tokio::fs::File::from_std);

    let child = Arc::new(sandboxed);
    let timeout_duration = Duration::from_millis(timeout_ms);

    let stdout_task = tokio::spawn(async move { read_to_string_best_effort(stdout).await });
    let stderr_task = tokio::spawn(async move { read_to_string_best_effort(stderr).await });

    let wait_child = Arc::clone(&child);
    let wait_handle = tokio::task::spawn_blocking(move || wait_child.wait_blocking());

    tokio::select! {
        _ = cancellation.cancelled() => {
            if let Err(error) = child.kill() {
                tracing::debug!("failed to kill sandboxed child: {}", error);
            }
            abort_after_timeout(wait_handle, POST_KILL_TIMEOUT).await;
            abort_after_timeout(stdout_task, POST_KILL_TIMEOUT).await;
            abort_after_timeout(stderr_task, POST_KILL_TIMEOUT).await;
            Err(AgshError::Interrupted)
        }
        _ = tokio::time::sleep(timeout_duration) => {
            if let Err(error) = child.kill() {
                tracing::debug!("failed to kill sandboxed child: {}", error);
            }
            abort_after_timeout(wait_handle, POST_KILL_TIMEOUT).await;
            abort_after_timeout(stdout_task, POST_KILL_TIMEOUT).await;
            abort_after_timeout(stderr_task, POST_KILL_TIMEOUT).await;
            Ok(ToolOutput::text(
                format!("Command timed out after {}ms", timeout_ms),
                true,
            ))
        }
        join = wait_handle => {
            let status = join
                .map_err(|error| AgshError::ToolExecution {
                    tool_name: "execute_command".to_string(),
                    message: format!("wait task panicked: {}", error),
                })?
                .map_err(|error| AgshError::ToolExecution {
                    tool_name: "execute_command".to_string(),
                    message: format!("failed to wait for command: {}", error),
                })?;

            let exit_code = status.code().unwrap_or(-1);
            let (stdout_content, stdout_timed_out) =
                join_drain_with_timeout(stdout_task, DRAIN_TIMEOUT).await;
            let (stderr_content, stderr_timed_out) =
                join_drain_with_timeout(stderr_task, DRAIN_TIMEOUT).await;
            if stdout_timed_out || stderr_timed_out {
                tracing::warn!(
                    "sandboxed command output drain timed out after {:?}; \
                     a background process may be holding the pipe open",
                    DRAIN_TIMEOUT
                );
            }
            let mut output =
                assemble_command_output(&stdout_content, &stderr_content, exit_code);
            if stdout_timed_out || stderr_timed_out {
                append_drain_truncation_note(
                    &mut output,
                    stdout_timed_out,
                    stderr_timed_out,
                );
            }
            Ok(output)
        }
    }
}

/// Await a `JoinHandle<String>` up to `timeout`. If the timeout expires the
/// task is aborted and an empty string is returned alongside `timed_out=true`
/// so the caller can surface a truncation note.
#[cfg(windows)]
async fn join_drain_with_timeout(
    mut task: tokio::task::JoinHandle<String>,
    timeout: std::time::Duration,
) -> (String, bool) {
    tokio::select! {
        result = &mut task => match result {
            Ok(content) => (content, false),
            Err(error) => {
                tracing::debug!("drain task failed: {}", error);
                (String::new(), false)
            }
        },
        _ = tokio::time::sleep(timeout) => {
            task.abort();
            (String::new(), true)
        }
    }
}

/// Abort any pending `JoinHandle` after `timeout`. Used on cancel/timeout
/// cleanup paths where we don't need the task's output — just its termination.
#[cfg(windows)]
async fn abort_after_timeout<T: 'static>(
    mut handle: tokio::task::JoinHandle<T>,
    timeout: std::time::Duration,
) {
    tokio::select! {
        _ = &mut handle => {}
        _ = tokio::time::sleep(timeout) => {
            handle.abort();
        }
    }
}

#[cfg(windows)]
fn append_drain_truncation_note(
    output: &mut ToolOutput,
    stdout_timed_out: bool,
    stderr_timed_out: bool,
) {
    let note = match (stdout_timed_out, stderr_timed_out) {
        (true, true) => {
            "\n(stdout/stderr drain timed out; output may be truncated — a background process likely held the pipe open past the child's exit)"
        }
        (true, false) => {
            "\n(stdout drain timed out; output may be truncated — a background process likely held the pipe open past the child's exit)"
        }
        (false, true) => {
            "\n(stderr drain timed out; output may be truncated — a background process likely held the pipe open past the child's exit)"
        }
        (false, false) => return,
    };
    if let Some(crate::provider::ToolResultContent::Text { text }) = output.content.last_mut() {
        text.push_str(note);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ContentBlock;

    fn text_content(output: &ToolOutput) -> String {
        ContentBlock::tool_result_text_content(&output.content)
    }

    fn test_shared_permission() -> crate::permission::SharedPermission {
        crate::permission::SharedPermission::new(Permission::Write)
    }

    #[tokio::test]
    async fn test_execute_command() {
        let tool = ExecuteCommandTool {
            sandbox_capability: crate::sandbox::detect(),
            shared_permission: test_shared_permission(),
            sandbox_enabled: true,
        };
        let result = tool
            .execute(
                serde_json::json!({"command": "echo hello"}),
                CancellationToken::new(),
            )
            .await
            .expect("should succeed");

        assert!(!result.is_error);
        assert_eq!(text_content(&result).trim(), "hello");
    }

    #[tokio::test]
    async fn test_execute_command_failure() {
        let tool = ExecuteCommandTool {
            sandbox_capability: crate::sandbox::detect(),
            shared_permission: test_shared_permission(),
            sandbox_enabled: true,
        };
        let result = tool
            .execute(
                serde_json::json!({"command": "false"}),
                CancellationToken::new(),
            )
            .await
            .expect("should succeed");

        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_execute_command_large_output_not_truncated() {
        // Output well over the old 30 KB cap — the tool must return it in
        // full. The agent layer handles oversize downstream.
        let tool = ExecuteCommandTool {
            sandbox_capability: crate::sandbox::detect(),
            shared_permission: test_shared_permission(),
            sandbox_enabled: true,
        };
        let result = tool
            .execute(
                // 50 000 "x" characters. POSIX-portable — uses `head` and `tr`
                // instead of bash brace expansion so it works under `dash`
                // (Debian/Ubuntu's default `/bin/sh`) as well as `bash`.
                serde_json::json!({
                    "command": "head -c 50000 /dev/zero | tr '\\0' x"
                }),
                CancellationToken::new(),
            )
            .await
            .expect("should succeed");

        let text = text_content(&result);
        assert!(
            !text.contains("(output truncated"),
            "no truncation marker expected, got: {:.200}...",
            text
        );
        assert!(
            text.trim().len() >= 50_000,
            "expected >= 50 000 chars, got {}",
            text.trim().len()
        );
    }

    #[cfg(windows)]
    mod windows_sandbox {
        use super::*;

        fn read_permission() -> crate::permission::SharedPermission {
            crate::permission::SharedPermission::new(Permission::Read)
        }

        /// Under Low integrity, writing to the user's profile directory
        /// must be denied by the OS. The test probes a path under
        /// `%USERPROFILE%` and asserts the file is never created.
        #[tokio::test]
        async fn test_windows_sandbox_blocks_write_to_userprofile() {
            let probe_path = format!(
                "{}\\agsh-sandbox-probe.txt",
                std::env::var("USERPROFILE").expect("USERPROFILE must be set on Windows")
            );
            // Clean any stray file from an earlier failed run before starting.
            let _ = std::fs::remove_file(&probe_path);

            let tool = ExecuteCommandTool {
                sandbox_capability: crate::sandbox::SandboxCapability::LowIntegrity,
                shared_permission: read_permission(),
                sandbox_enabled: true,
            };
            let _ = tool
                .execute(
                    serde_json::json!({
                        "command": format!("echo hello > \"{}\"", probe_path),
                    }),
                    CancellationToken::new(),
                )
                .await
                .expect("execute should not error");

            let existed = std::path::Path::new(&probe_path).exists();
            // Defensive cleanup even if the assertion below fails.
            let _ = std::fs::remove_file(&probe_path);
            assert!(
                !existed,
                "Low-integrity sandbox should have blocked write to {}",
                probe_path
            );
        }

        /// A command that produces well over the default Windows pipe buffer
        /// (~4 KB) of output must complete without deadlocking and without
        /// truncation. Before the concurrent-drain fix, the child would block
        /// in `WriteFile` past the buffer, the wait would never return, and
        /// the tool would report a spurious timeout.
        #[tokio::test]
        async fn test_windows_sandbox_large_output_under_sandbox() {
            let tool = ExecuteCommandTool {
                sandbox_capability: crate::sandbox::SandboxCapability::LowIntegrity,
                shared_permission: read_permission(),
                sandbox_enabled: true,
            };
            // PowerShell builds a 262144-char string in memory then emits it
            // as one line. Total output is ~256 KB — well past any plausible
            // pipe buffer.
            let result = tool
                .execute(
                    serde_json::json!({
                        "command": "'x' * 262144",
                        "timeout_ms": 60000u64,
                    }),
                    CancellationToken::new(),
                )
                .await
                .expect("execute should not error");

            assert!(
                !result.is_error,
                "large-output command should not be flagged as an error"
            );
            let text = text_content(&result);
            let x_count = text.matches('x').count();
            assert!(
                x_count >= 262144,
                "expected >= 262144 'x' characters in output, got {}",
                x_count
            );
        }

        /// The child's stdin must be connected to `NUL`, not inherited from
        /// the agent's TTY and not left as an invalid handle. `$input`
        /// enumerates pipeline input; piped from NUL it yields zero objects.
        /// The command must complete promptly rather than hanging on a
        /// dangling stdin.
        #[tokio::test]
        async fn test_windows_sandbox_stdin_is_null() {
            let tool = ExecuteCommandTool {
                sandbox_capability: crate::sandbox::SandboxCapability::LowIntegrity,
                shared_permission: read_permission(),
                sandbox_enabled: true,
            };
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                tool.execute(
                    serde_json::json!({
                        "command": "($input | Measure-Object).Count",
                        "timeout_ms": 5000u64,
                    }),
                    CancellationToken::new(),
                ),
            )
            .await
            .expect("command must not hang waiting for stdin")
            .expect("execute should not error");

            assert!(!result.is_error);
            let text = text_content(&result);
            assert!(
                text.trim().starts_with('0'),
                "expected stdin-object count of 0, got {:?}",
                text
            );
        }

        /// Round-trip a grab-bag of tricky marker strings through PowerShell
        /// to confirm `quote_command_arg` + PowerShell's argv parser are
        /// inverses. Uses PowerShell single-quote literals internally so the
        /// test exercises our command-line encoding, not PS string rules.
        #[tokio::test]
        async fn test_windows_sandbox_quoting_roundtrip() {
            let tool = ExecuteCommandTool {
                sandbox_capability: crate::sandbox::SandboxCapability::LowIntegrity,
                shared_permission: read_permission(),
                sandbox_enabled: true,
            };

            let cases: &[&str] = &[
                "plain",
                "with spaces",
                r#"quotes "inside""#,
                r"back\slashes",
                "meta & chars | pipe > redir",
                "日本語 unicode",
            ];
            for marker in cases {
                // Escape ' as '' inside the PS single-quote literal.
                let script = format!("Write-Output '{}'", marker.replace('\'', "''"));
                let result = tool
                    .execute(
                        serde_json::json!({ "command": script, "timeout_ms": 10000u64 }),
                        CancellationToken::new(),
                    )
                    .await
                    .expect("execute should not error");
                assert!(!result.is_error, "command for marker {:?} errored", marker);
                let text = text_content(&result);
                assert!(
                    text.contains(marker),
                    "marker {:?} missing from output {:?}",
                    marker,
                    text
                );
            }
        }

        /// Reads must still succeed under Low integrity. The hosts file is
        /// readable by Everyone on stock Windows, so it's a good probe.
        #[tokio::test]
        async fn test_windows_sandbox_allows_read() {
            let tool = ExecuteCommandTool {
                sandbox_capability: crate::sandbox::SandboxCapability::LowIntegrity,
                shared_permission: read_permission(),
                sandbox_enabled: true,
            };
            let result = tool
                .execute(
                    serde_json::json!({
                        "command": "type C:\\Windows\\System32\\drivers\\etc\\hosts",
                    }),
                    CancellationToken::new(),
                )
                .await
                .expect("execute should not error");

            assert!(
                !result.is_error,
                "reading %WINDIR%\\System32\\drivers\\etc\\hosts should succeed under Low integrity"
            );
        }
    }
}
