import unittest
from unittest.mock import patch

from tools import application_write_observer as observer


class ObserverTests(unittest.TestCase):
    def test_only_proven_process_free_units_are_idle(self) -> None:
        idle = {"ActiveState": "inactive", "MainPID": "0", "ControlPID": "0", "Job": ""}
        for fields, expected in ((idle, False), ({**idle, "ActiveState": "failed"}, False),
                                 ({**idle, "MainPID": "12"}, True), ({**idle, "ControlPID": "12"}, True),
                                 ({**idle, "ActiveState": "deactivating"}, True),
                                 ({**idle, "Job": "19"}, True)):
            with patch.object(observer.upload, "properties", return_value=fields):
                self.assertEqual(observer.unit_busy("unit"), expected)
        with patch.object(observer.upload, "properties", return_value={**idle, "MainPID": "?"}):
            with self.assertRaises(ValueError):
                observer.unit_busy("unit")

    def test_closure_never_stops_worker_or_server(self) -> None:
        with patch.object(observer.upload, "checked") as checked, patch.object(observer, "require_closed_admission") as verify:
            observer.close_admission()
            checked.assert_called_once_with(["systemctl", "stop", observer.contract.ACTIVATION, observer.contract.UPLOAD], timeout=20)
            verify.assert_called_once_with()

    def test_inventory_counts_busy_workers_only(self) -> None:
        raw = (b"hardened-blikvm-update-upload@1.service loaded inactive dead Upload\n"
               b"hardened-blikvm-update-activation@2.service loaded activating start Activation\n")
        with patch.object(observer.upload, "checked", return_value=raw), patch.object(observer, "unit_busy", side_effect=(True, False)):
            self.assertEqual(observer.live_workers(), 1)
        with patch.object(observer.upload, "checked", return_value=b"unrelated.service loaded active running foreign"):
            with self.assertRaises(ValueError):
                observer.live_workers()

    def test_dangling_endpoint_or_pending_start_is_not_closed(self) -> None:
        idle = {"ActiveState": "inactive", "SubState": "dead", "Job": ""}
        with patch.object(observer.upload, "properties", return_value=idle), patch.object(observer.upload.application.infrastructure, "exists", return_value=True):
            with self.assertRaises(ValueError):
                observer.require_closed_admission()
        with patch.object(observer.upload, "properties", return_value={**idle, "Job": "42"}):
            with self.assertRaises(ValueError):
                observer.require_closed_admission()

    def test_failed_socket_is_closed_only_with_no_job_and_no_endpoint(self) -> None:
        fields = {"ActiveState": "failed", "SubState": "failed", "Job": "0"}
        with patch.object(observer.upload, "properties", return_value=fields), patch.object(observer.upload.application.infrastructure, "exists", return_value=False):
            observer.require_closed_admission()
        with patch.object(observer.upload, "properties", return_value={**fields, "SubState": "listening"}):
            with self.assertRaises(ValueError):
                observer.require_closed_admission()
        with patch.object(observer.upload, "properties", return_value={**fields, "Job": "42"}):
            with self.assertRaises(ValueError):
                observer.require_closed_admission()


if __name__ == "__main__":
    unittest.main()
