import ctypes
from typing import Callable, cast
import unittest
from unittest.mock import patch

from tools import upload_unit_reference as reference


class Function:
    def __init__(self, callback: Callable[..., object]) -> None:
        self.callback = callback

    def __call__(self, *args: object) -> object:
        return self.callback(*args)


class Library:
    def __init__(self, failure: bool) -> None:
        self.calls: list[tuple[object, ...]] = []
        self.closed = 0
        self.unreferenced = 0
        def opened(bus: object) -> int:
            ctypes.cast(cast(ctypes.c_void_p, bus), ctypes.POINTER(ctypes.c_void_p))[0] = ctypes.c_void_p(1)
            return 0
        def call(*args: object) -> int:
            self.calls.append(args)
            ctypes.cast(cast(ctypes.c_void_p, args[6]), ctypes.POINTER(ctypes.c_void_p))[0] = ctypes.c_void_p(2)
            return -1 if failure else 0
        def close(_bus: object) -> None:
            self.closed += 1
        def unref(_message: object) -> None:
            self.unreferenced += 1
        self.sd_bus_open_system = Function(opened)
        self.sd_bus_set_method_call_timeout = Function(lambda *_args: 0)
        self.sd_bus_call_method = Function(call)
        self.sd_bus_message_get_signature = Function(lambda *_args: b"")
        self.sd_bus_message_unref = Function(unref)
        self.sd_bus_close_unref = Function(close)


class UploadUnitReferenceTests(unittest.TestCase):
    def test_reference_pins_only_one_unit_and_closes_on_success_or_error(self) -> None:
        for failure in (False, True):
            library = Library(failure)
            with patch.object(reference.ctypes, "CDLL", return_value=library):
                if failure:
                    with self.assertRaises(RuntimeError):
                        reference.UploadUnitReference("hardened-blikvm-update-upload@1-123.service")
                else:
                    with reference.UploadUnitReference("hardened-blikvm-update-upload@1-123.service") as held:
                        self.assertEqual(library.closed, 0)
                    held.close()
            self.assertEqual(library.closed, 1)
            self.assertEqual(library.unreferenced, 1)
            self.assertEqual(len(library.calls), 1)
            call = library.calls[0]
            self.assertEqual(call[4], b"RefUnit")
            self.assertEqual(call[7], b"s")
            self.assertEqual(getattr(call[8], "value"), b"hardened-blikvm-update-upload@1-123.service")

    def test_arbitrary_unit_rejected_before_bus_access(self) -> None:
        with patch.object(reference.ctypes, "CDLL") as library:
            for unit in ("sshd.service", "hardened-blikvm-update-upload@.service", "hardened-blikvm-update-upload@a.service\nother"):
                with self.assertRaises(ValueError):
                    reference.UploadUnitReference(unit)
            library.assert_not_called()


if __name__ == "__main__":
    unittest.main()
