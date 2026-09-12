import json
import io
import socket
import struct
import tarfile
from typing import cast
import unittest
from unittest.mock import patch

from tools import probe_installed_upload as probe


class Stream:
    def __init__(self, reply: bytes = b"") -> None:
        self.reply = bytearray(reply)
        self.sent = bytearray()
        self.shutdown_how: int | None = None

    def sendall(self, data: bytes) -> None:
        self.sent.extend(data)

    def shutdown(self, how: int) -> None:
        self.shutdown_how = how

    def recv(self, size: int) -> bytes:
        part = bytes(self.reply[:min(size, 3)])
        del self.reply[:len(part)]
        return part


def framed(value: object) -> bytes:
    raw = json.dumps(value).encode()
    return struct.pack(">I", len(raw)) + raw


def receipt() -> dict[str, object]:
    return {"type": "staged", "receipt": {
        "slot": {"slotId": probe.SLOT, "version": "0.1.0-dev6", "sequence": 7, "minimumSystemSequence": 1},
        "disposition": "already_present", "manifestSha256": probe.MANIFEST_SHA, "payloadBytes": 8683648}}


class InstalledUploadProbeTests(unittest.TestCase):
    def test_seq9_retry_profile_cannot_publish(self) -> None:
        stream = Stream()
        profile = probe.PROFILES['seq9-retry']
        self.assertFalse(profile.publish)
        self.assertEqual(profile.sequence, 9)
        with self.assertRaises(ValueError):
            probe.exchange(cast(socket.socket, stream), b'archive', 'publish', profile=profile)
        self.assertEqual(stream.sent, b'')

    def test_publication_requires_explicit_profile_and_exact_disposition(self) -> None:
        profile = probe.PROFILES["seq8-publish"]
        response: dict[str, object] = {"type": "staged", "receipt": {
            "slot": {"slotId": profile.slot, "version": "0.1.0-dev6", "sequence": 8, "minimumSystemSequence": 1},
            "disposition": "published", "manifestSha256": profile.manifest_sha, "payloadBytes": 8683648}}
        stream = Stream(framed({"schemaVersion": 1, "requestId": 1, "response": response}))
        self.assertEqual(probe.exchange(cast(socket.socket, stream), b"archive", "publish", profile=profile)["result"], "passed")
        with self.assertRaises(ValueError):
            probe.exchange(cast(socket.socket, Stream()), b"archive", "publish")
        with self.assertRaises(ValueError):
            probe.validate_retry(response, profile)
        cast(dict[str, object], response["receipt"])["disposition"] = "already_present"
        probe.validate_retry(response, profile)
        with self.assertRaises(ValueError):
            probe.validate_retry(response, profile, publish=True)

    def test_tamper_changes_payload_without_changing_archive_structure(self) -> None:
        stream = io.BytesIO()
        with tarfile.open(fileobj=stream, mode="w") as archive:
            member = tarfile.TarInfo("payload/share/web/index.html")
            member.size = 3
            archive.addfile(member, io.BytesIO(b"abc"))
        original = stream.getvalue()
        changed = probe.tamper_archive(original)
        self.assertEqual(len(changed), len(original))
        self.assertEqual(sum(left != right for left, right in zip(original, changed, strict=True)), 1)
        with tarfile.open(fileobj=io.BytesIO(changed)) as archive:
            member = archive.extractfile("payload/share/web/index.html")
            assert member is not None
            self.assertNotEqual(member.read(), b"abc")

    def test_held_body_advances_only_real_prefix_and_requires_explicit_release(self) -> None:
        stream = Stream()
        body = b"abcdef" * 100
        with patch.object(probe.sys, "stdin", type("Input", (), {"buffer": io.BytesIO(b"G")})()), \
             patch.object(probe.sys, "stdout", io.StringIO()), patch.object(probe.time, "monotonic", return_value=0), \
             patch.object(probe.select, "select", side_effect=[([], [], []), ([1], [], [])]):
            remaining = probe.await_release(cast(socket.socket, stream), body)
        self.assertEqual(bytes(stream.sent) + remaining, body)
        self.assertEqual(stream.sent, body[:2])
        with patch.object(probe.sys, "stdout", io.StringIO()), patch.object(probe.time, "monotonic", side_effect=[0, 31]), self.assertRaises(TimeoutError):
            probe.await_release(cast(socket.socket, Stream()), body)

    def test_idle_probe_keeps_write_half_open_until_worker_timeout(self) -> None:
        stream = Stream()
        with patch.object(probe.time, "monotonic", side_effect=[0, 5]):
            probe.exchange(cast(socket.socket, stream), b"archive", "idle")
        self.assertIsNone(stream.shutdown_how)
        self.assertTrue(stream.sent.endswith(b"a"))
        with patch.object(probe.time, "monotonic", side_effect=[0, 0.1]), self.assertRaises(ValueError):
            probe.exchange(cast(socket.socket, Stream()), b"archive", "idle")

    def test_exact_frame_body_eof_and_correlated_retry(self) -> None:
        stream = Stream(framed({"schemaVersion": 1, "requestId": 1, "response": receipt()}))
        self.assertEqual(probe.exchange(cast(socket.socket, stream), b"archive", "retry"), {"mode": "retry", "result": "passed"})
        size = struct.unpack(">I", stream.sent[:4])[0]
        self.assertEqual(json.loads(stream.sent[4:4 + size]),
                         {"schemaVersion": 1, "requestId": 1, "request": {"type": "stage", "archiveBytes": 7}})
        self.assertEqual(stream.sent[4 + size:], b"archive")
        self.assertEqual(stream.shutdown_how, socket.SHUT_WR)

    def test_negative_vectors_and_unobserved_reply(self) -> None:
        for mode, body in (("short", b"archiv"), ("surplus", b"archivex"), ("unauthorized", b"x")):
            stream = Stream()
            self.assertEqual(probe.exchange(cast(socket.socket, stream), b"archive", mode)["result"], "passed")
            size = struct.unpack(">I", stream.sent[:4])[0]
            self.assertEqual(stream.sent[4 + size:], body)
        malformed = Stream()
        probe.exchange(cast(socket.socket, malformed), b"archive", "malformed")
        self.assertEqual(malformed.sent, b"\0\0\0\1{")
        lost = Stream(b"must not read this response")
        self.assertEqual(probe.exchange(cast(socket.socket, lost), b"archive", "lost-reply")["result"], "reply-unobserved")
        self.assertEqual(lost.reply, b"must not read this response")
        invalid = Stream(framed({"schemaVersion": 1, "requestId": 1,
                                "response": {"type": "error", "code": "invalid_archive"}}))
        with patch.object(probe, "tamper_archive", return_value=b"altered"):
            probe.exchange(cast(socket.socket, invalid), b"archive", "invalid-archive")

    def test_receipt_rejects_publication_wrong_identity_and_rounded_counters(self) -> None:
        for key, value in (("disposition", "published"), ("manifestSha256", "a" * 64), ("payloadBytes", 1)):
            changed = receipt()
            cast(dict[str, object], changed["receipt"])[key] = value
            with self.assertRaises(ValueError):
                probe.validate_retry(changed)
        changed = receipt()
        cast(dict[str, object], cast(dict[str, object], changed["receipt"])["slot"])["minimumSystemSequence"] = True
        with self.assertRaises(ValueError):
            probe.validate_retry(changed)

    def test_response_is_bounded_closed_and_strict(self) -> None:
        duplicate = b'{"schemaVersion":1,"requestId":1,"requestId":1,"response":{}}'
        for raw in (b"x", struct.pack(">I", 0), struct.pack(">I", 16385),
                    struct.pack(">I", 3) + b"{}", struct.pack(">I", len(duplicate)) + duplicate,
                    framed({"schemaVersion": 1, "requestId": 2, "response": {}}),
                    framed({"schemaVersion": True, "requestId": 1, "response": {}}),
                    framed({"schemaVersion": 1, "requestId": 1, "response": {}}) + b"x"):
            with self.subTest(raw=raw[:20]), self.assertRaises(ValueError):
                probe.receive(cast(socket.socket, Stream(raw)))
        for size in (0, -1, True, 128 * 1024 * 1024 + 1):
            with self.assertRaises(ValueError):
                probe.frame(size)


if __name__ == "__main__":
    unittest.main()
