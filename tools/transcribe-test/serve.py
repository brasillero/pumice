#!/usr/bin/env python3
"""Loopback-only static page and bounded streaming transcription proxy."""
import argparse
import http.client
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit

MAX_BODY = 26_000_000
CHUNK = 64 * 1024
HOP_HEADERS = {"connection", "keep-alive", "proxy-authenticate", "proxy-authorization",
               "te", "trailer", "transfer-encoding", "upgrade"}
INDEX = Path(__file__).with_name("index.html")


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):
        pass  # Do not log URLs, credentials, bodies, or response content.

    def error(self, status, message):
        body = json.dumps({"error": {"message": message, "type": "proxy_error",
                                     "param": None, "code": "proxy_error"}}).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.close_connection = True
        self.wfile.write(body)

    def do_GET(self):
        if self.path in ("/", "/index.html"):
            body = INDEX.read_bytes()
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(body)
        elif self.path.startswith("/v1/"):
            self.proxy()
        else:
            self.error(404, "Not found.")

    def do_POST(self):
        if self.path.startswith("/v1/"):
            self.proxy()
        else:
            self.error(404, "Not found.")

    def handle_expect_100(self):
        length = self.body_length()
        if length is None:
            return False
        return super().handle_expect_100()

    def body_length(self):
        lengths = self.headers.get_all("Content-Length", [])
        if self.headers.get("Transfer-Encoding"):
            self.error(411, "Use Content-Length; chunked uploads are not supported by this helper.")
            return None
        if len(lengths) > 1 or (lengths and not lengths[0].isascii()) or (lengths and not lengths[0].isdigit()):
            self.error(400, "Invalid Content-Length.")
            return None
        if not lengths and self.command == "POST":
            self.error(411, "Content-Length is required.")
            return None
        length = int(lengths[0]) if lengths and len(lengths[0]) <= 8 else (MAX_BODY + 1 if lengths else 0)
        if length > MAX_BODY:
            self.error(413, "Request body exceeds 26,000,000 bytes.")
            return None
        return length

    def proxy(self):
        length = self.body_length()
        if length is None:
            return
        upstream = http.client.HTTPConnection(self.server.upstream_host,
                                              self.server.upstream_port, timeout=135)
        responded = False
        self.close_connection = True
        self.connection.settimeout(15)
        try:
            excluded = HOP_HEADERS | {"host", "content-length", "expect"}
            excluded |= {part.strip().lower() for part in self.headers.get("Connection", "").split(",")}
            upstream.putrequest(self.command, self.path, skip_accept_encoding=True)
            for key, value in self.headers.items():
                if key.lower() not in excluded:
                    upstream.putheader(key, value)
            upstream.putheader("Content-Length", str(length))
            upstream.endheaders()
            remaining = length
            try:
                while remaining:
                    chunk = self.rfile.read1(min(CHUNK, remaining))
                    if not chunk:
                        raise ConnectionError("Incomplete upload")
                    upstream.send(chunk)
                    remaining -= len(chunk)
            except (BrokenPipeError, ConnectionResetError):
                # An upstream may reject a busy upload before reading its body.
                pass
            response = upstream.getresponse()
            self.send_response_only(response.status, response.reason)
            excluded = HOP_HEADERS | {part.strip().lower() for part in response.getheader("Connection", "").split(",")}
            for key, value in response.getheaders():
                if key.lower() not in excluded:
                    self.send_header(key, value)
            self.send_header("Connection", "close")
            self.end_headers()
            responded = True
            while chunk := response.read(CHUNK):
                self.wfile.write(chunk)
        except (OSError, http.client.HTTPException):
            if not responded:
                self.error(502, "Transcription upstream unavailable or upload interrupted.")
        finally:
            upstream.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=17569)
    parser.add_argument("--upstream", default="http://127.0.0.1:17568")
    args = parser.parse_args()
    url = urlsplit(args.upstream)
    if (url.scheme != "http" or url.hostname != "127.0.0.1" or url.path not in ("", "/")
            or url.username or url.password or url.query or url.fragment):
        parser.error("--upstream must be http://127.0.0.1:PORT with no path or credentials")
    with ThreadingHTTPServer(("127.0.0.1", args.port), Handler) as server:
        server.upstream_host = url.hostname
        server.upstream_port = url.port or 80
        print(f"Open http://127.0.0.1:{server.server_port} (upstream {args.upstream})", flush=True)
        try:
            server.serve_forever()
        except KeyboardInterrupt:
            pass


if __name__ == "__main__":
    main()
