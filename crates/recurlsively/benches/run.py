#!/usr/bin/env python3
"""Scenarios, result parsing, and summary writer for recurlsively benchmarks."""

from __future__ import annotations

import csv
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field, asdict
from pathlib import Path
from typing import Any, Sequence


REPO_ROOT = Path(__file__).resolve().parents[2]
BINARY = REPO_ROOT / "target" / "debug" / "recurlsively"
SUMMARY_PATH = REPO_ROOT / "benches" / "results" / "latest.csv"


@dataclass
class CrawlReport:
    pages_written: int = 0
    pages_failed: int = 0
    pages_skipped: int = 0
    pages_pending: int = 0
    changed: int = 0
    unchanged: int = 0
    truncated: bool = False

    @classmethod
    def from_text(cls, text: str) -> CrawlReport:
        report = cls()
        for line in text.splitlines():
            line = line.strip()
            if not line:
                continue
            if ":" not in line:
                continue
            _, rest = line.split(":", 1)
            parts = rest.strip().split()
            if len(parts) >= 8:
                try:
                    report.pages_written = int(parts[1])
                    report.pages_failed = int(parts[3])
                    report.pages_skipped = int(parts[5])
                    report.pages_pending = int(parts[7])
                except ValueError:
                    pass
            if "truncated" in line:
                report.truncated = "truncated 1" in line or line.endswith("truncated 1")
        return report

    @classmethod
    def from_json(cls, payload: str) -> CrawlReport:
        data = json.loads(payload)
        totals = data.get("totals", {})
        return cls(
            pages_written=int(totals.get("pages_written", 0)),
            pages_failed=int(totals.get("pages_failed", 0)),
            pages_skipped=int(totals.get("pages_skipped", 0)),
            pages_pending=int(totals.get("pages_pending", 0)),
            changed=int(totals.get("changed", 0)),
            unchanged=int(totals.get("unchanged", 0)),
            truncated=bool(totals.get("truncated", False)),
        )


@dataclass
class CaseResult:
    case: str
    exit_code: int
    wall_ms: float
    output_bytes: int = 0
    manifest_records: int = 0
    error_records: int = 0
    pages_written: int = 0
    pages_failed: int = 0
    pages_skipped: int = 0
    pages_pending: int = 0
    changed: int = 0
    unchanged: int = 0
    truncated: bool = False
    stderr: str = ""
    stdout_tail: str = ""
    output_path: str = ""


def measure_output_bytes(path: Path) -> int:
    total = 0
    if not path.exists():
        return total
    for entry in path.rglob("*"):
        if entry.is_file():
            total += entry.stat().st_size
    return total


def parse_report(stdout: str, report_format: str) -> CrawlReport:
    if report_format == "json":
        return CrawlReport.from_json(stdout.strip())
    return CrawlReport.from_text(stdout)


@dataclass
class Case:
    name: str
    args: Sequence[str]
    expect_written_min: int = 0
    report_format: str = "text"
    fresh: bool = False
    resume_after: bool = False
    token_query: str | None = None
    search_after: bool = False


CASES: list[Case] = [
    Case(
        name="baseline_full",
        args=[
            str(BINARY), "crawl", "http://localhost:8123/docs", "-o", "bench/out/baseline_full",
            "--max-pages", "64", "--concurrency", "8", "--max-depth", "3",
            "--report", "json", "--progress", "none", "--sitemap", "off",
            "--allow-private-network",
        ],
        expect_written_min=6,
        report_format="json",
        fresh=True,
    ),
    Case(
        name="high_concurrency",
        args=[
            str(BINARY), "crawl", "http://localhost:8123/docs", "-o", "bench/out/high_concurrency",
            "--max-pages", "64", "--concurrency", "32", "--max-depth", "3",
            "--report", "json", "--progress", "none", "--sitemap", "off",
            "--allow-private-network",
        ],
        expect_written_min=6,
        report_format="json",
        fresh=True,
    ),
    Case(
        name="deep_crawl",
        args=[
            str(BINARY), "crawl", "http://localhost:8123/docs", "-o", "bench/out/deep_crawl",
            "--max-pages", "128", "--concurrency", "8", "--max-depth", "6",
            "--report", "json", "--progress", "none", "--sitemap", "off",
            "--allow-private-network",
        ],
        expect_written_min=32,
        report_format="json",
        fresh=True,
    ),
    Case(
        name="shallow_relevant",
        args=[
            str(BINARY), "crawl", "http://localhost:8123/docs", "-o", "bench/out/shallow_relevant",
            "--max-pages", "32", "--concurrency", "8", "--max-depth", "2",
            "--report", "json", "--progress", "none", "--sitemap", "off",
            "--for", "install", "--allow-private-network",
        ],
        expect_written_min=1,
        report_format="json",
        fresh=True,
    ),
    Case(
        name="relevance_pruned",
        args=[
            str(BINARY), "crawl", "http://localhost:8123/docs", "-o", "bench/out/relevance_pruned",
            "--max-pages", "32", "--concurrency", "8", "--max-depth", "2",
            "--report", "json", "--progress", "none", "--sitemap", "off",
            "--for", "install", "--for-prune", "--allow-private-network",
        ],
        expect_written_min=1,
        report_format="json",
        fresh=True,
    ),
    Case(
        name="resume_noop",
        args=[
            str(BINARY), "crawl", "http://localhost:8123/docs", "-o", "bench/out/resume_noop",
            "--max-pages", "64", "--concurrency", "8", "--max-depth", "3",
            "--report", "json", "--progress", "none", "--sitemap", "off",
            "--allow-private-network",
        ],
        expect_written_min=6,
        report_format="json",
        fresh=False,
        resume_after=True,
    ),
]


