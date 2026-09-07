#!/usr/bin/env python3
"""Compare recurlsively against alternative crawl/extraction tools."""

from __future__ import annotations

import csv
import json
import os
import re
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from html.parser import HTMLParser
from pathlib import Path
from typing import Callable

from bs4 import BeautifulSoup
from lxml_html_clean import Cleaner
from readability import Document
import html2text
import requests

REPO_ROOT = Path(__file__).resolve().parents[2]
BINARY = REPO_ROOT / "target" / "debug" / "recurlsively"
SUMMARY_PATH = REPO_ROOT / "benches" / "results" / "compare.csv"
FIXTURE_DIR = REPO_ROOT / "benches" / "fixtures" / "static"
SERVER_SCRIPT = REPO_ROOT / "benches" / "server.py"
PYTHON = sys.executable


@dataclass
class CaseResult:
    case: str
    exit_code: int
    wall_ms: float
    output_bytes: int = 0
    file_count: int = 0
    pages_fetched: int = 0
    ms_per_page: float = 0.0
    bytes_per_page: float = 0.0
    stderr: str = ""
    stdout_tail: str = ""
    output_path: str = ""


def finalize(result: CaseResult) -> CaseResult:
    if result.pages_fetched > 0:
        result.ms_per_page = result.wall_ms / result.pages_fetched
        result.bytes_per_page = result.output_bytes / result.pages_fetched
    return result


def measure_output(path: Path) -> tuple[int, int]:
    total_bytes = 0
    files = 0
    if not path.exists():
        return total_bytes, files
    for entry in path.rglob("*"):
        if entry.is_file():
            files += 1
            try:
                total_bytes += entry.stat().st_size
            except OSError:
                pass
    return total_bytes, files


def tail(text: str, n: int = 12) -> str:
    lines = text.splitlines()
    return "\n".join(lines[-n:])


def ensure_empty(path: Path) -> None:
    if path.exists():
        shutil.rmtree(path, ignore_errors=True)
    path.mkdir(parents=True, exist_ok=True)


def fetch_links(start_url: str, max_pages: int = 64, max_depth: int = 3) -> list[tuple[str, int]]:
    urls: list[tuple[str, int]] = []
    seen: set[str] = set()
    queue: list[tuple[str, int]] = [(start_url, 0)]
    seen.add(start_url)
    html_re = re.compile(r"<!DOCTYPE html|<html", re.I)
    link_re = re.compile(r"href=\"([^\"]+)\"")
    while queue and len(urls) < max_pages:
        current, depth = queue.pop(0)
        try:
            proc = subprocess.run(
                ["curl", "-sS", "-L", "--max-time", "10", current],
                capture_output=True,
                text=True,
                check=False,
            )
            body = proc.stdout
        except OSError:
            continue
        if proc.returncode != 0 or not html_re.search(body):
            continue
        urls.append((current, depth))
        if depth >= max_depth:
            continue
        for match in link_re.finditer(body):
            candidate = match.group(1)
            if candidate.startswith("/"):
                candidate = start_url.rstrip("/") + candidate
            elif candidate.startswith("./"):
                candidate = start_url.rstrip("/") + "/" + candidate[2:]
            if not candidate.startswith("http://localhost:8123/") or candidate in seen:
                continue
            seen.add(candidate)
            queue.append((candidate, depth + 1))
    return urls


def run_case(name: str, args: list[str], output_dir: Path, env: dict[str, str] | None = None) -> CaseResult:
    ensure_empty(output_dir)
    start = time.perf_counter()
    proc = subprocess.run(args, capture_output=True, text=True, cwd=REPO_ROOT, env=env)
    wall_ms = (time.perf_counter() - start) * 1000
    output_bytes, files = measure_output(output_dir)
    return finalize(
        CaseResult(
            case=name,
            exit_code=proc.returncode,
            wall_ms=wall_ms,
            output_bytes=output_bytes,
            file_count=files,
            stdout_tail=tail(proc.stdout),
            stderr=tail(proc.stderr),
            output_path=str(output_dir),
        )
    )


