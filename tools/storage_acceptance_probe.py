#!/usr/bin/env python3
"""Run the storage gate against a hardened stand and write a private receipt.

The gate is the E1 acceptance path over the reference storage API: list the
images, read the mounted image and the cdrom flag, mount the first image,
re-enumerate the gadget, eject it, and attach it again. With `--upload-image`
it additionally uploads a local image and runs the whole cycle on it (upload,
mount, eject, delete) before re-attaching the reviewed image.

Every mutation is spaced like the GUI spaces it, because the media engine
enforces a per-operation cooldown. The password is read from stdin exactly once
and never appears in the arguments, the environment or the receipt.
"""

from __future__ import annotations

import argparse
import json
import secrets
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

sys.dont_write_bytecode = True

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import stage_device_prototype as prototype
from tools import upload_media_image as media

COOLDOWN_SECONDS = 2.5
MAX_STEPS = 64


def call(
    opener: urllib.request.OpenerDirector,
    base: str,
    method: str,
    route: str,
    payload: dict | None = None,
    csrf: str | None = None,
) -> tuple[int, dict]:
    headers = {"Origin": base}
    data = None
    if payload is not None:
        headers["Content-Type"] = "application/json"
        data = json.dumps(payload).encode()
    if csrf:
        headers["x-csrf-token"] = csrf
    request = urllib.request.Request(
        base + route, data=data, headers=headers, method=method
    )
    try:
        with opener.open(request, timeout=60) as response:
            return response.status, json.loads(response.read().decode())
    except urllib.error.HTTPError as error:
        try:
            return error.code, json.loads(error.read().decode())
        except Exception:
            return error.code, {}
    except urllib.error.URLError:
        return 0, {}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--base-url", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--output", required=True)
    _ = parser.add_argument("--ca-file", default=None)
    _ = parser.add_argument(
        "--tls-server-name",
        default=None,
        help="certificate name to verify when connecting by address",
    )
    _ = parser.add_argument(
        "--upload-image",
        default=None,
        help="local image to upload and cycle; without it the gate only exercises the reviewed image",
    )
    _ = parser.add_argument(
        "--keep-upload",
        action="store_true",
        help="do not delete the uploaded image at the end",
    )
    return parser.parse_args()


def execute(args: argparse.Namespace) -> int:
    output = prototype.validate_output_path(args.output)
    base = args.base_url.rstrip("/")
    parsed = urllib.parse.urlparse(base)
    if parsed.scheme != "https" or not parsed.hostname:
        raise ValueError("the stand must be reached over https")

    upload_path: Path | None = None
    upload_name = ""
    upload_size = 0
    upload_sha = ""
    upload_content = b""
    if args.upload_image:
        upload_path = Path(args.upload_image)
        upload_name, upload_size, upload_sha = media.validate_image(upload_path)
        upload_content = upload_path.read_bytes()

    password = media.read_password()
    if len(password) < 8:
        raise ValueError("password input is invalid")

    steps: list[dict[str, object]] = []
    opener = media.build_opener(args.ca_file, args.tls_server_name)
    body = media.post_json(
        opener, base, "/api/auth/login", {"username": args.username, "password": password}
    )
    csrf = (body.get("data") or {}).get("csrfToken")
    steps.append({"step": "login", "status": 200, "code": body.get("code")})
    if body.get("code") != 0 or not csrf:
        raise ValueError("login was rejected")

    def observe(label: str) -> tuple[list[str], str]:
        status, body = call(opener, base, "GET", "/api/storage/image")
        images = list((body.get("data") or {}).get("files") or [])
        status_mounted, body_mounted = call(
            opener, base, "GET", "/api/storage/image/mounted"
        )
        mounted = (body_mounted.get("data") or {}).get("file") or ""
        status_cdrom, body_cdrom = call(opener, base, "GET", "/api/storage/cdrom")
        cdrom = (body_cdrom.get("data") or {}).get("cdrom")
        steps.append(
            {
                "step": label,
                "status": status,
                "images": images,
                "mounted": mounted,
                "cdrom": cdrom,
                "cdromStatus": status_cdrom,
            }
        )
        if status != 200 or status_mounted != 200 or status_cdrom != 200:
            raise ValueError(f"observation {label} failed")
        return images, mounted

    def mutate(label: str, route: str, payload: dict | None) -> bool:
        status, body = call(opener, base, "POST", route, payload, csrf)
        data = body.get("data") or {}
        steps.append(
            {
                "step": label,
                "status": status,
                "applied": data.get("applied"),
                "rejection": data.get("rejection"),
            }
        )
        time.sleep(COOLDOWN_SECONDS)
        return bool(data.get("applied"))

    def mount(label: str, name: str) -> bool:
        return mutate(label, "/api/storage/image/mount", {"file": name, "cdrom": False})

    images, _ = observe("initial")
    if not images:
        raise ValueError("the stand exposes no images to mount")
    reviewed = upload_name if args.upload_image else images[0]

    if args.upload_image:
        boundary = f"hardened-{secrets.token_hex(8)}"
        payload = media.multipart_body(upload_name, upload_content, boundary)
        request = urllib.request.Request(
            base + "/api/download/file",
            data=payload,
            headers={
                "Content-Type": f"multipart/form-data; boundary={boundary}",
                "Origin": base,
                "x-csrf-token": csrf,
            },
            method="POST",
        )
        try:
            with opener.open(request, timeout=300) as response:
                upload_status = response.status
                upload_body = json.loads(response.read().decode())
        except urllib.error.HTTPError as error:
            upload_status = error.code
            upload_body = {}
        steps.append(
            {
                "step": "upload",
                "status": upload_status,
                "applied": bool((upload_body.get("data") or {}).get("imageId")),
            }
        )
        if upload_status != 200:
            raise ValueError(f"upload failed with HTTP {upload_status}")
        observe("after upload")

    if not mount("mount", reviewed):
        raise ValueError("mount was rejected")
    images, mounted = observe("mounted")
    if mounted != reviewed:
        raise ValueError(f"mounted image mismatch: {mounted!r} != {reviewed!r}")

    mutate("usb reconnect", "/api/storage/usb/reconnect", None)

    if not mount("eject", ""):
        raise ValueError("eject was rejected")
    _, mounted = observe("ejected")
    if mounted:
        raise ValueError("eject left the image mounted")

    if args.upload_image and not args.keep_upload:
        mutate(
            "delete",
            "/api/storage/image/delete",
            {"file": upload_name},
        )
        images, _ = observe("after delete")
        if upload_name in images:
            raise ValueError("the uploaded image is still listed")

    if not mount("re-attach", images[0]):
        raise ValueError("re-attach was rejected")
    _, mounted = observe("final")
    if mounted != images[0]:
        raise ValueError("the final attach did not hold")

    result = "passed" if len(steps) <= MAX_STEPS else "inspect"
    receipt: dict[str, object] = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-storage-acceptance",
        "result": result,
        "target": {"scheme": parsed.scheme, "host": parsed.hostname},
        "reviewedImage": images[0],
        "steps": steps,
        "policy": {
            "credentialsRead": False,
            "mutationsPerformed": True,
            "receiptContainsSecrets": False,
        },
    }
    if args.upload_image:
        receipt["uploadedImage"] = {
            "name": upload_name,
            "size": upload_size,
            "sha256": upload_sha,
        }
    prototype.write_json_0600_atomic(output, receipt)
    print(f"storage_acceptance={output} result={result} steps={len(steps)}")
    return 0


def main() -> int:
    args = parse_args()
    try:
        return execute(args)
    except (ValueError, urllib.error.URLError) as error:
        print(f"storage_acceptance: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
