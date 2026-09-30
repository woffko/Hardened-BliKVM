#!/usr/bin/env python3
"""Exercise packaged nginx upload limits and early upstream forwarding locally."""
import argparse
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import pwd
import socket
import ssl
import subprocess
import tempfile
import threading
import time


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--nginx", required=True)
    parser.add_argument("--temp-root")
    args = parser.parse_args()
    source = Path(__file__).resolve().parent.parent / "packaging/nginx/hardened-blikvm.conf"
    observed: list[tuple[str, str | None, str | None]] = []

    class Backend(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            observed.append((self.path, self.headers.get("Host"), self.headers.get("X-Forwarded-Proto")))
            self.send_response(200)
            self.send_header("Content-Length", "2")
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(b"ok")
            self.close_connection = True

        def log_message(self, format: str, *args: object) -> None:
            pass

    backend = ThreadingHTTPServer(("127.0.0.1", 0), Backend)
    thread = threading.Thread(target=backend.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="nginx-upload-", dir=args.temp_root) as temporary:
            root = Path(temporary)
            cert, key = root / "tls.crt", root / "tls.key"
            subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                            "-subj", "/CN=fixture.local", "-addext", "subjectAltName=DNS:fixture.local",
                            "-keyout", str(key), "-out", str(cert)], check=True,
                           stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=5)
            port, http_port = free_port(), free_port()
            config = source.read_text()
            config = config.replace("user www-data;", f"user {pwd.getpwuid(os.getuid()).pw_name};")
            config = config.replace("/run/hardened-blikvm-nginx", str(root))
            config = config.replace("/etc/hardened-blikvm/tls.crt", str(cert)).replace("/etc/hardened-blikvm/tls.key", str(key))
            config = config.replace("listen 443 ssl default_server;", f"listen 127.0.0.1:{port} ssl default_server;")
            config = config.replace("listen [::]:443 ssl default_server;", "")
            config = config.replace("listen 80 default_server;", f"listen 127.0.0.1:{http_port} default_server;")
            config = config.replace("listen [::]:80 default_server;", "")
            config = config.replace("127.0.0.1:18080", f"127.0.0.1:{backend.server_port}")
            path = root / "nginx.conf"
            path.write_text(config)
            subprocess.run([args.nginx, "-t", "-p", str(root), "-c", str(path)], check=True, capture_output=True, timeout=3)
            process = subprocess.Popen([args.nginx, "-p", str(root), "-c", str(path), "-g", "daemon off;"], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            try:
                deadline = time.monotonic() + 3
                while True:
                    try:
                        with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                            break
                    except OSError:
                        if time.monotonic() >= deadline:
                            raise RuntimeError("nginx did not start")
                        time.sleep(0.02)
                context = ssl.create_default_context(cafile=str(cert))

                def request(route: str, length: int) -> int:
                    with socket.create_connection(("127.0.0.1", port), timeout=3) as tcp:
                        with context.wrap_socket(tcp, server_hostname="fixture.local") as tls:
                            # Deliberately do not send the declared large body: the
                            # upstream must see headers before full buffering.
                            tls.sendall(f"POST {route} HTTP/1.1\r\nHost: fixture.local:{port}\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n".encode() + b"x")
                            return int(tls.recv(4096).split(b"\r\n", 1)[0].split()[1])

                mib = 1024 * 1024
                assert request("/api/application-update/upload", 17 * mib) == 200
                assert request("/api/application-update/upload", 128 * mib + 1) == 413
                assert request("/api/download/file", 129 * mib) == 200
                assert request("/api/download/file", 5 * 1024 * mib) == 200
                assert request("/api/download/file", 65537 * mib + 1) == 413
                assert request("/api/other", 17 * mib) == 413
                assert request("/api/application-update/upload/extra", 17 * mib) == 413
                assert observed == [("/api/application-update/upload", f"fixture.local:{port}", "https"),
                                    ("/api/download/file", f"fixture.local:{port}", "https"),
                                    ("/api/download/file", f"fixture.local:{port}", "https")]
                print("nginx upload contract: endpoint limits, early forwarding and unchanged default limit passed")
            finally:
                process.terminate()
                try:
                    process.communicate(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.communicate(timeout=3)
    finally:
        backend.shutdown()
        backend.server_close()
        thread.join(timeout=2)


if __name__ == "__main__":
    main()
