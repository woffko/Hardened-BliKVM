#!/usr/bin/env python3
"""Check the native firewall HTTPS probe using ephemeral loopback TLS fixtures."""
import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import ssl
import subprocess
import tempfile
import threading


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gate", required=True)
    parser.add_argument("--temp-root")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="firewall-https-", dir=args.temp_root) as temporary:
        root = Path(temporary)
        for name in ("front", "untrusted"):
            subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                            "-subj", "/CN=fixture.local", "-addext", "subjectAltName=DNS:fixture.local",
                            "-keyout", str(root / f"{name}.key"), "-out", str(root / f"{name}.crt")],
                           check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=5)

        class Handler(BaseHTTPRequestHandler):
            status = 200
            body = b'{"code":0,"data":{"status":"ok","backend":"rust","platformId":"bli-h616-pikvm"}}'
            paths: list[str] = []

            def do_GET(self) -> None:
                self.paths.append(self.path)
                self.send_response(self.status)
                if self.status == 302:
                    self.send_header("Location", "/redirected")
                self.send_header("Content-Length", str(len(self.body)))
                self.end_headers()
                self.wfile.write(self.body)

            def log_message(self, format: str, *args: object) -> None:
                pass

        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(root / "front.crt", root / "front.key")
        server.socket = context.wrap_socket(server.socket, server_side=True)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            def probe(host: str, ca: str, expected: str) -> None:
                subprocess.run([args.gate, host, str(root / f"{ca}.crt"), str(server.server_port), expected], check=True, timeout=5)
            probe("fixture.local", "front", "pass")
            probe("wrong.local", "front", "fail")
            probe("fixture.local", "untrusted", "fail")
            Handler.status = 302
            probe("fixture.local", "front", "fail")
            assert "/redirected" not in Handler.paths
            Handler.status = 200
            Handler.body = b'{"code":0,"data":{"status":"failed","backend":"rust","platformId":"bli-h616-pikvm"}}'
            probe("fixture.local", "front", "fail")
            print("HTTPS preflight: trusted certificate, wrong hostname/CA, redirect and unhealthy body passed")
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)


if __name__ == "__main__":
    main()