class SimpleTextExtractor:
    def __init__(self) -> None:
        self.parser = HTMLParser()
        self._skip = False
        self._buf: list[str] = []

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str]]) -> None:
        if tag in {"script", "style"}:
            self._skip = True

    def handle_endtag(self, tag: str) -> None:
        if tag in {"script", "style"}:
            self._skip = False
        elif tag in {"p", "div", "br", "li", "h1", "h2", "h3", "h4", "h5", "h6"}:
            self._buf.append("\n")

    def handle_data(self, data: str) -> None:
        if not self._skip:
            text = data.strip()
            if text:
                self._buf.append(text)
                self._buf.append(" ")

    def extract(self, html: str) -> str:
        self._buf.clear()
        self._skip = False
        self.parser.feed(html)
        text = "".join(self._buf)
        return re.sub(r"\n{3,}", "\n\n", text).strip()


def run_recurlsively(output_dir: Path, start_url: str) -> CaseResult:
    args = [
        str(BINARY),
        "crawl",
        start_url,
        "-o",
        str(output_dir),
        "--max-pages",
        "64",
        "--concurrency",
        "8",
        "--max-depth",
        "3",
        "--report",
        "json",
        "--progress",
        "none",
        "--sitemap",
        "off",
        "--timeout",
        "5s",
        "--delay",
        "0ms",
        "--allow-private-network",
    ]
    start = time.perf_counter()
    proc = subprocess.run(args, capture_output=True, text=True, cwd=REPO_ROOT)
    wall_ms = (time.perf_counter() - start) * 1000
    output_bytes, files = measure_output(output_dir)
    pages_fetched = 0
    try:
        data = json.loads(proc.stdout or "{}")
        pages_fetched = int(data.get("totals", {}).get("pages_written", 0))
    except json.JSONDecodeError:
        pass
    return CaseResult(
        case="recurlsively",
        exit_code=proc.returncode,
        wall_ms=wall_ms,
        output_bytes=output_bytes,
        file_count=files,
        pages_fetched=pages_fetched,
        stdout_tail=tail(proc.stdout),
        stderr=tail(proc.stderr),
        output_path=str(output_dir),
    )


def run_wget(output_dir: Path, start_url: str) -> CaseResult:
    args = [
        "wget",
        "-r",
        "-l",
        "3",
        "-np",
        "-E",
        "-k",
        "-p",
        "-P",
        str(output_dir),
        start_url,
    ]
    start = time.perf_counter()
    proc = subprocess.run(args, capture_output=True, text=True)
    wall_ms = (time.perf_counter() - start) * 1000
    base = output_dir / "localhost:8123"
    target = base if base.exists() else output_dir
    output_bytes, files = measure_output(target)
    return finalize(
        CaseResult(
            case="wget",
            exit_code=proc.returncode,
            wall_ms=wall_ms,
            output_bytes=output_bytes,
            file_count=files,
            stdout_tail=tail(proc.stdout),
            stderr=tail(proc.stderr),
            output_path=str(target),
        )
    )


def run_curl_bs4(output_dir: Path, start_url: str) -> CaseResult:
    return _run_curl_extractor(output_dir, start_url, "curl_bs4", _extract_bs4)


def run_curl_html2text(output_dir: Path, start_url: str) -> CaseResult:
    return _run_curl_extractor(output_dir, start_url, "curl_html2text", _extract_html2text)


def run_curl_readability(output_dir: Path, start_url: str) -> CaseResult:
    return _run_curl_extractor(output_dir, start_url, "curl_readability", _extract_readability)


def run_aria_bs4(output_dir: Path, start_url: str) -> CaseResult:
    return _run_curl_extractor(output_dir, start_url, "aria_bs4", _extract_bs4, fetcher="aria2c")


def _run_curl_extractor(
    output_dir: Path,
    start_url: str,
    name: str,
    extractor: Callable[[str], str],
    fetcher: str = "curl",
) -> CaseResult:
    ensure_empty(output_dir)
    start = time.perf_counter()
    urls = fetch_links(start_url)
    pages = 0
    for url, depth in urls:
        try:
            if fetcher == "aria2c":
                raw = subprocess.run(
                    ["aria2c", "-q", "-o", "/dev/null", url],
                    capture_output=True,
                    text=True,
                    check=False,
                ).stdout
                body_proc = subprocess.run(
                    ["curl", "-sS", "-L", "--max-time", "10", url],
                    capture_output=True,
                    text=True,
                    check=False,
                )
                body = body_proc.stdout
            else:
                proc = subprocess.run(
                    ["curl", "-sS", "-L", "--max-time", "10", url],
                    capture_output=True,
                    text=True,
                    check=False,
                )
                body = proc.stdout
        except OSError:
            continue
        text = extractor(body)
        if not text:
            continue
        pages += 1
        safe_name = re.sub(r"[^a-zA-Z0-9_-]", "_", url.rsplit("/", 1)[-1]) or f"page_{pages}"
        if not safe_name:
            safe_name = f"page_{pages}"
        (output_dir / f"{safe_name}.md").write_text(text, encoding="utf-8")
    wall_ms = (time.perf_counter() - start) * 1000
    output_bytes, files = measure_output(output_dir)
    return finalize(
        CaseResult(
            case=name,
            exit_code=0,
            wall_ms=wall_ms,
            output_bytes=output_bytes,
            file_count=files,
            pages_fetched=pages,
            output_path=str(output_dir),
        )
    )


