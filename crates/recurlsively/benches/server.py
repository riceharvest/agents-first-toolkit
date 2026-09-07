"""Local deterministic HTTP fixture server for recurlsively benchmarks."""

from __future__ import annotations

import argparse
import os
import random
import threading
from http.server import HTTPServer, SimpleHTTPRequestHandler

FIXTURE_DIR = os.path.join(os.path.dirname(__file__), "fixtures", "static")
TEMPLATES = {
    "index": "<html><body><h1>Bench docs</h1><links/></body></html>",
    "install": "<html><body><h1>Install</h1><p>Install steps, then related docs.</p><links/></body></html>",
    "guide": "<html><body><h1>Guide</h1><p>Walkthrough links below.</p><links/></body></html>",
    "reference": "<html><body><h1>Reference</h1><p>Canonical terms and links.</p><links/></body></html>",
    "faq": "<html><body><h1>FAQ</h1><p>Common issue links.</p><links/></body></html>",
    "changelog": "<html><body><h1>Changelog</h1><p>Release notes and links.</p><links/></body></html>",
}
DOC_KEYS = list(TEMPLATES.keys())[1:]


def render_template(template: str, seed: int = 0xBEAF) -> str:
    rng = random.Random(seed)
    out: list[str] = []
    links: list[str] = []
    for block in template.split("<links/>"):
        out.append(block)
        count = rng.randint(1, 3)
        for _ in range(count):
            target = rng.choice(DOC_KEYS)
            links.append(target)
    if links:
        out.append("<ul>")
        for link in links:
            out.append(f'<li><a href="/docs/{link}">{link}</a></li>')
        out.append("</ul>")
    return "".join(out)


class FixtureHandler(SimpleHTTPRequestHandler):
    def do_GET(self) -> None:  # noqa: N802
        path = self.path.split("?")[0]
        if path in ("/", "/docs", "/docs/"):
            body = render_template(TEMPLATES["index"])
            return self._respond(body)
        if path.startswith("/docs/"):
            key = path[len("/docs/"):].strip("/")
            if not key:
                body = render_template(TEMPLATES["index"])
                return self._respond(body)
            template = TEMPLATES.get(key)
            if template:
                body = render_template(template, seed=(hash(key) & 0xFFFFFFFF) or 0xBEAF)
                return self._respond(body)
        body = "<html><body><h1>Not found</h1></body></html>"
        self._respond(body, status=404)

    def _respond(self, body: str, status: int = 200) -> None:
        body_bytes = body.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body_bytes)))
        self.end_headers()
        self.wfile.write(body_bytes)

    def log_message(self, format, *args):
        return


def serve(port: int):
    os.chdir(FIXTURE_DIR)
    server = HTTPServer(("127.0.0.1", port), FixtureHandler)
    server.serve_forever()


def main():
    parser = argparse.ArgumentParser(description="Run benchmark fixture server")
    parser.add_argument("--port", type=int, default=int(os.environ.get("RECURLSIVELY_BENCH_PORT", "8123")))
    args = parser.parse_args()
    serve(args.port)


if __name__ == "__main__":
    main()
