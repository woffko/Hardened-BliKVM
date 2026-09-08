import ctypes
import contextlib
import io
from types import SimpleNamespace
from typing import Any
import unittest
from unittest.mock import MagicMock, patch

from tools import systemd_subscription as subscription
from tools.status_restore_event import SYSTEM_BUS_ADDRESS


def fake_library() -> MagicMock:
    library = MagicMock()
    def allocate(pointer: Any) -> int:
        ctypes.cast(pointer, ctypes.POINTER(ctypes.c_void_p))[0] = ctypes.c_void_p(1234)
        return 0
    def call(*args: Any) -> int:
        ctypes.cast(args[6], ctypes.POINTER(ctypes.c_void_p))[0] = ctypes.c_void_p(5678)
        return 1
    library.sd_bus_new.side_effect = allocate
    library.sd_bus_call_method.side_effect = call
    library.sd_bus_message_get_signature.return_value = b""
    library.sd_bus_process.return_value = 0
    library.sd_bus_is_open.return_value = 1
    for name in ("sd_bus_set_method_call_timeout", "sd_bus_set_address", "sd_bus_set_bus_client", "sd_bus_start"):
        getattr(library, name).return_value = 0
    return library


class SubscriptionTests(unittest.TestCase):
    def test_connection_loss_is_detected_and_closed(self) -> None:
        library = fake_library()
        with patch.object(subscription, "load_library", return_value=library):
            with subscription.SystemdSubscription() as held:
                held.check_connection()
                library.sd_bus_is_open.return_value = 0
                with self.assertRaises(RuntimeError):
                    held.check_connection()
            with self.assertRaises(RuntimeError):
                held.check_connection()
        library.sd_bus_close.assert_called_once()

    def test_worker_accepts_only_lease_eof_and_checks_connection(self) -> None:
        for raw in (b"", b"x"):
            lease = MagicMock()
            source = SimpleNamespace(buffer=io.BytesIO(raw))
            output = io.StringIO()
            with patch.object(subscription, "SystemdSubscription", return_value=lease), \
                    patch.object(subscription.sys, "stdin", source), \
                    patch.object(subscription.select, "select", return_value=([source.buffer], [], [])), \
                    contextlib.redirect_stdout(output):
                if raw:
                    with self.assertRaises(RuntimeError):
                        subscription.subscription_worker()
                else:
                    subscription.subscription_worker()
            self.assertEqual(output.getvalue(), "SUBSCRIBED\n")
            lease.__enter__.return_value.check_connection.assert_called_once()
            lease.__exit__.assert_called_once()

    def test_real_library_allocation_configuration_and_cleanup_without_connecting(self) -> None:
        library = subscription.load_library()
        bus = ctypes.c_void_p()
        try:
            self.assertEqual(library.sd_bus_new(ctypes.byref(bus)), 0)
            self.assertIsNotNone(bus.value)
            self.assertGreaterEqual(library.sd_bus_set_method_call_timeout(bus, 5_000_000), 0)
            self.assertGreaterEqual(library.sd_bus_set_address(bus, SYSTEM_BUS_ADDRESS.encode()), 0)
            self.assertGreaterEqual(library.sd_bus_set_bus_client(bus, 1), 0)
            self.assertEqual(len(library.sd_bus_call_method.argtypes), 8)
            self.assertIsNone(library.sd_bus_close.restype)
        finally:
            if bus.value:
                library.sd_bus_close(bus)
                library.sd_bus_unref(bus)

    def test_allocation_failure_never_closes_a_null_bus(self) -> None:
        library = fake_library()
        library.sd_bus_new.side_effect = None
        library.sd_bus_new.return_value = -12
        with patch.object(subscription, "load_library", return_value=library), self.assertRaises(RuntimeError):
            subscription.SystemdSubscription()
        library.sd_bus_close.assert_not_called()
        library.sd_bus_unref.assert_not_called()

    def test_fixed_empty_subscribe_timeout_before_start_and_held_connection(self) -> None:
        library = fake_library()
        with patch.object(subscription, "load_library", return_value=library):
            with subscription.SystemdSubscription() as held:
                self.assertEqual(held.bus.value, 1234)
                library.sd_bus_close.assert_not_called()
                library.sd_bus_set_address.assert_called_once_with(held.bus, SYSTEM_BUS_ADDRESS.encode())
                library.sd_bus_set_method_call_timeout.assert_called_once_with(held.bus, 5_000_000)
                args = library.sd_bus_call_method.call_args.args
                self.assertEqual(args[1:6], (b"org.freedesktop.systemd1", b"/org/freedesktop/systemd1",
                                             b"org.freedesktop.systemd1.Manager", b"Subscribe", None))
                self.assertEqual(args[7:], (b"",))
            held.close()
        calls = [item[0] for item in library.method_calls]
        self.assertLess(calls.index("sd_bus_set_method_call_timeout"), calls.index("sd_bus_start"))
        library.sd_bus_close.assert_called_once()
        library.sd_bus_unref.assert_called_once()
        library.sd_bus_message_unref.assert_called_once()

    def test_each_setup_failure_closes_allocated_connection(self) -> None:
        for name in ("sd_bus_set_method_call_timeout", "sd_bus_set_address", "sd_bus_set_bus_client", "sd_bus_start",
                     "sd_bus_call_method"):
            library = fake_library()
            method = getattr(library, name)
            method.side_effect = None
            method.return_value = -5
            with patch.object(subscription, "load_library", return_value=library), self.assertRaises(RuntimeError):
                subscription.SystemdSubscription()
            library.sd_bus_close.assert_called_once()
            library.sd_bus_unref.assert_called_once()

    def test_unexpected_reply_and_consumer_error_release_connection(self) -> None:
        library = fake_library()
        library.sd_bus_message_get_signature.return_value = b"s"
        with patch.object(subscription, "load_library", return_value=library), self.assertRaises(RuntimeError):
            subscription.SystemdSubscription()
        library.sd_bus_message_unref.assert_called_once()
        library.sd_bus_close.assert_called_once()
        library = fake_library()
        with patch.object(subscription, "load_library", return_value=library), self.assertRaises(ValueError):
            with subscription.SystemdSubscription():
                raise ValueError("synthetic consumer failure")
        library.sd_bus_close.assert_called_once()


if __name__ == "__main__":
    unittest.main()
