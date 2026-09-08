import base64
import contextlib
import copy
import io
import json
import unittest
from unittest.mock import patch

from tools import secret_pair_stdin as pair


def envelope() -> dict[str, object]:
    return {"schemaVersion": 1, "encoding": "base64", "secrets": {
        "ssh": base64.b64encode(b"ssh-synthetic\x00\xff\n").decode(),
        "gui": base64.b64encode(b"gui-synthetic\n").decode(),
    }}


class PairTests(unittest.TestCase):
    def test_exact_bytes_wiped_after_success_and_consumer_failure(self) -> None:
        for fail in (False, True):
            retained: dict[str, bytearray] = {}
            try:
                with pair.secret_pair_stdin(io.BytesIO(json.dumps(envelope()).encode())) as values:
                    self.assertEqual(values["ssh"], b"ssh-synthetic\x00\xff\n")
                    self.assertEqual(values["gui"], b"gui-synthetic\n")
                    retained.update(values)
                    values.clear()
                    if fail:
                        raise RuntimeError("consumer failed")
            except RuntimeError as error:
                self.assertTrue(fail)
                self.assertEqual(str(error), "consumer failed")
            self.assertTrue(retained)
            self.assertTrue(all(bytes(value) == b"\0" * len(value) for value in retained.values()))

    def test_malformed_oversize_duplicate_schema_roles_and_base64_rejected(self) -> None:
        invalid: list[bytes] = [b"", b"x" * (pair.MAX_ENVELOPE_BYTES + 1), b"[]", b"{}",
                               b'{"schemaVersion":1,"schemaVersion":1}', b"\xff",
                               b'[' * 2000 + b']' * 2000]
        replacements: dict[str, object] = {"schemaVersion": True, "encoding": "raw", "secrets": {"ssh": "YQ=="}}
        for name, value in replacements.items():
            changed = envelope()
            changed[name] = value
            invalid.append(json.dumps(changed).encode())
        for secret in ("", "!!!!", "YQ===", "YR==", "YQ==\n", "é", 5):
            changed = envelope()
            changed["secrets"] = {"ssh": secret, "gui": "Yg=="}
            invalid.append(json.dumps(changed).encode())
        for raw in invalid:
            with self.subTest(length=len(raw)), self.assertRaisesRegex(ValueError, "^invalid secret stdin pair$"):
                with pair.secret_pair_stdin(io.BytesIO(raw)):
                    self.fail("invalid envelope yielded")

    def test_partial_decode_is_wiped_and_parser_does_not_emit_contents(self) -> None:
        changed = copy.deepcopy(envelope())
        changed["secrets"] = {"gui": "YQ==", "ssh": "!!!"}
        stdout, stderr = io.StringIO(), io.StringIO()
        retained: dict[str, bytearray] = {}
        original = pair.clear_secret_pair
        def wipe(values: dict[str, bytearray]) -> None:
            retained.update(values)
            original(values)
        with patch.object(pair, "clear_secret_pair", side_effect=wipe), \
                contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            with self.assertRaises(ValueError):
                with pair.secret_pair_stdin(io.BytesIO(json.dumps(changed).encode())):
                    self.fail("partial envelope yielded")
        self.assertEqual(retained, {"gui": bytearray(b"\0")})
        self.assertEqual(stdout.getvalue() + stderr.getvalue(), "")


if __name__ == "__main__":
    unittest.main()
