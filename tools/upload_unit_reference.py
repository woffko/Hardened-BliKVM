"""Keep one upload worker's terminal properties available without stopping it.

RefUnit pins only the unit's in-memory record, not the worker process. Closing
this private bus connection releases its references. API: systemd v249
man/org.freedesktop.systemd1.xml, RefUnit/UnrefUnit.
"""

from __future__ import annotations

import ctypes
import re


class UploadUnitReference:
    def __init__(self, unit: str) -> None:
        if not re.fullmatch(r"hardened-blikvm-update-upload@[A-Za-z0-9_.:\\-]+\.service", unit):
            raise ValueError("invalid upload unit reference")
        library = self.library = ctypes.CDLL("libsystemd.so.0")
        pointer = ctypes.c_void_p
        library.sd_bus_open_system.argtypes = [ctypes.POINTER(pointer)]
        library.sd_bus_open_system.restype = ctypes.c_int
        library.sd_bus_set_method_call_timeout.argtypes = [pointer, ctypes.c_uint64]
        library.sd_bus_set_method_call_timeout.restype = ctypes.c_int
        library.sd_bus_call_method.argtypes = [pointer, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_char_p,
            ctypes.c_char_p, pointer, ctypes.POINTER(pointer), ctypes.c_char_p]
        library.sd_bus_call_method.restype = ctypes.c_int
        library.sd_bus_message_get_signature.argtypes = [pointer, ctypes.c_int]
        library.sd_bus_message_get_signature.restype = ctypes.c_char_p
        library.sd_bus_message_unref.argtypes = [pointer]
        library.sd_bus_message_unref.restype = pointer
        library.sd_bus_close_unref.argtypes = [pointer]
        library.sd_bus_close_unref.restype = pointer
        self.bus = pointer()
        reply = pointer()
        try:
            if library.sd_bus_open_system(ctypes.byref(self.bus)) < 0 or not self.bus.value:
                raise RuntimeError("unit reference bus unavailable")
            if library.sd_bus_set_method_call_timeout(self.bus, 5_000_000) < 0:
                raise RuntimeError("unit reference timeout unavailable")
            if library.sd_bus_call_method(self.bus, b"org.freedesktop.systemd1", b"/org/freedesktop/systemd1",
                    b"org.freedesktop.systemd1.Manager", b"RefUnit", None, ctypes.byref(reply), b"s",
                    ctypes.c_char_p(unit.encode("ascii"))) < 0:
                raise RuntimeError("unit reference failed")
            if not reply.value or library.sd_bus_message_get_signature(reply, 1) != b"":
                raise RuntimeError("invalid unit reference reply")
        except BaseException:
            self.close()
            raise
        finally:
            if reply.value:
                library.sd_bus_message_unref(reply)

    def close(self) -> None:
        if self.bus.value:
            self.library.sd_bus_close_unref(self.bus)
            self.bus.value = None

    def __enter__(self) -> UploadUnitReference:
        return self

    def __exit__(self, *_args: object) -> None:
        self.close()
