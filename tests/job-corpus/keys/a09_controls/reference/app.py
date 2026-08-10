"""Reference answer to A-9. Not given to the agent.

Its only job is to prove the probe suite is winnable. Standard library only.
"""

import http.server
import json
import os
import random
import sqlite3
import string
import sys
import urllib.parse

HERE = os.path.dirname(os.path.abspath(__file__))
DB_PATH = os.path.join(HERE, "links.db")
ALPHABET = string.ascii_letters + string.digits


def connect():
    conn = sqlite3.connect(DB_PATH)
    conn.execute(
        "CREATE TABLE IF NOT EXISTS links ("
        " code TEXT PRIMARY KEY,"
        " url TEXT NOT NULL UNIQUE,"
        " hits INTEGER NOT NULL DEFAULT 0)"
    )
    conn.commit()
    return conn


def new_code(conn):
    while True:
        code = "".join(random.choice(ALPHABET) for _ in range(7))
        row = conn.execute("SELECT 1 FROM links WHERE code = ?", (code,)).fetchone()
        if row is None:
            return code


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        sys.stderr.write("%s\n" % (fmt % args))

    def _send(self, status, payload=None, headers=None):
        body = b"" if payload is None else json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        for name, value in (headers or {}).items():
            self.send_header(name, value)
        self.end_headers()
        if body:
            self.wfile.write(body)

    def do_GET(self):
        path = urllib.parse.urlparse(self.path).path
        if path == "/healthz":
            return self._send(200, {"status": "ok"})
        conn = connect()
        try:
            if path.startswith("/api/links/"):
                code = path[len("/api/links/"):]
                row = conn.execute(
                    "SELECT url, hits FROM links WHERE code = ?", (code,)
                ).fetchone()
                if row is None:
                    return self._send(404, {"error": "no such code"})
                return self._send(200, {"url": row[0], "hits": row[1]})
            code = path.lstrip("/")
            if not code:
                return self._send(404, {"error": "not found"})
            row = conn.execute("SELECT url FROM links WHERE code = ?", (code,)).fetchone()
            if row is None:
                return self._send(404, {"error": "no such code"})
            conn.execute("UPDATE links SET hits = hits + 1 WHERE code = ?", (code,))
            conn.commit()
            return self._send(302, None, {"Location": row[0]})
        finally:
            conn.close()

    def do_POST(self):
        path = urllib.parse.urlparse(self.path).path
        if path != "/api/links":
            return self._send(404, {"error": "not found"})
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b""
        try:
            payload = json.loads(raw.decode("utf-8"))
            if not isinstance(payload, dict):
                raise ValueError("expected an object")
        except (ValueError, UnicodeDecodeError):
            return self._send(400, {"error": "body must be a JSON object"})
        url = payload.get("url")
        if not isinstance(url, str) or not url:
            return self._send(400, {"error": "url is required"})
        parsed = urllib.parse.urlparse(url)
        if parsed.scheme not in ("http", "https") or not parsed.netloc:
            return self._send(400, {"error": "url must be http or https"})
        conn = connect()
        try:
            row = conn.execute("SELECT code FROM links WHERE url = ?", (url,)).fetchone()
            if row is not None:
                return self._send(200, {"code": row[0], "short_url": self._short(row[0])})
            code = new_code(conn)
            conn.execute("INSERT INTO links (code, url, hits) VALUES (?, ?, 0)", (code, url))
            conn.commit()
            return self._send(201, {"code": code, "short_url": self._short(code)})
        finally:
            conn.close()

    def _short(self, code):
        host = self.headers.get("Host") or "127.0.0.1"
        return "http://%s/%s" % (host, code)


def main():
    port = int(os.environ.get("PORT", "8317"))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler)
    server.serve_forever()


if __name__ == "__main__":
    main()
