import hashlib
import json
import subprocess
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from tools import upload_media_image as upload


class MockStand(BaseHTTPRequestHandler):
    received: dict = {}

    def log_message(self, format: str, *args: object) -> None:
        return

    def _json(self, value: dict, status: int = 200) -> None:
        body = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:
        if self.path == "/api/auth/setup":
            self._json({"code": 0, "data": {"required": False}})
            return
        self._json({"code": 1}, 404)

    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        if self.path == "/api/auth/login":
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Set-Cookie", "session=fixture; Path=/")
            payload = json.dumps({"code": 0, "data": {"csrfToken": "fixture-csrf"}}).encode()
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return
        if self.path == "/api/download/file":
            MockStand.received = {
                "content_type": self.headers.get("Content-Type", ""),
                "csrf": self.headers.get("x-csrf-token", ""),
                "origin": self.headers.get("Origin", ""),
                "body": body,
            }
            self._json({"code": 0, "data": {"imageId": 3, "imageName": "kubuntu.img", "size": 16}})
            return
        self._json({"code": 1}, 404)


class UploadMediaImageTests(unittest.TestCase):
    def test_validation_rejects_unsafe_images(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            good = root / "kubuntu.img"
            good.write_bytes(b"disk-image-bytes")
            name, size, digest = upload.validate_image(good)
            self.assertEqual(name, "kubuntu.img")
            self.assertEqual(size, 16)
            self.assertEqual(digest, hashlib.sha256(b"disk-image-bytes").hexdigest())

            for bad in ("../escape.img", ".hidden.img", "image.raw", "image.img.gz"):
                with self.subTest(bad=bad):
                    with self.assertRaises(ValueError):
                        upload.validate_image(root / bad)
            empty = root / "empty.img"
            empty.write_bytes(b"")
            with self.assertRaises(ValueError):
                upload.validate_image(empty)

    def test_multipart_body_wraps_one_file(self) -> None:
        body = upload.multipart_body("kubuntu.img", b"payload", "----boundary")
        self.assertIn(b'name="file"; filename="kubuntu.img"', body)
        self.assertIn(b"\r\n\r\npayload\r\n", body)
        self.assertTrue(body.endswith(b"------boundary--\r\n"))

    def test_upload_logs_in_and_posts_the_file_without_leaking_the_password(self) -> None:
        server = ThreadingHTTPServer(("127.0.0.1", 0), MockStand)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        base = f"http://127.0.0.1:{server.server_address[1]}"
        try:
            with tempfile.TemporaryDirectory() as directory:
                image = Path(directory) / "kubuntu.img"
                image.write_bytes(b"disk-image-bytes")
                output = Path("evidence/private/upload-selftest-test.json")
                try:
                    result = subprocess.run(
                        [
                            sys.executable,
                            "tools/upload_media_image.py",
                            "--base-url",
                            base,
                            "--username",
                            "operator",
                            "--file",
                            str(image),
                            "--output",
                            str(output),
                        ],
                        input=b"fixture password\n",
                        stdout=subprocess.PIPE,
                        stderr=subprocess.PIPE,
                        check=False,
                    )
                    self.assertEqual(result.returncode, 0, result.stderr.decode())
                    receipt = json.loads(output.read_text())
                    self.assertEqual(receipt["probe"], "hardened-blikvm-media-upload")
                    self.assertEqual(receipt["file"]["name"], "kubuntu.img")
                    self.assertEqual(receipt["file"]["size"], 16)
                    self.assertEqual(receipt["imageId"], 3)
                    self.assertIs(receipt["applied"], True)
                    self.assertNotIn("fixture password", output.read_text())

                    received = MockStand.received
                    self.assertEqual(received["csrf"], "fixture-csrf")
                    self.assertEqual(received["origin"], base)
                    self.assertIn("multipart/form-data; boundary=", received["content_type"])
                    self.assertIn(b"disk-image-bytes", received["body"])
                finally:
                    output.unlink(missing_ok=True)
        finally:
            server.shutdown()
            server.server_close()


if __name__ == "__main__":
    unittest.main()
