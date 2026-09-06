use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory, Parser};
use clap_complete::{Shell, generate as gen_completions};

/// readcursively: batch code intelligence for AI agents. One call does what
/// used to cost N tool calls (search_files + grep + read_file).
#[derive(Parser, Debug)]
#[command(
    name = "readcursively",
    version,
    about = "Batch code intelligence: parallel regex search + file read in one JSON call",
    long_about = None,
    after_help = "Pipe a BatchRequest JSON to stdin:\n  echo '{\"searches\":[{\"pattern\":\"TODO\"}],\"auto_read\":true}' | readcursively"
)]
struct Cli {
    /// Read BatchRequest JSON from this file instead of stdin.
    #[arg(long, value_name = "FILE")]
    input: Option<PathBuf>,
    /// Read a batch from a JSON string argument instead of stdin.
    #[arg(long, value_name = "JSON")]
    batch: Option<String>,
    /// Root directory for the batch (overrides request "root").
    #[arg(short, long, value_name = "DIR")]
    root: Option<PathBuf>,
    /// Auto-read files hit by searches (overrides request "auto_read").
    #[arg(long)]
    auto_read: bool,
    /// Include hidden files (overrides request "hidden").
    #[arg(long)]
    hidden: bool,
    /// Do not respect .gitignore/.ignore files (overrides request "respect_ignore").
    #[arg(long)]
    no_ignore: bool,
    /// Per-query match cap (overrides request "max_results").
    #[arg(long, value_name = "N")]
    max_results: Option<usize>,
    /// Skip files larger than this many bytes (overrides request "max_file_size").
    #[arg(long, value_name = "BYTES")]
    max_file_size: Option<u64>,
    /// Truncate total output after this many chars (overrides request "max_total_chars").
    #[arg(long, value_name = "N")]
    max_total_chars: Option<usize>,
    /// Maximum directory depth (overrides request "max_depth").
    #[arg(long, value_name = "N")]
    max_depth: Option<usize>,
    /// Self-update from GitHub Releases (checksum-verified).
    #[arg(long)]
    update: bool,
    /// Emit shell completions for the given shell and exit.
    #[arg(long, value_name = "SHELL")]
    completions: Option<Shell>,
    /// Print the hermes tool declaration JSON (hermes-tool.json) and exit.
    #[arg(long)]
    hermes_tool: bool,
    /// One-shot agent bootstrap: emit tool declaration JSON, man page, and all
    /// shell completions in a single call (sections delimited by `--- <name> ---`).
    #[arg(long)]
    emit: bool,
    /// Cold-start probe: list files in the current directory (JSON) so an agent
    /// can orient without a separate read tool. Optional DIR argument; default cwd.
    #[arg(long, value_name = "DIR", num_args = 0..=1, default_missing_value = ".")]
    probe: Option<String>,
}

fn hermes_tool_json() -> String {
    let v = serde_json::json!({
        "name": "readcursively",
        "description": "Batch code intelligence: run parallel ripgrep-style regex searches and parallel file reads in ONE call. Supports globs, file-type filters, auto_read chaining (search results feed file reads automatically), 1-indexed offset/limit reads, binary skip, gitignore respect, and adversarial caps (max_results, max_file_size, max_total_chars, max_depth).",
        "input_schema": {
            "type": "object",
            "properties": {
                "root": {"type": "string", "description": "Root directory for relative paths and searches (default: cwd)"},
                "searches": {
                    "type": "array",
                    "description": "Parallel regex searches over file contents",
                    "items": {
                        "type": "object",
                        "properties": {
                            "pattern": {"type": "string", "description": "Ripgrep-style regex"},
                            "glob": {"type": "string", "description": "Optional glob filter, e.g. *.rs or src/**/*.ts"},
                            "file_type": {"type": "string", "description": "Optional type filter: rust/python/js/ts/go/md/json/toml/yaml/sh/c/cpp/text/html/css"},
                            "max_results": {"type": "integer", "description": "Per-query hit cap (default 200)"}
                        },
                        "required": ["pattern"]
                    }
                },
                "reads": {
                    "type": "array",
                    "description": "Parallel file reads (deduplicated)",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": {"type": "string"},
                            "offset": {"type": "integer", "description": "1-indexed first line"},
                            "limit": {"type": "integer", "description": "Max lines to return"}
                        },
                        "required": ["path"]
                    }
                },
                "auto_read": {"type": "boolean", "description": "Return contents of every file hit by searches too"},
                "auto_read_limit": {"type": "integer", "description": "Line cap per auto-read file (default 2000)"},
                "max_results": {"type": "integer"},
                "max_file_size": {"type": "integer", "description": "Skip files over this many bytes (default 5242880)"},
                "max_total_chars": {"type": "integer", "description": "Truncate total output (default 500000)"},
                "max_depth": {"type": "integer"},
                "respect_ignore": {"type": "boolean", "description": "Respect .gitignore (default true)"},
                "hidden": {"type": "boolean", "description": "Include hidden files (default false)"},
                "follow_symlinks": {"type": "boolean", "description": "Follow symlinked dirs (default false)"}
            },
            "anyOf": [
                {"required": ["searches"]},
                {"required": ["reads"]}
            ]
        }
    });
    serde_json::to_string_pretty(&v).expect("hermes tool json")
}

