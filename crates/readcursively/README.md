# readcursively

Batch code intelligence for AI agents: `search_files` + `grep` + `read_file`
collapsed into one tool call. Pure Rust, edition 2024, no network at runtime.

Why: analysis of ~283k Hermes agent tool calls shows `read_file -> read_file`
49.8%, `search_files -> search_files` 37.2%, and `search_files -> read_file`
32% (4201 occurrences). One readcursively batch replaces that whole chain.

## Install

macOS / Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/riceharvest/agents-first-toolkit/main/crates/readcursively/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/riceharvest/agents-first-toolkit/main/crates/readcursively/install.ps1 | iex
```

Or from source:

```sh
cargo install --git https://github.com/riceharvest/agents-first-toolkit
```

Update an existing install (checksum-verified, in place):

```sh
readcursively --update
```

## Usage

One call: search for a pattern, auto-read every file it hits.

```sh
echo '{
  "root": "~/myrepo",
  "searches": [{"pattern": "fn handle_request", "glob": "*.rs"}],
  "reads": [{"path": "src/config.toml", "offset": 10, "limit": 20}],
  "auto_read": true
}' | readcursively
```

Equivalent one-liner with CLI flags (flags override the request):

```sh
echo '{"searches":[{"pattern":"TODO"}],"auto_read":true}' | readcursively --auto-read --max-results 50
```

Multiple parallel searches + reads in a single call:

```sh
echo '{
  "searches": [
    {"pattern": "unwrap\\(\\)", "file_type": "rust", "max_results": 100},
    {"pattern": "\"error\"", "glob": "src/**/*.ts"},
    {"pattern": "TODO|FIXME"}
  ],
  "reads": [
    {"path": "README.md"},
    {"path": "src/main.rs", "offset": 1, "limit": 50},
    {"path": "src/main.rs"}
  ]
}' | readcursively
```

`reads` are deduplicated, so the repeated `src/main.rs` above is read once.

## Input schema (BatchRequest)

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `root` | string | `.` | Root directory for searches and relative paths |
| `searches` | array | `[]` | Parallel regex searches over file contents |
| `searches[].pattern` | string | required | Ripgrep-style regex (Rust `regex` crate) |
| `searches[].glob` | string | - | Glob filter, e.g. `*.rs`, `src/**/*.ts` |
| `searches[].file_type` | string | - | `rust` `python` `js` `ts` `go` `md` `json` `toml` `yaml` `sh` `c` `cpp` `text` `html` `css` |
| `searches[].max_results` | int | 200 | Per-query hit cap |
| `reads` | array | `[]` | Parallel file reads, deduplicated by path |
| `reads[].path` | string | required | Relative to `root` (absolute also accepted) |
| `reads[].offset` | int | 1 | 1-indexed first line |
| `reads[].limit` | int | all | Max lines to return |
| `auto_read` | bool | false | Also read every file hit by the searches |
| `auto_read_limit` | int | 2000 | Line cap per auto-read file |
| `max_results` | int | 200 | Default per-query cap |
| `max_file_size` | int | 5242880 | Skip files over this many bytes |
| `max_total_chars` | int | 500000 | Truncate total output |
| `max_depth` | int | 64 | Max directory depth |
| `respect_ignore` | bool | true | Respect `.gitignore` / `.ignore` |
| `hidden` | bool | false | Include hidden files |
| `follow_symlinks` | bool | false | Follow symlinked directories |

## Output schema (BatchResult)

```json
{
  "ok": true,
  "searches": [
    {
      "pattern": "hello",
      "hits": [
        {"path": "src/lib.rs", "line": 12, "col": 9, "preview": "    println!(\"hello\");"}
      ]
    }
  ],
  "reads": [
    {
      "path": "src/lib.rs",
      "truncated": false,
      "line_count": 340,
      "display": ["1|use std::io;", "2|", "3|fn main() {"]
    }
  ],
  "total_chars": 1035
}
```

- `ok` is `false` if any search pattern failed to compile or any read errored; per-item details are in each element's `error` field. The batch itself still completes.
- Search `hits` are sorted by path, then line. `col` is the 1-indexed column of the first match on the line.
- Read `display` lines are `"LNUM|content"` with 1-indexed line numbers, ready to pipe or grep.
- `truncated` on a search is the cap that bit; on a read it means lines were cut.
- Binary files are detected (NUL byte / invalid UTF-8 in the first 8 KiB) and skipped with `"error": "binary file skipped"`.

## Flags

| Flag | Effect |
| --- | --- |
| `--input FILE` | Read BatchRequest JSON from a file instead of stdin |
| `--batch JSON` | Read the batch from a string argument |
| `-r, --root DIR` | Override the request's `root` |
| `--auto-read` | Force `auto_read: true` |
| `--hidden` | Include hidden files |
| `--no-ignore` | Disable gitignore respect (search vendored/ignored dirs too) |
| `--max-results N` | Override per-query cap |
| `--max-file-size BYTES` | Override the file size cap |
| `--max-total-chars N` | Override the output cap |
| `--max-depth N` | Override directory depth |
| `--update` | Self-update from GitHub Releases |
| `--completions SHELL` | Emit shell completions (bash/zsh/fish/elvish/powershell) |
| `--hermes-tool` | Print the hermes tool declaration JSON |
| `--emit` | One-shot agent bootstrap: hermes-tool.json + man page + all shell completions, sections delimited by `--- name ---` |
| `--probe DIR` | Cold-start orientation: list a directory's entries as JSON (`kind`, `size`; sorted, capped at 500 entries; default cwd) |
| `-h, --help` / `-V, --version` | Help / version |

## Runnable demo

Copy-paste this whole block: it creates a fixture repo, runs one batch with
search + auto_read, and prints the actual output.

```sh
mkdir -p /tmp/readcursively-demo/src
printf 'fn handle_request() {\n    // TODO: validate input\n}\n' > /tmp/readcursively-demo/src/api.rs
printf '# demo\nsome markdown\n' > /tmp/readcursively-demo/README.md

