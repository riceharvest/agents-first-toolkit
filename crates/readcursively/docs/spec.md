# Specification

This document is normative for the `readcursively` command. Behavior not
specified here is not promised by the current release.

## Command contract

`readcursively [OPTIONS]` reads a BatchRequest JSON from stdin (or
`--input FILE` / `--batch JSON`) and writes a BatchResult JSON to stdout.
`--help` and `--version` succeed without input. `--update` self-updates the
binary from GitHub Releases. `--completions SHELL` emits shell completions.
`--hermes-tool` prints the tool declaration JSON.

Invalid arguments or invalid BatchRequest JSON produce a human-readable error
on stderr and exit code 2. Exit codes: 0 batch completed (item-level errors
are reported in the payload, not the exit code), 1 update failure, 2
usage/protocol error.

## Batch request

- `searches`: parallel regex searches. `pattern` required; optional `glob`,
  `file_type`, `max_results`. Patterns use the Rust `regex` crate syntax
  (ripgrep-compatible for common cases).
- `reads`: parallel file reads, deduplicated by path. `path` required;
  optional `offset` (1-indexed first line) and `limit`.
- `auto_read`: when true, every file hit by a search is also read.
- `root`: base directory for searches and relative paths.
- Caps: `max_results` (default 200), `max_file_size` (default 5 MiB),
  `max_total_chars` (default 500k), `max_depth` (default 64),
  `auto_read_limit` (default 2000).
- Policy: `respect_ignore` (default true), `hidden` (default false),
  `follow_symlinks` (default false).

CLI flags of the same name override request fields.

## Batch result

- `ok`: false if any pattern failed to compile or any read errored.
- `searches[]`: `pattern`, `error?`, `truncated?` (the cap that bit), `hits[]`
  with `path`, `line` (1-indexed), `col` (1-indexed), `preview` (max 240
  chars), sorted by path then line.
- `reads[]`: `path`, `error?`, `truncated?`, `line_count?`, `display[]` of
  `"LNUM|content"` strings (1-indexed, max 2000 chars per line).
- `total_chars`: size of the returned payload before any global truncation.

Binary files are skipped (NUL byte or invalid UTF-8 in the first 8 KiB) and
reported as `"binary file skipped"`.

## MVP boundaries

The MVP searches and reads local text files. It does not write files, run
commands, follow network links, or index binaries. Any change to the request
or result schema requires updating tests, README.md, and this specification
together.