def _extract_bs4(body: str) -> str:
    soup = BeautifulSoup(body, "html.parser")
    for tag in soup(["script", "style", "nav", "header", "footer"]):
        tag.decompose()
    text = soup.get_text("\n", strip=True)
    return text[: 120 * 1024]


def _extract_html2text(body: str) -> str:
    handler = html2text.HTML2Text()
    handler.ignore_links = True
    handler.ignore_images = True
    handler.ignore_tables = False
    return handler.handle(body)[: 120 * 1024]


def _extract_readability(body: str) -> str:
    cleaner = Cleaner(scripts=True, javascript=True, style=True, comments=True, links=False)
    cleaned = cleaner.clean_html(body)
    doc = Document(cleaned)
    summary = doc.summary()
    if not summary:
        return ""
    text = BeautifulSoup(summary, "html.parser").get_text("\n", strip=True)
    return text[: 120 * 1024]


def write_summary(results: list[CaseResult]) -> Path:
    SUMMARY_PATH.parent.mkdir(parents=True, exist_ok=True)
    fieldnames = [
        "case",
        "exit_code",
        "wall_ms",
        "output_bytes",
        "file_count",
        "pages_fetched",
        "ms_per_page",
        "bytes_per_page",
        "output_path",
        "stderr",
        "stdout_tail",
    ]
    with SUMMARY_PATH.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=fieldnames)
        writer.writeheader()
        for result in results:
            writer.writerow({field: getattr(result, field) for field in fieldnames})
    return SUMMARY_PATH


def wait_for_server(port: int, timeout: float = 10.0) -> None:
    deadline = time.perf_counter() + timeout
    last_error = ""
    while time.perf_counter() < deadline:
        try:
            response = requests.get(f"http://127.0.0.1:{port}/", timeout=1)
            if response.status_code < 500:
                return
        except Exception as e:
            last_error = str(e)
        time.sleep(0.1)
    raise RuntimeError(f"fixture server did not become ready on port {port}: {last_error}")


def main() -> int:
    print("Starting local fixture server...")
    server = subprocess.Popen(
        [PYTHON, str(SERVER_SCRIPT), "--port", "8123"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        wait_for_server(8123)
        base = REPO_ROOT / "benches" / "results" / "compare_out"
        start_url = "http://localhost:8123/docs"
        results: list[CaseResult] = [
            run_recurlsively(base / "recurlsively", start_url),
            run_wget(base / "wget", start_url),
            run_curl_bs4(base / "curl_bs4", start_url),
            run_curl_html2text(base / "curl_html2text", start_url),
            run_curl_readability(base / "curl_readability", start_url),
            run_aria_bs4(base / "aria_bs4", start_url),
        ]
    finally:
        server.terminate()
        try:
            server.wait(timeout=10)
        except subprocess.TimeoutExpired:
            server.kill()

    path = write_summary(results)
    print(f"Wrote summary: {path}")
    for result in results:
        status = "OK" if result.exit_code == 0 else "FAIL"
        print(
            f"[{status}] {result.case}: wall={result.wall_ms:0.1f}ms "
            f"output_bytes={result.output_bytes} files={result.file_count} "
            f"pages_fetched={result.pages_fetched} "
            f"ms_per_page={result.ms_per_page:0.1f} "
            f"bytes_per_page={result.bytes_per_page:0.0f}"
        )
        if result.stdout_tail:
            print(result.stdout_tail)
        if result.stderr.strip():
            print("STDERR:")
            print(result.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
