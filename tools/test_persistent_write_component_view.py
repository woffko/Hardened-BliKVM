import copy
import hashlib
import json
import unittest
from typing import cast

from tools import persistent_write_component_view as view

HARNESS = "a" * 64


def section(value: dict[str, object], key: str) -> dict[str, object]:
    return cast(dict[str, object], value[key])


def digests(names: tuple[str, ...]) -> dict[str, str]:
    return {name: hashlib.sha256(name.encode()).hexdigest() for name in names}


def plan() -> dict[str, object]:
    files = {**digests((*view.PLAN_FILES, *view.RECOVERY_HELPERS)), "unrelated.py": "f" * 64}
    return {"schemaVersion": 1, "phase": "wired", "files": files,
            "before": {"runtime": {"trust": "1" * 64, "nginxConfig": "2" * 64, "account": "3" * 64},
                       "nginxUnit": "4" * 64,
                       "persistentDropinHashes": {"wired.service": "5" * 64, "status.service": "6" * 64}}}


class ComponentViewTests(unittest.TestCase):
    def test_components_are_derived_from_stable_plan_fields(self) -> None:
        components = view.component_view(plan(), test_harness=HARNESS)
        expected_files = digests((*view.PLAN_FILES, *view.RECOVERY_HELPERS))
        self.assertEqual(components["updaterPackage"], expected_files["new.deb"])
        self.assertEqual(components["applicationPayload"], expected_files["bindings.json"])
        self.assertEqual(components["controller"], expected_files["controller.py"])
        self.assertEqual(components["trustRoot"], "1" * 64)
        self.assertEqual(components["proxyExecutable"], "4" * 64)
        self.assertEqual(components["proxyConfiguration"], "2" * 64)
        self.assertEqual(components["testHarness"], HARNESS)
        for name in ("recoveryPolicy", "unitContracts"):
            self.assertRegex(components[name], r"^[a-f0-9]{64}$")

    def test_unrelated_fields_do_not_change_components(self) -> None:
        first = view.component_view(plan(), test_harness=HARNESS)
        changed = copy.deepcopy(plan())
        changed["phase"] = "committed"
        section(changed, "files")["unrelated.py"] = "9" * 64
        section(section(changed, "before"), "runtime")["account"] = "8" * 64
        self.assertEqual(view.component_view(changed, test_harness=HARNESS), first)

    def test_missing_or_invalid_components_fail_closed(self) -> None:
        missing = copy.deepcopy(plan())
        del section(section(missing, "before"), "runtime")["trust"]
        with self.assertRaises(ValueError):
            view.component_view(missing, test_harness=HARNESS)
        invalid = copy.deepcopy(plan())
        section(invalid, "files")["new.deb"] = "short"
        with self.assertRaises(ValueError):
            view.component_view(invalid, test_harness=HARNESS)
        with self.assertRaises(ValueError):
            view.component_view(plan(), test_harness="short")

    def test_read_plan_accepts_legacy_keys_and_rejects_duplicates(self) -> None:
        document = plan()
        self.assertEqual(view.read_plan(json.dumps(document).encode())["phase"], "wired")
        for raw in (b'{"a":1,"a":2}', b"", b"{"):
            with self.assertRaises(ValueError):
                view.read_plan(raw)


if __name__ == "__main__":
    unittest.main()
