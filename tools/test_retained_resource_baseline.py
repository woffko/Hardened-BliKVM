import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from typing import Any, cast

from tools import observe_service_resources as observer
from tools.capture_retained_resource_baseline import validate


def fixture() -> list[observer.Sample]:
    return [
        {"monotonicNs": 1_000_000_000 + index * 10_000_000_000, "bootIdSha256": "a" * 64,
         "units": {unit: {"mainPid": 100, "mainStartTicks": 99, "restarts": 0,
                          "cpuUsec": index * 10_000_000, "rssKiB": 1024 + index,
                          "fileDescriptors": 10, "threads": 2, "processes": 1} for unit in observer.UNITS}}
        for index in range(observer.SAMPLES)
    ]


class ResourceTests(unittest.TestCase):
    def test_snapshot_profile_checks_both_slot_links_and_executable(self) -> None:
        root = Path("/var/lib/hardened-blikvm-slots")
        current = root / "current"
        executable = Path("/proc/100/exe")
        seq6 = root / "app-00000000000000000006-8eedeb5ce21ca08b90336cad27bdd7f16c6502388a1f13bc943a2026d80534ac"
        seq5 = root / observer.SLOT
        units = fixture()[0]["units"]
        def unit_sample(unit: str) -> observer.UnitSample:
            return units[unit]
        boot = "12345678-1234-1234-1234-123456789abc"
        for profile, slot in (("seq5", seq5), ("seq6", seq6)):
            for failure in (None, "initial", "executable", "final"):
                calls: list[Path] = []

                def resolve(path: Path, strict: bool = False) -> Path:
                    self.assertTrue(strict)
                    calls.append(path)
                    if path == executable:
                        return (root / "foreign" if failure == "executable" else slot) / "bin/bli-kvm-server"
                    self.assertEqual(path, current)
                    if failure == "initial" or (failure == "final" and len(calls) == 3):
                        return root / "foreign"
                    return slot

                with self.subTest(profile=profile, failure=failure), \
                        patch.object(Path, "resolve", resolve), \
                        patch.object(observer, "read_bounded", return_value=boot), \
                        patch.object(observer, "unit_sample", side_effect=unit_sample):
                    if failure is None:
                        self.assertEqual(observer.snapshot(profile)["units"], units)
                        self.assertEqual(calls, [current, executable, current])
                    else:
                        with self.assertRaises(ValueError):
                            observer.snapshot(profile)
        with patch.object(observer, "read_bounded") as read:
            with self.assertRaisesRegex(ValueError, "unknown status profile"):
                observer.snapshot("unknown")
            read.assert_not_called()

    def test_cpu_units_peaks_and_growth(self) -> None:
        samples = fixture()
        summary = observer.summarize(samples)
        units = cast(dict[str, dict[str, float | int]], summary["units"])
        unit = units[observer.UNITS[0]]
        self.assertEqual(unit["cpuMeanCorePercent"], 100)
        self.assertEqual(unit["cpuMaxIntervalCorePercent"], 100)
        self.assertEqual(unit["rssMaxKiB"], 1084)
        self.assertEqual(unit["rssGrowthKiB"], 60)
        value = {"result": "observed", "workload": "no-observer-generated-workload", "samples": samples, "summary": summary}
        self.assertEqual(validate(json.dumps(value).encode()), value)

    def test_identity_clock_and_incomplete_rejected(self) -> None:
        for key in ("mainPid", "mainStartTicks", "restarts"):
            samples = fixture()
            samples[1]["units"][observer.UNITS[0]][key] += 1
            with self.assertRaises(ValueError):
                observer.summarize(samples)
        for kind in ("boot", "cpu", "clock", "duration", "count"):
            samples = fixture()
            if kind == "boot":
                samples[1]["bootIdSha256"] = "b" * 64
            elif kind == "cpu":
                samples[2]["units"][observer.UNITS[0]]["cpuUsec"] = 0
            elif kind == "clock":
                samples[1]["monotonicNs"] = samples[0]["monotonicNs"]
            elif kind == "duration":
                samples[-1]["monotonicNs"] -= 1
            else:
                samples.pop()
            with self.assertRaises(ValueError):
                observer.summarize(samples)

    def test_wire_rejects_extra_fields_boolean_numbers_and_forged_summary(self) -> None:
        samples = fixture()
        good: dict[str, Any] = {"result": "observed", "workload": "no-observer-generated-workload", "samples": samples,
                "summary": observer.summarize(samples)}
        for kind in ("extra", "bool", "summary", "unit"):
            bad = copy.deepcopy(good)
            if kind == "extra":
                bad["unexpected"] = "rejected"
            elif kind == "bool":
                bad["samples"][0]["units"][observer.UNITS[0]]["cpuUsec"] = True
            elif kind == "unit":
                bad["samples"][0]["units"]["foreign.service"] = {}
            else:
                bad["summary"] = {}
            with self.assertRaises(ValueError):
                validate(json.dumps(bad).encode())

    def test_stat_comm_parentheses_and_spaces(self) -> None:
        self.assertEqual(observer.start_ticks("123 (name with ) spaces) S " + "0 " * 18 + "876 0"), 876)
        with self.assertRaises(ValueError):
            observer.start_ticks("123 (short) S")

    def test_sample_reads_all_cgroup_processes_and_rejects_races(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            unit = observer.UNITS[0]
            group = root / "sys/fs/cgroup/system.slice" / unit
            group.mkdir(parents=True)
            (group / "cgroup.procs").write_text("100\n101\n")
            (group / "cpu.stat").write_text("usage_usec 12345\nuser_usec 10000\nsystem_usec 2345\n")
            for pid in (100, 101):
                proc = root / "proc" / str(pid)
                (proc / "fd").mkdir(parents=True)
                (proc / "fd/0").touch()
                (proc / "status").write_text("Name:\ttest\nVmRSS:\t1024 kB\nThreads:\t2\n")
                (proc / "stat").write_text(str(pid) + " (test) S " + "0 " * 18 + "99 0")
            state = {"ActiveState": "active", "MainPID": "100", "NRestarts": "0", "ControlGroup": "/system.slice/" + unit}
            def mapped_path(value: str) -> Path:
                return root / value.lstrip("/")

            with patch.object(observer, "Path", side_effect=mapped_path):
                with patch.object(observer, "properties", return_value=state):
                    value = observer.unit_sample(unit)
                    self.assertEqual(value["rssKiB"], 2048)
                    self.assertEqual(value["threads"], 4)
                    self.assertEqual(value["fileDescriptors"], 2)
                    self.assertEqual(value["cpuUsec"], 12345)
                with patch.object(observer, "properties", side_effect=[state, {**state, "MainPID": "101"}]):
                    with self.assertRaisesRegex(ValueError, "unit changed during sample"):
                        observer.unit_sample(unit)
                with patch.object(observer, "properties", return_value={**state, "ControlGroup": "/foreign"}):
                    with self.assertRaisesRegex(ValueError, "unexpected service cgroup"):
                        observer.unit_sample(unit)
                (group / "nested").mkdir()
                with patch.object(observer, "properties", return_value=state):
                    with self.assertRaisesRegex(ValueError, "nested service cgroup"):
                        observer.unit_sample(unit)

    def test_no_device_writes_or_secret_reads(self) -> None:
        source = Path(observer.__file__).read_text()
        for forbidden in ("write_text", "write_bytes", "environ", "cmdline", '"restart"', '"stop"', '"start"', "http://", "https://"):
            self.assertNotIn(forbidden, source)


if __name__ == "__main__":
    unittest.main()
