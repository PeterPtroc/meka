# Shell Tool

## `execute_command`

Execute a shell command and return its output.

**Permission:** Read (sandboxed) / Write (unsandboxed)

### Parameters

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `command` | string | yes | The shell command to execute |
| `timeout_ms` | integer | no | Timeout in milliseconds (default: 30000) |
| `scratchpad` | string | no | Save output to the scratchpad under this name |

### Behavior

- Executes the command via `sh -c "<command>"` on Unix or `powershell -Command "<command>"` on Windows.
- Captures both stdout and stderr.
- Returns the exit code along with the output if non-zero.
- Output is truncated to 30,000 characters. Oversized output is automatically saved to the scratchpad.
- Default timeout is 30 seconds. If the command exceeds the timeout, it is killed.
- Supports cancellation: pressing Ctrl+C while a command is running kills the child process.

### Read-Only Sandbox

In **read mode**, commands run inside a read-only filesystem sandbox. The child process can read files and execute programs, but any attempt to write to the filesystem is blocked by the kernel:

- **Linux**: Uses [Landlock LSM](https://landlock.io/) (kernel 5.13+). The child process is restricted via `landlock_restrict_self` before exec. Only `READ_FILE`, `READ_DIR`, and `EXECUTE` access rights are granted.
- **macOS**: Uses `sandbox-exec` with a SBPL profile that denies all `file-write*` operations.
- **Windows / unsupported platforms**: Shell commands are not available in read mode. Switch to write mode to execute commands.

In **write mode**, commands run without any sandbox restrictions.

To disable sandboxed shell execution in read mode, set `sandbox = false` under `[shell]` in the config file. When disabled, shell commands require write mode.

```toml
[shell]
sandbox = false
```
