# Architecture

`readcursively` is a local Rust binary that collapses the agent tool-call
chain (search -> read, read -> read, search -> search) into one batch call.
Deterministic, bounded behavior; pure Rust; no network at runtime (only
`--update` touches the network).

## Boundaries

1. **CLI (`src/main.rs`)** parses arguments with clap, gathers BatchRequest
   JSON from `--batch` / `--input` / stdin, applies CLI overrides, prints
   BatchResult JSON to stdout. Also emits shell completions (`--completions`),
   a man page (`--man`), the hermes tool declaration (`--hermes-tool`), and
   self-updates (`--update`, `src/update.rs`).
2. **Batch engine (`src/lib.rs`)** compiles regexes up front (bad patterns
   become per-query errors, never panics), groups queries by their
   (glob, file_type) filter so the walker runs once per distinct filter,
   then executes all queries in parallel via rayon over the shared file list.
3. **Walker** uses the `ignore` crate (gitignore/hidden filtering, parallel
   walk, symlink loop safety) with a size pre-filter and a 1M-file ceiling.
4. **Reader** reads files in parallel via rayon + std fs (the async surface
   is a thin wrapper; the work is CPU/IO-bound blocking, rayon is the right
   executor). Binary detection, size caps, 1-indexed offset/limit,
   deduplication, `LNUM|content` display lines.
5. **Caps layer** enforces `max_results`, `max_file_size`, `max_total_chars`
   (char-boundary-safe truncation), `max_depth`, `auto_read_limit`.

## Design decisions

- **Search and read share one process**: the 32% search->read bigram dies via
  `auto_read` (hit paths feed the reader directly, deduplicated).
- **Glob + file_type are both globsets** under the hood; file types are
  extension lists compiled to `**/*.<ext>` globs. A query may set both.
- **Errors are data, not aborts**: one missing file or one bad regex does not
  fail a 50-item batch; `ok: false` plus per-item `error` reports it.
- **Limits are layered**: per-query caps -> global output cap -> hard walk
  ceiling, so adversarial repos flood nothing.

## Security model

The tool is meant to run against untrusted codebases. All default limits are
documented in README.md; every limit is overridable explicitly, and the
overrides appear in the request schema so an orchestrator can audit them.
Symlinks are not followed by default; gitignore is respected by default;
binary files are skipped by content, not extension.

## Testing

- Unit tests in `src/lib.rs`: regex/glob/file-type matching, offset/limit,
  dedup, binary skip, caps, gitignore, symlinks, char-boundary truncation.
- Integration tests in `tests/integration.rs` drive the compiled binary:
  help/version/completions/hermes-tool, the 20-file fixture crawl, auto_read
  chaining, caps end-to-end, error handling, symlink loops, and the
  fail-closed installer dry-run.