fn print_man() -> anyhow::Result<()> {
    let cmd = <Cli as CommandFactory>::command();
    let man = clap_mangen::Man::new(cmd);
    let mut buf = Vec::new();
    man.render(&mut buf)?;
    print!("{}", String::from_utf8_lossy(&buf));
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    if cli.emit {
        let mut cmd = <Cli as CommandFactory>::command();
        println!("--- hermes-tool.json ---");
        println!("{}", hermes_tool_json());
        println!("--- man ---");
        let man = clap_mangen::Man::new(cmd.clone());
        let mut buf = Vec::new();
        if man.render(&mut buf).is_ok() {
            println!("{}", String::from_utf8_lossy(&buf));
        }
        for shell in [
            Shell::Bash,
            Shell::Zsh,
            Shell::Fish,
            Shell::Elvish,
            Shell::PowerShell,
        ] {
            println!("--- completions:{} ---", shell);
            gen_completions(shell, &mut cmd, "readcursively", &mut std::io::stdout());
        }
        return ExitCode::SUCCESS;
    }

    if cli.hermes_tool {
        println!("{}", hermes_tool_json());
        return ExitCode::SUCCESS;
    }
    if let Some(shell) = cli.completions {
        let mut cmd = <Cli as CommandFactory>::command();
        gen_completions(shell, &mut cmd, "readcursively", &mut std::io::stdout());
        return ExitCode::SUCCESS;
    }
    if cli.completions.is_none() && std::env::args().any(|a| a == "--man") {
        // man page via clap_mangen, only when the feature is compiled in
        if print_man().is_err() {
            eprintln!("readcursively: man page generation failed");
            return ExitCode::FAILURE;
        }
        return ExitCode::SUCCESS;
    }

    if let Some(dir) = &cli.probe {
        return match readcursively::probe(dir) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("readcursively probe: {e}");
                ExitCode::from(2)
            }
        };
    }

    if cli.update {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        return match rt.block_on(readcursively::update::run_update()) {
            Ok(m) => {
                println!("{m}");
                ExitCode::SUCCESS
            }
            Err(readcursively::update::UpdateError::UpToDate(m)) => {
                println!("{m}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("readcursively update: {e}");
                ExitCode::FAILURE
            }
        };
    }

    // Gather request JSON from --batch, --input, or stdin.
    let raw = if let Some(json) = &cli.batch {
        json.clone()
    } else if let Some(file) = &cli.input {
        match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("readcursively: cannot read {}: {e}", file.display());
                return ExitCode::from(2);
            }
        }
    } else {
        let mut buf = String::new();
        if std::io::stdin().read_to_string(&mut buf).is_err() || buf.trim().is_empty() {
            eprintln!("readcursively: no batch JSON on stdin (or use --input FILE / --batch JSON)");
            return ExitCode::from(2);
        }
        buf
    };

    let mut req: readcursively::BatchRequest = match serde_json::from_str(&raw) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("readcursively: invalid BatchRequest JSON: {e}");
            return ExitCode::from(2);
        }
    };

    // CLI overrides win over request fields.
    if let Some(root) = cli.root {
        req.root = Some(root.to_string_lossy().into_owned());
    }
    if cli.auto_read {
        req.auto_read = true;
    }
    if cli.hidden {
        req.hidden = true;
    }
    if cli.no_ignore {
        req.respect_ignore = false;
    }
    if let Some(n) = cli.max_results {
        req.max_results = Some(n);
    }
    if let Some(n) = cli.max_file_size {
        req.max_file_size = Some(n);
    }
    if let Some(n) = cli.max_total_chars {
        req.max_total_chars = Some(n);
    }
    if let Some(n) = cli.max_depth {
        req.max_depth = Some(n);
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    match rt.block_on(readcursively::batch(req)) {
        Ok(result) => {
            let json = serde_json::to_string(&result).expect("serialize BatchResult");
            let cap = cli
                .max_total_chars
                .unwrap_or(readcursively::DEFAULT_MAX_TOTAL_CHARS);
            let (out, truncated) = readcursively::enforce_total_cap(&json, cap);
            println!("{out}");
            if truncated {
                eprintln!("readcursively: output truncated at {cap} chars");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("readcursively: {e}");
            ExitCode::from(2)
        }
    }
}
