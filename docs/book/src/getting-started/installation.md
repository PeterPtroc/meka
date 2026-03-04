# Installation

agsh is written in Rust and builds as a single binary.

## Building from Source

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (edition 2024, requires Rust 1.85+)
- A C compiler (for the bundled SQLite)

### Build

```bash
git clone https://github.com/k4yt3x/agsh.git
cd agsh
cargo build --release
```

The binary will be at `target/release/agsh`. Copy it somewhere on your `$PATH`:

```bash
cp target/release/agsh ~/.local/bin/
```

### Verify

```bash
agsh --version
agsh --help
```
