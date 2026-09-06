use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

// Live integration matrix: exercises the debug binary the way an agent would.
// Runs the fixture crawl (20 files), search+read batch, auto_read, caps,
// completions, man, hermes-tool, and a Windows-free install.sh dry-run.

fn bin() -> &'static str {
    // cargo test builds it before tests run (see Cargo.toml integration notes below).
    env!("CARGO_BIN_EXE_readcursively")
}

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new(files: usize) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        // pkg dirs always exist so the extra fixture files below can land.
        for p in ["pkg0", "pkg1", "pkg2", "pkg3"] {
            std::fs::create_dir_all(root.join(p)).unwrap();
        }
        // 20+ small rust/python/md/text files across nested dirs.
        for i in 0..files {
            let sub = root.join(format!("pkg{}", i % 4));
            let f = sub.join(format!("file{i}.rs"));
            std::fs::write(&f, format!("// TODO: fix bug{i}\nfn handler_{i}() {{\n    let needle_{i} = {i};\n    todo!()\n}}\n"))
                .unwrap();
        }
        // a python file, an md file, a gitignored secret, and a binary blob
        std::fs::write(
            root.join("pkg0/script.py"),
            "def run():\n    return 'needle here'\n",
        )
        .unwrap();
        std::fs::write(root.join("docs.md"), "# doc\nneedle in docs\n").unwrap();
        std::fs::write(root.join(".gitignore"), "secret*\n*.bin\n").unwrap();
        std::fs::write(root.join("secret.env"), "needle=leaked\n").unwrap();
        std::fs::write(root.join("pkg1/blob.bin"), b"\x00\x01needle\x00").unwrap();
        // symlink loop guard: a -> b -> a
        #[cfg(unix)]
        {
            let a = root.join("loopy_a");
            let b = root.join("loopy_b");
            std::os::unix::fs::symlink(&b, &a).unwrap();
            std::os::unix::fs::symlink(&a, &b).unwrap();
        }
        Self { dir }
    }

    fn run(&self, args: &[&str], stdin: Option<&str>) -> (String, i32) {
        use std::io::Write;
        use std::process::Stdio;
        let mut child = Command::new(bin())
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn binary");
        if let Some(s) = stdin {
            child.stdin.take().unwrap().write_all(s.as_bytes()).unwrap();
        }
        let out = child.wait_with_output().expect("wait");
        (
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
            out.status.code().unwrap_or(-1),
        )
    }
}

#[test]
fn help_version_completions_hermes_tool() {
    let fx = Fixture::new(0);
    let (out, code) = fx.run(&["--help"], None);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("--auto-read"), "{out}");
    let (out, code) = fx.run(&["--version"], None);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains(env!("CARGO_PKG_VERSION")), "{out}");
    let (out, code) = fx.run(&["--completions", "bash"], None);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("readcursively"), "{out}");
    let (out, code) = fx.run(&["--hermes-tool"], None);
    assert_eq!(code, 0, "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).expect("hermes-tool.json is valid JSON");
    assert_eq!(v["name"], "readcursively");
    assert!(v["input_schema"]["properties"]["searches"].is_object());
}

#[test]
fn fixture_crawl_search_and_read_batch() {
    let fx = Fixture::new(20);
    let root = fx.dir.path().to_string_lossy().into_owned();
    let batch = serde_json::json!({
        "root": root,
        "searches": [
            {"pattern": r"needle_\d+", "glob": "*.rs"},
            {"pattern": "needle", "file_type": "md"}
        ],
        "reads": [
            {"path": "docs.md"},
            {"path": "docs.md"},            // duplicate: deduped
            {"path": "pkg0/script.py", "offset": 2, "limit": 1},
            {"path": "pkg1/blob.bin"},      // binary: skipped
            {"path": "missing.rs"}          // missing: reported
        ],
        "auto_read": false
    })
    .to_string();
    let (out, code) = fx.run(&[], Some(&batch));
    assert_eq!(code, 0, "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON out");
    assert!(
        !v["ok"].as_bool().unwrap(),
        "ok must be false: missing.rs read errors (per-item errors reported, batch still completes)"
    );
    let searches = v["searches"].as_array().unwrap();
    assert_eq!(searches.len(), 2);
    // 20 .rs files, each has one "needle_N" per file... actually the TODO comment
    // line also contains "TODO", not needle. needle_i appears once per file.
    assert_eq!(searches[0]["hits"].as_array().unwrap().len(), 20, "{}", out);
    assert_eq!(searches[1]["hits"].as_array().unwrap().len(), 1);
    let reads = v["reads"].as_array().unwrap();
    // docs.md once (deduped), script.py, blob.bin, missing.rs = 4
    assert_eq!(reads.len(), 4, "{}", out);
    assert!(
        !v["ok"].as_bool().unwrap(),
        "ok must be false when a read errors (missing.rs)"
    );
    let blob = reads
        .iter()
        .find(|r| r["path"].as_str().unwrap().contains("blob.bin"))
        .unwrap();
    assert_eq!(blob["error"], "binary file skipped");
    let missing = reads
        .iter()
        .find(|r| r["path"].as_str().unwrap().contains("missing.rs"))
        .unwrap();
    assert!(missing["error"].is_string());
    let docs = reads
        .iter()
        .find(|r| r["path"].as_str().unwrap().contains("docs.md"))
        .unwrap();
    assert!(
        docs["display"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l.as_str().unwrap().contains("needle in docs"))
    );
    let script = reads
        .iter()
        .find(|r| r["path"].as_str().unwrap().contains("script.py"))
        .unwrap();
    let lines = script["display"].as_array().unwrap();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].as_str().unwrap().starts_with("2|"));
}

