#!/usr/bin/env python3
"""Upload one image to a hardened stand over the authenticated API.

The GUI keeps its upload control in the download menu, which is gated by the
remote-download capability and therefore hidden today, so this tool is the
operator path for the same endpoint: log in, then `POST /api/download/file` as
multipart with a single bounded file. It writes a private receipt naming the
file, its size, its SHA-256 and the catalog ID the server returned.

The password is read from stdin exactly once and never appears in the arguments,
the environment or the receipt.
"""

from __future__ import annotations

import argparse
import hashlib
import http.cookiejar
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
import uuid
from pathlib import Path

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype

MAX_UPLOAD_BYTES = 256 * 1024 * 1024
MAX_NAME_BYTES = 64
UPLOAD_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")
ALLOWED_EXTENSIONS = ("img", "iso")


def validate_image(path: Path) -> tuple[str, int, str]:
    name = path.name
    if (
        len(name.encode()) > MAX_NAME_BYTES
        or not UPLOAD_NAME.fullmatch(name)
        or ".." in name
    ):
        raise ValueError("image name is invalid")
    if name.rsplit(".", 1)[-1].lower() not in ALLOWED_EXTENSIONS:
        raise ValueError("only .img and .iso images are accepted")
    try:
        size = path.stat().st_size
    except OSError as error:
        raise ValueError("image file is not readable") from error
    if size == 0 or size > MAX_UPLOAD_BYTES:
        raise ValueError("image size is out of range")
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return name, size, digest.hexdigest()


def read_password() -> str:
    buffer = sys.stdin.buffer.read(4097)
    if len(buffer) > 4096:
        raise ValueError("password input exceeded the limit")
    return buffer.decode("utf-8", errors="strict").rstrip("\r\n")


def multipart_body(name: str, content: bytes, boundary: str) -> bytes:
    head = (
        f"--{boundary}\r\n"
        f'Content-Disposition: form-data; name="file"; filename="{name}"\r\n'
        "Content-Type: application/octet-stream\r\n\r\n"
    ).encode()
    tail = f"\r\n--{boundary}--\r\n".encode()
    return head + content + tail


def build_opener() -> urllib.request.OpenerDirector:
    jar = http.cookiejar.CookieJar()
    return urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))


def post_json(
    opener: urllib.request.OpenerDirector, base: str, route: str, payload: dict
) -> dict:
    request = urllib.request.Request(
        urllib.parse.urljoin(base, route),
        data=json.dumps(payload).encode(),
        headers={
            "Content-Type": "application/json",
            "Origin": base.rstrip("/"),
        },
        method="POST",
    )
    with opener.open(request, timeout=30) as response:
        return json.loads(response.read().decode())


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--base-url", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--file", required=True)
    _ = parser.add_argument("--output", required=True)
    return parser.parse_args()


def execute(args: argparse.Namespace) -> int:
    output = prototype.validate_output_path(args.output)
    base = args.base_url.rstrip("/")
    parsed = urllib.parse.urlparse(base)
    if parsed.scheme not in ("http", "https") or not parsed.hostname:
        raise ValueError("base URL must be an absolute http(s) URL")
    if parsed.scheme == "http" and parsed.hostname not in ("127.0.0.1", "localhost"):
        raise ValueError("a remote stand must be reached over https")

    image = Path(args.file)
    if not image.is_file() or image.is_symlink():
        raise ValueError("image file is invalid")
    name, size, digest = validate_image(image)
    content = image.read_bytes()
    password = read_password()
    if len(password) < 8:
        raise ValueError("password input is invalid")

    receipt: dict[str, object] = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-media-upload",
        "target": {"host": parsed.hostname or "", "scheme": parsed.scheme},
        "file": {"name": name, "size": size, "sha256": digest},
        "policy": {
            "credentialsRead": False,
            "mutationsPerformed": True,
            "receiptContainsSecrets": False,
        },
    }
    opener = build_opener()
    setup = json.loads(
        opener.open(urllib.parse.urljoin(base, "/api/auth/setup"), timeout=30)
        .read()
        .decode()
    )
    if setup.get("data", {}).get("required"):
        raise ValueError("the stand still requires first-account setup")

    login = post_json(opener, base, "/api/auth/login", {"username": args.username, "password": password})
    password = ""
    if login.get("code") != 0:
        raise ValueError("login failed")
    csrf = login.get("data", {}).get("csrfToken")
    if not csrf:
        raise ValueError("login returned no CSRF token")

    boundary = f"----blikvm{uuid.uuid4().hex}"
    body = multipart_body(name, content, boundary)
    request = urllib.request.Request(
        urllib.parse.urljoin(base, "/api/download/file"),
        data=body,
        headers={
            "Content-Type": f"multipart/form-data; boundary={boundary}",
            "Content-Length": str(len(body)),
            "Origin": base,
            "x-csrf-token": csrf,
        },
        method="POST",
    )
    try:
        with opener.open(request, timeout=300) as response:
            uploaded = json.loads(response.read().decode())
    except urllib.error.HTTPError as error:
        receipt["status"] = error.code
        receipt["rejection"] = "http_error"
        write_json_0600_atomic(output, receipt)
        raise ValueError(f"upload failed with HTTP {error.code}") from error

    data = uploaded.get("data", {})
    receipt["status"] = 200
    receipt["imageId"] = data.get("imageId")
    receipt["applied"] = uploaded.get("code") == 0
    write_json_0600_atomic(output, receipt)
    print(
        f"media_upload={output} file={name} size={size} applied={receipt['applied']}"
    )
    return 0 if receipt["applied"] else 2


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError) as error:
        print(f"media_upload: {type(error).__name__}: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