echo '{"root":"/tmp/readcursively-demo","searches":[{"pattern":"TODO","glob":"*.rs"}],"auto_read":true}' \
  | readcursively | python3 -m json.tool
```

Output (verbatim from v0.1.1):

```json
{
    "ok": true,
    "searches": [
        {
            "pattern": "TODO",
            "hits": [
                {
                    "path": "src/api.rs",
                    "line": 2,
                    "col": 8,
                    "preview": "    // TODO: validate input"
                }
            ]
        }
    ],
    "reads": [
        {
            "path": "/tmp/readcursively-demo/src/api.rs",
            "truncated": false,
            "line_count": 3,
            "display": [
                "1|fn handle_request() {",
                "2|    // TODO: validate input",
                "3|}"
            ]
        }
    ],
    "total_chars": 107
}
```

One call found the TODO and returned the whole file it lives in.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Batch completed (even if individual items reported `error`) |
| 2 | Usage/protocol error: bad JSON, empty batch, unreadable input, bad root |
| 1 | Self-update failed |

## Hermes tool registration

One tool, `readcursively`, minimal schema (`searches` + `reads` +
`auto_read`). After `cargo install`, the declaration JSON is bundled:

```sh
readcursively --hermes-tool > hermes-tool.json
```

Then register it as a single `readcursively` tool with the schema below
(excerpt; full JSON via the command above):

```json
{
  "name": "readcursively",
  "description": "Batch code intelligence: parallel regex searches + parallel file reads in ONE call. auto_read chains search hits into file contents. 1-indexed offset/limit reads, binary skip, gitignore respect, adversarial caps.",
  "input_schema": {
    "type": "object",
    "properties": {
      "searches": [{"pattern": "string", "glob": "string", "file_type": "string", "max_results": "int"}],
      "reads": [{"path": "string", "offset": "int", "limit": "int"}],
      "auto_read": "bool",
      "root": "string"
    }
  }
}
```

## Adversarial hardening

The tool is built to be pointed at untrusted codebases. Every cap below is proven by a test in `tests/integration.rs` (the `adversarial_*` suite):

- **Huge / pathological regex patterns**: patterns over 1024 chars are rejected at compile time (`pattern too long`). The regex crate is RE2-style (no catastrophic backtracking), but measured worst case shows 100k-char literals stall the lazy-DFA scan >25s; at 1024 chars the worst-case match confirmation is ~5ms, so even a fully adversarial file finishes ~1s with the hit cap. Proof: `adversarial_1_huge_pattern_rejected_fast`.
- **Match flooding / huge result DoS**: `max_results` (default 200/query) plus the `max_total_chars` output cap (default 500k, char-boundary-safe truncation with a stderr notice). A 20k-match file returns 200 hits in milliseconds. Proof: `adversarial_5_flood_capped_and_fast`.
- **Binary blob disguised as text**: content-based detection (NUL byte / invalid UTF-8 in the first 8 KiB) skips the file regardless of extension, for both search and read. Proof: `adversarial_2_binary_blob_as_text_file`.
- **Symlink cycles**: symlinks are not followed by default (`follow_symlinks: false`); when explicitly enabled, the `ignore` crate detects cycles. A `a -> b -> a` loop terminates in milliseconds. Proof: `adversarial_3_symlink_cycle_terminates` and `symlink_loops_do_not_hang`.
- **Hidden secret leakage**: `.env`, `.git/`, and dotfile paths are invisible to default searches (hidden + gitignore filtering). Exposure requires the explicit double opt-in `--hidden --no-ignore`. Proof: `adversarial_4_hidden_env_not_leaked_by_default`.
- **Huge file OOM**: files over `max_file_size` (default 5 MiB) are stat-filtered before reading; a 1M-file walk ceiling bounds memory on pathological repos.
- **Bad regexes**: reported per-query as `error`, never panic or kill the batch.

The 1024-char pattern cap is deliberately below ripgrep's unbounded limit: agents rarely need longer patterns, and the failure mode (one query returns `error`, siblings unaffected) is cheap.

## Testing

No network needed:

```sh
cargo test
```

The integration suite runs the real binary: fixture crawl (20 files), search+read batch, auto_read chaining, caps, gitignore/binary handling, symlink loops, completions/man/hermes-tool/emit/probe, the `adversarial_*` cap proofs, and an `install.sh` fail-closed dry-run.

## Release

Tag a version and CI builds 5 targets (linux x86_64+aarch64 musl, macOS x86_64+aarch64, Windows x86_64) with SHA256SUMS:

```sh
git tag v0.1.0 && git push origin v0.1.0
```

The installers verify SHA-256 before installing and fail closed on any mismatch.
