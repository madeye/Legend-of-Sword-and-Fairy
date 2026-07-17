#!/usr/bin/env python3
"""Dev server for the web build.

SharedArrayBuffer (which the engine worker needs for input + sleeping)
requires cross-origin isolation, so this wraps http.server with the COOP/COEP
headers. Serves the repository root so both /web/ and /pal/ are reachable.

Usage: python3 web/serve.py [port]   (default 8080; open /web/)
"""
import functools
import http.server
import os
import sys


class Handler(http.server.SimpleHTTPRequestHandler):
    extensions_map = {
        **http.server.SimpleHTTPRequestHandler.extensions_map,
        ".wasm": "application/wasm",
        ".js": "text/javascript",
    }

    def end_headers(self):
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        self.send_header("Cache-Control", "no-store")
        super().end_headers()


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8080
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    handler = functools.partial(Handler, directory=root)
    with http.server.ThreadingHTTPServer(("127.0.0.1", port), handler) as httpd:
        print(f"serving {root} at http://127.0.0.1:{port}/web/")
        httpd.serve_forever()


if __name__ == "__main__":
    main()