def token_counts(text: str) -> dict[str, int]:
    words = [token for token in text.replace("\n", " ").split(" ") if token]
    return {"word_tokens": len(words), "chars": len(text)}


def run_case(case: Case) -> CaseResult:
    output_path = Path(case.args[case.args.index("-o") + 1])
    if case.fresh and output_path.exists():
        shutil.rmtree(output_path, ignore_errors=True)

    start = time.perf_counter()
    proc = subprocess.run(case.args, cwd=REPO_ROOT, capture_output=True, text=True)
    wall_ms = (time.perf_counter() - start) * 1000
    stdout = proc.stdout or ""
    stderr = proc.stderr or ""
    report = parse_report(stdout, case.report_format)

    output_bytes = measure_output_bytes(output_path)
    manifest = output_path.joinpath("manifest.jsonl")
    manifest_records = sum(1 for _ in manifest.open("r", errors="ignore")) if manifest.exists() else 0
    errors = output_path.joinpath("errors.jsonl")
    error_records = sum(1 for _ in errors.open("r", errors="ignore")) if errors.exists() else 0

    tail = "\n".join(stdout.strip().splitlines()[-8:])

    return CaseResult(
        case=case.name,
        exit_code=proc.returncode,
        wall_ms=wall_ms,
        output_bytes=output_bytes,
        manifest_records=manifest_records,
        error_records=error_records,
        pages_written=report.pages_written,
        pages_failed=report.pages_failed,
        pages_skipped=report.pages_skipped,
        pages_pending=report.pages_pending,
        changed=report.changed,
        unchanged=report.unchanged,
        truncated=report.truncated,
        stderr=stderr,
        stdout_tail=tail,
        output_path=str(output_path),
    )


def write_summary(results: Sequence[CaseResult]) -> Path:
    SUMMARY_PATH.parent.mkdir(parents=True, exist_ok=True)
    fieldnames = [
        "case", "exit_code", "wall_ms", "pages_written", "pages_failed",
        "pages_skipped", "pages_pending", "changed", "unchanged", "truncated",
        "output_bytes", "manifest_records", "error_records", "output_path", "stderr", "stdout_tail",
    ]
    with SUMMARY_PATH.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=fieldnames)
        writer.writeheader()
        for result in results:
            writer.writerow({field: getattr(result, field) for field in fieldnames})
    return SUMMARY_PATH


def main() -> int:
    print(f"Using binary: {BINARY}")
    if not BINARY.exists():
        print(f"Missing binary: {BINARY}")
        return 2

    print("Starting local fixture server...")
    server_proc = subprocess.Popen(
        [sys.executable, str(REPO_ROOT / "benches" / "server.py"), "--port", "8123"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        time.sleep(1.5)
        results = [run_case(case) for case in CASES]
    finally:
        server_proc.terminate()
        try:
            server_proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            server_proc.kill()

    path = write_summary(results)
    print(f"Wrote summary: {path}")
    for result in results:
        status = "OK" if result.exit_code == 0 and result.pages_written >= 1 else "FAIL"
        print(
            f"[{status}] {result.case}: wall={result.wall_ms:0.1f}ms "
            f"written={result.pages_written} failed={result.pages_failed} "
            f"skipped={result.pages_skipped} pending={result.pages_pending} "
            f"changed={result.changed} unchanged={result.unchanged} truncated={result.truncated} "
            f"output_bytes={result.output_bytes} manifest={result.manifest_records} errors={result.error_records}"
        )
        if result.stdout_tail:
            print(result.stdout_tail)
        if result.stderr.strip():
            print("STDERR:")
            print("\n".join(result.stderr.strip().splitlines()[-8:]))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
