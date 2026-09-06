# agents-first-toolkit

Five single-purpose, pure-Rust CLI tools that batch agent tool calls into
one tool call. Every crate is a standalone binary with its own `src/lib.rs`
for library consumers and a `src/main.rs` for CLI use.

## The crates

| Crate | Name | What it batches | Repo |
|---|---|---|---|
| `crates/shellaborate` | agentic-shell | `terminal` + `git` + `gh` + `execute_code` + `process` DAG | riceharvest/agents-first-toolkit · crates/shellaborate |
| `crates/patchify` | agentic-edit | `read_file` + `patch` + `write_file` + `terminal` | riceharvest/agents-first-toolkit · crates/patchify |
| `crates/curlosity` | agentic-web | `web_search` + `web_extract` + `curl` | riceharvest/agents-first-toolkit · crates/curlosity |
| `crates/readcursively` | agentic-code | `search_files` + `grep` + `read_file` | riceharvest/agents-first-toolkit · crates/readcursively |
| `crates/recurlsively` | agentic-web | recursive domain crawl → Markdown | riceharvest/agents-first-toolkit · crates/recurlsively |

## Build

```sh
# build all five
cargo build --release

# or build a single crate
cargo build --release --bin shellaborate
cargo build --release --bin patchify
cargo build --release --bin curlosity
cargo build --release --bin readcursively
cargo build --release --bin recurlsively
```

## Run all tests

```sh
cargo test
```

## Install (single crate)

Each crate has its own `install.sh` / `install.ps1`:

```sh
# shellaborate
curl -fsSL https://raw.githubusercontent.com/riceharvest/agents-first-toolkit/main/crates/shellaborate/install.sh | sh

# patchify
curl -fsSL https://raw.githubusercontent.com/riceharvest/agents-first-toolkit/main/crates/patchify/install.sh | sh

# curlosity
curl -fsSL https://raw.githubusercontent.com/riceharvest/agents-first-toolkit/main/crates/curlosity/install.sh | sh

# readcursively
curl -fsSL https://raw.githubusercontent.com/riceharvest/agents-first-toolkit/main/crates/readcursively/install.sh | sh

# recurlsively
curl -fsSL https://raw.githubusercontent/riceharvest/agents-first-toolkit/main/crates/recurlsively/install.sh | sh
```

## Install (all crates at once)

```sh
curl -fsSL https://raw.githubusercontent.com/riceharvest/agents-first-toolkit/main/install.sh | sh
```

## Layout

```
agents-first-toolkit/
├── Cargo.toml          # workspace manifest
├── README.md           # this file
├── install.sh          # install all five
├── install.ps1         # install all five (Windows)
├── .github/
│   └── workflows/
│       ├── ci.yml      # cargo fmt, clippy, test (OS matrix)
│       └── release.yml # build 5-target matrix per crate, publish
└── crates/
    ├── shellaborate/
    │   ├── Cargo.toml
    │   ├── src/
    │   ├── tests/
    │   ├── docs/
    │   ├── install.sh
    │   ├── install.ps1
    │   ├── README.md
    │   ├── LICENSE-MIT
    │   ├── LICENSE-APACHE
    │   └── deny.toml
    ├── patchify/
    │   ├── Cargo.toml
    │   ├── src/
    │   ├── tests/
    │   ├── docs/
    │   ├── examples/
    │   ├── hermes-tool.json
    │   ├── install.sh
    │   ├── install.ps1
    │   ├── README.md
    │   ├── CHANGELOG.md
    │   ├── LICENSE-MIT
    │   ├── LICENSE-APACHE
    │   └── deny.toml
    ├── curlosity/
    │   ├── Cargo.toml
    │   ├── src/
    │   ├── tests/
    │   ├── docs/
    │   ├── install.sh
    │   ├── install.ps1
    │   ├── README.md
    │   ├── CHANGELOG.md
    │   ├── LICENSE-MIT
    │   ├── LICENSE-APACHE
    │   └── deny.toml
    ├── readcursively/
    │   ├── Cargo.toml
    │   ├── src/
    │   ├── tests/
    │   ├── docs/
    │   ├── hermes-tool.json
    │   ├── install.sh
    │   ├── install.ps1
    │   ├── README.md
    │   ├── LICENSE-MIT
    │   ├── LICENSE-APACHE
    │   └── deny.toml
    └── recurlsively/
        ├── Cargo.toml
        ├── src/
        ├── tests/
        ├── docs/
        ├── install.sh
        ├── install.ps1
        ├── README.md
        ├── LICENSE-MIT
        ├── LICENSE-APACHE
        ├── LICENSE-APACHE
        ├── SECURITY.md
        ├── CONTRIBUTING.md
        └── deny.toml
```

## Licensing

Each crate is dual-licensed MIT / Apache-2.0. The workspace root inherits
the same dual license.
