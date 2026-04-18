//! End-to-end CLI smoke tests. These shell out to the built `agsh` binary
//! (`env!("CARGO_BIN_EXE_agsh")`) so they exercise the same entry point
//! users hit on the command line. They cover surface-level invariants that
//! unit tests can't reach: argument-parser wiring, `--help` output, and the
//! exit status of trivial subcommands.

use std::process::Command;

fn agsh() -> Command {
    Command::new(env!("CARGO_BIN_EXE_agsh"))
}

#[test]
fn version_flag_prints_version_and_exits_zero() {
    let output = agsh()
        .arg("--version")
        .output()
        .expect("failed to spawn agsh");
    assert!(
        output.status.success(),
        "agsh --version exited non-zero: {:?}",
        output.status
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.starts_with("agsh "),
        "expected version output to start with 'agsh ', got: {}",
        stdout
    );
}

#[test]
fn help_flag_lists_subcommands() {
    let output = agsh().arg("--help").output().expect("failed to spawn agsh");
    assert!(output.status.success(), "agsh --help exited non-zero");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for expected in ["setup", "export", "delete", "list"] {
        assert!(
            stdout.contains(expected),
            "--help output missing subcommand '{}':\n{}",
            expected,
            stdout
        );
    }
}

#[test]
fn unknown_subcommand_exits_nonzero() {
    let output = agsh()
        .arg("--definitely-not-a-flag")
        .output()
        .expect("failed to spawn agsh");
    assert!(
        !output.status.success(),
        "agsh accepted an unknown flag without erroring"
    );
}
