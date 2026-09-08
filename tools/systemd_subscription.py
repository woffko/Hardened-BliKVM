"""Hold one systemd Manager.Subscribe connection without broad signal matches.

The separate exact-match monitor receives the selected job event. No unit
operation, environment query, credentials query or signal dump is performed.
C ABI signatures follow systemd v249 src/systemd/sd-bus.h.
Run this native lease inside an externally time-bounded worker: the method
timeout alone is not an independent wall-clock bound on connection setup.
"""

from __future__ import annotations

import ctypes
import select
import sys

from tools.status_restore_event import SYSTEM_BUS_ADDRESS


def load_library() -> ctypes.CDLL:
    library = ctypes.CDLL("libsystemd.so.0")
    pointer = ctypes.c_void_p
    library.sd_bus_new.argtypes = [ctypes.POINTER(pointer)]
    library.sd_bus_new.restype = ctypes.c_int
    library.sd_bus_set_address.argtypes = [pointer, ctypes.c_char_p]
    library.sd_bus_set_address.restype = ctypes.c_int
    library.sd_bus_set_bus_client.argtypes = [pointer, ctypes.c_int]
    library.sd_bus_set_bus_client.restype = ctypes.c_int
    library.sd_bus_set_method_call_timeout.argtypes = [pointer, ctypes.c_uint64]
    library.sd_bus_set_method_call_timeout.restype = ctypes.c_int
    library.sd_bus_start.argtypes = [pointer]
    library.sd_bus_start.restype = ctypes.c_int
    library.sd_bus_process.argtypes = [pointer, ctypes.POINTER(pointer)]
    library.sd_bus_process.restype = ctypes.c_int
    library.sd_bus_is_open.argtypes = [pointer]
    library.sd_bus_is_open.restype = ctypes.c_int
    # Subscribe has no input arguments: only the fixed part of this variadic
    # API is used, with an empty signature and no varargs.
    library.sd_bus_call_method.argtypes = [pointer, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_char_p,
                                          ctypes.c_char_p, pointer, ctypes.POINTER(pointer), ctypes.c_char_p]
    library.sd_bus_call_method.restype = ctypes.c_int
    library.sd_bus_message_get_signature.argtypes = [pointer, ctypes.c_int]
    library.sd_bus_message_get_signature.restype = ctypes.c_char_p
    library.sd_bus_message_unref.argtypes = [pointer]
    library.sd_bus_message_unref.restype = pointer
    library.sd_bus_close.argtypes = [pointer]
    library.sd_bus_close.restype = None
    library.sd_bus_unref.argtypes = [pointer]
    library.sd_bus_unref.restype = pointer
    return library


def checked(code: int) -> None:
    if code < 0:
        raise RuntimeError("systemd subscription operation failed")


class SystemdSubscription:
    def __init__(self) -> None:
        self.library = load_library()
        self.bus = ctypes.c_void_p()
        reply = ctypes.c_void_p()
        try:
            checked(self.library.sd_bus_new(ctypes.byref(self.bus)))
            if not self.bus.value:
                raise RuntimeError("systemd subscription allocation failed")
            checked(self.library.sd_bus_set_method_call_timeout(self.bus, 5_000_000))
            checked(self.library.sd_bus_set_address(self.bus, SYSTEM_BUS_ADDRESS.encode("ascii")))
            checked(self.library.sd_bus_set_bus_client(self.bus, 1))
            checked(self.library.sd_bus_start(self.bus))
            checked(self.library.sd_bus_call_method(
                self.bus, b"org.freedesktop.systemd1", b"/org/freedesktop/systemd1",
                b"org.freedesktop.systemd1.Manager", b"Subscribe", None, ctypes.byref(reply), b""))
            if not reply.value or self.library.sd_bus_message_get_signature(reply, 1) != b"":
                raise RuntimeError("unexpected systemd subscription reply")
        except BaseException:
            if reply.value:
                self.library.sd_bus_message_unref(reply)
                reply.value = None
            self.close()
            raise
        finally:
            if reply.value:
                self.library.sd_bus_message_unref(reply)

    def close(self) -> None:
        if self.bus.value:
            self.library.sd_bus_close(self.bus)
            self.library.sd_bus_unref(self.bus)
            self.bus.value = None

    def check_connection(self) -> None:
        if not self.bus.value:
            raise RuntimeError("systemd subscription is closed")
        checked(self.library.sd_bus_process(self.bus, None))
        if self.library.sd_bus_is_open(self.bus) <= 0:
            raise RuntimeError("systemd subscription disconnected")

    def __enter__(self) -> SystemdSubscription:
        return self

    def __exit__(self, *_exception: object) -> None:
        self.close()


def subscription_worker() -> None:
    """Fixed entry point for the externally bounded worker; stdin is a lease."""
    with SystemdSubscription() as subscription:
        print("SUBSCRIBED", flush=True)
        while True:
            subscription.check_connection()
            if select.select([sys.stdin.buffer], [], [], 0.25)[0]:
                if sys.stdin.buffer.read(1) != b"":
                    raise RuntimeError("unexpected subscription control input")
                return