#[test]
fn auto_read_chains_hits_into_reads() {
    let fx = Fixture::new(6);
    let root = fx.dir.path().to_string_lossy().into_owned();
    let batch = serde_json::json!({
        "root": root,
        "searches": [{"pattern": "TODO"}],
        "auto_read": true,
        "auto_read_limit": 10
    })
    .to_string();
    let (out, code) = fx.run(&[], Some(&batch));
    assert_eq!(code, 0, "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).expect("json");
    let reads = v["reads"].as_array().unwrap();
    // 6 .rs files hit -> 6 auto-reads
    assert_eq!(reads.len(), 6, "{}", out);
    assert!(reads.iter().all(|r| r["display"].is_array()));
    // display lines are LNUM|content
    let first = reads[0]["display"].as_array().unwrap()[0].as_str().unwrap();
    assert!(first.contains('|'), "{first}");
    assert!(
        first
            .split('|')
            .next()
            .unwrap()
            .chars()
            .all(|c| c.is_ascii_digit())
    );
}

#[test]
fn gitignored_and_binary_excluded_unless_flags() {
    let fx = Fixture::new(4);
    let root = fx.dir.path().to_string_lossy().into_owned();
    let batch = serde_json::json!({
        "root": root,
        "searches": [{"pattern": "needle", "max_results": 100}]
    })
    .to_string();
    let (out, code) = fx.run(&[], Some(&batch));
    assert_eq!(code, 0, "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let hits = v["searches"][0]["hits"].as_array().unwrap();
    // default: gitignored secret.env and binary blob.bin excluded; script.py + docs.md remain
    assert!(
        !hits
            .iter()
            .any(|h| h["path"].as_str().unwrap().contains("secret")),
        "{out}"
    );
    assert!(
        !hits
            .iter()
            .any(|h| h["path"].as_str().unwrap().contains("blob.bin")),
        "{out}"
    );
    assert!(
        hits.iter()
            .any(|h| h["path"].as_str().unwrap().contains("script.py"))
    );
    assert!(
        hits.iter()
            .any(|h| h["path"].as_str().unwrap().contains("docs.md"))
    );

    // --no-ignore --hidden: secret.env now included
    let batch2 =
        serde_json::json!({"root": root, "searches": [{"pattern": "needle", "max_results": 100}]})
            .to_string();
    let (out2, code2) = fx.run(&["--no-ignore", "--hidden"], Some(&batch2));
    assert_eq!(code2, 0, "{out2}");
    let v2: serde_json::Value = serde_json::from_str(&out2).unwrap();
    let hits2 = v2["searches"][0]["hits"].as_array().unwrap();
    assert!(
        hits2
            .iter()
            .any(|h| h["path"].as_str().unwrap().contains("secret.env")),
        "{out2}"
    );
    // binary still excluded even with --no-ignore (binary detection is content-based)
    assert!(
        !hits2
            .iter()
            .any(|h| h["path"].as_str().unwrap().contains("blob.bin"))
    );
}

#[test]
fn caps_enforced_end_to_end() {
    let fx = Fixture::new(8);
    let root = fx.dir.path().to_string_lossy().into_owned();
    let batch = serde_json::json!({
        "root": root,
        "searches": [{"pattern": r"fn|let|todo|TODO|//|\{|\}"}],
        "max_results": 5,
        "max_total_chars": 5000
    })
    .to_string();
    let (out, _code) = fx.run(&[], Some(&batch));
    let v: serde_json::Value = serde_json::from_str(&out).expect("json");
    let hits = v["searches"][0]["hits"].as_array().unwrap();
    assert!(hits.len() <= 5, "{out}");
    assert_eq!(v["searches"][0]["truncated"], 5, "{out}");
    assert!(out.len() < 500_000, "output not capped");
}

#[test]
fn symlink_loops_do_not_hang() {
    let fx = Fixture::new(2);
    let root = fx.dir.path().to_string_lossy().into_owned();
    let batch = serde_json::json!({"root": root, "searches": [{"pattern": "needle"}]}).to_string();
    let started = SystemTime::now();
    let (out, code) = fx.run(&[], Some(&batch));
    assert_eq!(code, 0, "{out}");
    assert!(
        started.elapsed().unwrap().as_secs() < 10,
        "symlink loop hung"
    );
}

#[test]
fn empty_and_bad_requests_fail_cleanly() {
    let fx = Fixture::new(1);
    let (out, code) = fx.run(&[], Some("not json"));
    assert_ne!(code, 0, "{out}");
    assert!(out.contains("invalid BatchRequest"), "{out}");
    let (out, code) = fx.run(&[], Some("{}"));
    assert_ne!(code, 0, "{out}");
    assert!(out.contains("empty batch"), "{out}");
    let (out, code) = fx.run(&[], None);
    assert_ne!(code, 0, "{out}");
}

#[test]
fn install_sh_dry_run_fails_closed_offline() {
    // Without network and without a release, the installer must fail closed
    // (non-zero) rather than half-install. We only assert it does not crash the shell.
    if !Path::new("install.sh").exists() {
        // run from repo root via CARGO_MANIFEST_DIR
        let manifest = env!("CARGO_MANIFEST_DIR");
        std::env::set_current_dir(manifest).unwrap();
    }
    let out = Command::new("sh")
        .arg("install.sh")
        .env("READCURSIVELY_VERSION", "v0.0.0-nonexistent")
        .output()
        .expect("run install.sh");
    assert!(
        !out.status.success(),
        "installer must fail closed for a nonexistent version"
    );
}

// ---------------------------------------------------------------------------
// Adversarial cap proofs - each test pins one attack to a measurable bound.
// ---------------------------------------------------------------------------

#[test]
fn adversarial_1_huge_pattern_rejected_fast() {
    // 100k-char pattern used to stall the binary >25s (lazy-DFA pathology on
    // huge literals). Now rejected at compile time, bounded well under 2s.
    let fx = Fixture::new(2);
    let root = fx.dir.path().to_string_lossy().into_owned();
    let big = "a".repeat(100_000);
    let batch = serde_json::json!({
        "root": root,
        "searches": [{"pattern": big}, {"pattern": "needle"}]
    })
    .to_string();
    let started = SystemTime::now();
    let (out, code) = fx.run(&[], Some(&batch));
    let secs = started.elapsed().unwrap().as_secs();
    assert_eq!(code, 0, "{out}");
    assert!(secs < 2, "huge pattern must be rejected fast, took {secs}s");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let s = &v["searches"];
    assert!(
        s[0]["error"].as_str().unwrap().contains("pattern too long"),
        "{out}"
    );
    assert!(s[0]["hits"].as_array().unwrap().is_empty());
    // the healthy sibling query still ran
    assert_eq!(s[1]["pattern"], "needle");
    // also prove the boundary: exactly MAX chars is fine
    let at_cap = "n".repeat(readcursively::MAX_PATTERN_CHARS);
    let batch2 = serde_json::json!({"root": root, "searches": [{"pattern": at_cap}]}).to_string();
    let (out2, code2) = fx.run(&[], Some(&batch2));
    assert_eq!(code2, 0, "{out2}");
    let v2: serde_json::Value = serde_json::from_str(&out2).unwrap();
    assert!(v2["searches"][0]["error"].is_null(), "{out2}");
}

#[test]
fn adversarial_2_binary_blob_as_text_file() {
    // 100k random bytes named .txt: must be classified binary and skipped
    // for both search and read, without poisoning output.
    let fx = Fixture::new(1);
    let root = fx.dir.path();
    let blob: Vec<u8> = (0..100_000u32).map(|i| (i * 7919 % 256) as u8).collect();
    std::fs::write(root.join("fake.txt"), &blob).unwrap();
    let batch = serde_json::json!({
        "root": root.to_string_lossy().into_owned(),
        "searches": [{"pattern": "."}],
        "reads": [{"path": "fake.txt"}]
    })
    .to_string();
    let (out, code) = fx.run(&[], Some(&batch));
    assert_eq!(code, 0, "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(
        v["searches"][0]["hits"]
            .as_array()
            .unwrap()
            .iter()
            .all(|h| h["path"].as_str().unwrap() != "fake.txt")
    );
    assert_eq!(v["reads"][0]["error"], "binary file skipped");
}

#[test]
fn adversarial_3_symlink_cycle_terminates() {
    // a -> b -> a cycle must not hang or recurse unboundedly (follow_links
    // is off by default; even on, the ignore crate breaks cycles).
    let fx = Fixture::new(1);
    let root = fx.dir.path().to_string_lossy().into_owned();
    #[cfg(unix)]
    {
        let a = fx.dir.path().join("cyc_a");
        let b = fx.dir.path().join("cyc_b");
        std::os::unix::fs::symlink(&b, &a).unwrap();
        std::os::unix::fs::symlink(&a, &b).unwrap();
    }
    let batch = serde_json::json!({"root": root, "searches": [{"pattern": "needle"}]}).to_string();
    let started = SystemTime::now();
    let (out, code) = fx.run(&[], Some(&batch));
    let secs = started.elapsed().unwrap().as_secs();
    assert_eq!(code, 0, "{out}");
    assert!(secs < 5, "symlink cycle must terminate fast, took {secs}s");
    // default follow_symlinks=false: cycle links are invisible to results
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(
        v["searches"][0]["hits"]
            .as_array()
            .unwrap()
            .iter()
            .all(|h| !h["path"].as_str().unwrap().contains("cyc_"))
    );
}

#[test]
fn adversarial_4_hidden_env_not_leaked_by_default() {
    // .env and .config/creds.yaml with a unique marker must be invisible to
    // default searches; only the explicit --hidden --no-ignore opt-in sees them.
    let fx = Fixture::new(1);
    let root = fx.dir.path();
    std::fs::write(root.join(".env"), "API_KEY=zqxjkwvunique\n").unwrap();
    std::fs::create_dir_all(root.join(".config")).unwrap();
    std::fs::write(root.join(".config/creds.yaml"), "API_KEY=zqxjkwvunique\n").unwrap();
    let batch = serde_json::json!({
        "root": root.to_string_lossy().into_owned(),
        "searches": [{"pattern": "zqxjkwvunique"}]
    })
    .to_string();
    let (out, code) = fx.run(&[], Some(&batch));
    assert_eq!(code, 0, "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let paths: Vec<&str> = v["searches"][0]["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["path"].as_str().unwrap())
        .collect();
    // request JSON is not a file in root, so nothing may match by default
    // (fixture .rs files exist but don't contain the marker)
    assert!(
        paths
            .iter()
            .all(|p| !p.ends_with(".env") && !p.contains("creds.yaml")),
        "{paths:?}"
    );

    let (out2, code2) = fx.run(&["--hidden", "--no-ignore"], Some(&batch));
    assert_eq!(code2, 0, "{out2}");
    let v2: serde_json::Value = serde_json::from_str(&out2).unwrap();
    let paths2: Vec<&str> = v2["searches"][0]["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["path"].as_str().unwrap())
        .collect();
    assert!(paths2.contains(&".env"), "{paths2:?}");
    assert!(paths2.contains(&".config/creds.yaml"), "{paths2:?}");
}

#[test]
fn adversarial_5_flood_capped_and_fast() {
    // 20k matching lines in one file: default max_results (200) + total char
    // cap must bound output, and the scan must finish fast.
    let fx = Fixture::new(1);
    let root = fx.dir.path().to_string_lossy().into_owned();
    let line = "matchme line\n".repeat(20_000);
    std::fs::write(fx.dir.path().join("flood.txt"), &line).unwrap();
    let batch = serde_json::json!({"root": root, "searches": [{"pattern": "matchme"}]}).to_string();
    let started = SystemTime::now();
    let (out, code) = fx.run(&[], Some(&batch));
    let secs = started.elapsed().unwrap().as_secs();
    assert_eq!(code, 0, "{out}");
    assert!(secs < 5, "flood must be capped fast, took {secs}s");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let s = &v["searches"][0];
    assert_eq!(
        s["hits"].as_array().unwrap().len(),
        readcursively::DEFAULT_MAX_RESULTS
    );
    assert_eq!(s["truncated"], readcursively::DEFAULT_MAX_RESULTS);
    assert!(
        out.len() < readcursively::DEFAULT_MAX_TOTAL_CHARS,
        "output must respect total cap"
    );
}

#[test]
fn emit_and_probe_qol() {
    let fx = Fixture::new(1);
    // --emit: one-shot dump with all sections
    let (out, code) = fx.run(&["--emit"], None);
    assert_eq!(code, 0, "{out}");
    for marker in [
        "--- hermes-tool.json ---",
        "--- man ---",
        "--- completions:bash ---",
        "--- completions:zsh ---",
    ] {
        assert!(out.contains(marker), "missing {marker}");
    }
    // --probe: cwd listing JSON with sorted entries
    let (out, code) = fx.run(&["--probe"], None);
    assert_eq!(code, 0, "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).expect("probe JSON");
    assert!(v["entries"].is_array());
    assert!(v["total"].as_u64().unwrap() > 0);
    // --probe on a bad dir exits 2
    let (out2, code2) = fx.run(&["--probe", "/definitely-not-here"], None);
    assert_eq!(code2, 2, "{out2}");
}
