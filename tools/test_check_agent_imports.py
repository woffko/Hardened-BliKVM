"""The Windows agent's import check (tools/check_agent_imports.py)."""

import unittest

from tools.check_agent_imports import problems


class AgentImportCheckTests(unittest.TestCase):
    def test_known_dlls_api_sets_and_the_flag_pass(self) -> None:
        self.assertEqual(
            problems(
                ["kernel32.dll", "user32.dll", "ntdll.dll", "api-ms-win-core-synch-l1-2-0.dll"],
                [],
                0x800,
            ),
            [],
        )

    def test_a_dll_of_the_programs_folder_is_named(self) -> None:
        found = problems(["kernel32.dll", "hid.dll"], [], 0x800)
        self.assertEqual(found, ["hid.dll is searched in the program's folder first"])
        found = problems(["kernel32.dll"], ["wtsapi32.dll"], 0x800)
        self.assertEqual(found, ["wtsapi32.dll is searched in the program's folder first"])

    def test_the_search_must_be_limited_to_the_system_folder(self) -> None:
        for flags in (None, 0, 0x100):
            self.assertEqual(
                problems(["kernel32.dll"], [], flags),
                ["the load configuration does not limit DLL loads to the system folder"],
            )


if __name__ == "__main__":
    _ = unittest.main()
