"""Pin what the SD image builder does to a board's state (tools/image/build_sd_image.sh).

The builder runs against a live board, so these tests exercise the pieces
that can run alone (the fstab rewrite) and pin the rest by their text.
"""

from __future__ import annotations

import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
BUILDER = ROOT / "tools/image/build_sd_image.sh"
GROW = ROOT / "tools/image/hardened-blikvm-grow-image-store.sh"
FIRSTBOOT = ROOT / "tools/image/hardened-blikvm-firstboot.sh"
FIRSTBOOT_UNIT = ROOT / "tools/image/hardened-blikvm-firstboot.service"
QUOTE = "'\"'\"'"


def fstab_program(script: str) -> str:
    start = script.index('awk -v root="$root_uuid"')
    start = script.index(QUOTE, start) + len(QUOTE)
    return script[start : script.index(QUOTE, start)]


class BuildSdImageTests(unittest.TestCase):
    def setUp(self) -> None:
        self.script = BUILDER.read_text(encoding="utf-8")

    def test_the_encoder_driver_is_checked_against_the_images_kernel(self) -> None:
        # The check runs on the tree the image is made of, before the file
        # system is: a driver for another kernel stops the build.
        check = 'python3 "$script_dir/../ve_module.py" image "$staging"\n'
        self.assertEqual(self.script.count(check), 1)
        self.assertLess(self.script.index(check), self.script.index('root_img="$work/root.ext4"'))
        self.assertLess(self.script.index("tar --numeric-owner --no-acls --no-xattrs"), self.script.index(check))
        self.assertTrue((ROOT / "tools/ve_module.py").is_file())
        # The module lives outside the directories the fetch leaves out.
        fetch = self.script[self.script.index("fetch() {") : self.script.index("assemble() {")]
        self.assertNotIn("--exclude=./usr/lib/hardened-blikvm", fetch)
        self.assertNotIn("--exclude=./usr", fetch)

    def test_the_image_store_is_mounted_after_tmpfiles_with_its_new_uuid(self) -> None:
        board = (
            "UUID=a / ext4 defaults,noatime 0 1\n"
            "UUID=b /mnt ext4 defaults 0 0\n"
            "UUID=c /var/lib/hardened-blikvm-gadget/images ext4 rw,nosuid,nodev,noexec,nofail,"
            "x-systemd.requires=systemd-tmpfiles-setup.service,x-systemd.device-timeout=10s 0 2\n"
        )
        with tempfile.TemporaryDirectory() as directory:
            program = Path(directory, "fstab.awk")
            program.write_text(fstab_program(self.script), encoding="utf-8")
            source = Path(directory, "fstab")
            source.write_text(board, encoding="utf-8")
            result = subprocess.run(
                ["awk", "-v", "root=R", "-v", "data=D", "-v", "msd=M", "-f", str(program), str(source)],
                check=True,
                capture_output=True,
                text=True,
            )
        self.assertEqual(
            result.stdout.splitlines(),
            [
                "UUID=R / ext4 defaults,noatime 0 1",
                "UUID=D /mnt ext4 defaults 0 0",
                "UUID=M /var/lib/hardened-blikvm-gadget/images ext4 "
                "rw,nosuid,nodev,noexec,nofail,x-systemd.device-timeout=10s 0 2",
            ],
        )

    def test_the_root_names_its_system_sequence(self) -> None:
        # A card written from the image refuses older system bundles by it
        # (security review of 2026-10-18, MED-07); the RAW bundle builder
        # checks it against its own sequence.
        self.assertIn("SYSTEM_SEQUENCE=${BLI_KVM_IMAGE_SYSTEM_SEQUENCE:-}", self.script)
        self.assertIn('*[!0-9]*) echo "BLI_KVM_IMAGE_SYSTEM_SEQUENCE is invalid"', self.script)
        self.assertIn(
            '> "$staging/usr/share/hardened-blikvm/system-update/system-sequence"',
            self.script,
        )
        self.assertIn('"$IMAGE_RELEASE" "$SYSTEM_SEQUENCE"', self.script)

    def test_the_image_store_root_takes_the_store_mode_and_group(self) -> None:
        self.assertIn('set_inode_field / mode 040770" "$work/msd.ext4"', self.script)
        self.assertIn('set_inode_field / gid $store_gid" "$work/msd.ext4"', self.script)
        # The result is checked, not assumed.
        self.assertIn('grep -q "Mode:  0770"', self.script)
        self.assertIn("the image store's owner could not be set", self.script)
        self.assertIn('$1 == "hardened-blikvm" { print $3 }', self.script)

    def test_the_source_boards_state_is_reset(self) -> None:
        for name in (
            "admin-features.json",
            "firewall-mode.json",
            "journal.json",
            "media-journal.json",
            "atx-journal.json",
            "hid-ledger.json",
            "hid-only.env",
            "display-sleep.env",
            # The target agent is off on a new card.
            "target-agent.env",
            # The owner's record of the switches kept in system files: a new
            # card records its own at its first start.
            "system-switches.json",
        ):
            self.assertIn(f'"$gadget_state/{name}"', self.script, name)
            self.assertIn(f"-name {name}", self.script, name)
        # The source target's monitors are not the new card's.
        self.assertIn('"$staging/var/lib/hardened-blikvm/screen-area.json"', self.script)
        self.assertIn('leftover="$leftover screen-area.json"', self.script)
        for leftover in ("kvmd-backup-*", "unit-owner-*", "update-trust-*.json", "web-pre-*"):
            self.assertIn(f'"$gadget_state"/{leftover}', self.script, leftover)
        self.assertIn('"$staging"/etc/hardened-blikvm/update-supervisor-*.json', self.script)
        # The video settings chosen on the source board (frame rate, quality,
        # resolution) give way to the capture unit's defaults.
        self.assertIn('"$staging/var/lib/hardened-blikvm-video/ustreamer.env"', self.script)
        # The firewall's configuration (not its confirmed mode) stays.
        self.assertIn('-not -path "$staging/etc/hardened-blikvm/firewall-mode.json"', self.script)
        self.assertIn("source board state remains", self.script)

    def test_a_new_card_has_an_empty_system_action_journal(self) -> None:
        # The source board's reboots, one perhaps still pending, would refuse
        # a new card's first reboot or fill its sixteen places.
        journal = "$staging/var/lib/hardened-blikvm-system-actions"
        self.assertIn(f'"{journal}/journal-a.bin"', self.script)
        self.assertIn(f'"{journal}/journal-b.bin"', self.script)
        self.assertIn(f'find "{journal}" -type f', self.script)
        self.assertIn("source board state remains: the system-action journal", self.script)
        # The owner makes it at its first start: the units that name it are
        # not held to it by the image's check of the paths units need.
        self.assertIn(
            'created_at_first_boot="$created_at_first_boot '
            '/var/lib/hardened-blikvm-system-actions/journal-a.bin"',
            self.script,
        )
        # The directory stays: the owner's store wants it, and the image's
        # reviewed state under var/lib names it.
        self.assertIn(
            " hardened-blikvm-system-actions ",
            self.script.split('known_state="', 1)[1].split('"', 1)[0],
        )

    def test_the_quic_gateway_is_disabled_on_a_new_card(self) -> None:
        # The board's QUIC switch is the unit's enablement: the link a source
        # board with QUIC switched on carries must not reach the image.
        self.assertIn(
            'rm -f "$staging/etc/systemd/system/multi-user.target.wants/'
            'hardened-blikvm-quic.service"',
            self.script,
        )
        unit = (ROOT / "packaging/systemd/hardened-blikvm-quic.service").read_text(encoding="utf-8")
        self.assertIn("WantedBy=multi-user.target", unit)
        # The gateway keeps nothing on disk, so the list of reviewed state
        # under var/lib needs no entry for it.
        self.assertNotIn("StateDirectory=", unit)
        self.assertNotIn("hardened-blikvm-quic ", self.script.split('known_state="', 1)[1].split('"', 1)[0])

    def test_optional_feature_state_is_removed_and_var_lib_is_an_allowlist(self) -> None:
        for path in (
            '"$staging/var/lib/private/hardened-blikvm-picoclaw"',
            '"$staging/var/lib/hardened-blikvm-picoclaw"/*',
            '"$staging/var/lib/hardened-blikvm-admin/scripts"/*',
            '"$staging/var/lib/hardened-blikvm-tailscale"/*',
        ):
            self.assertIn(path, self.script, path)
        self.assertIn("feature state remains in $state", self.script)
        self.assertIn("unreviewed state under var/lib:", self.script)
        self.assertIn('known_state=" alsa apt', self.script)
        self.assertIn("hardened-blikvm-hid-ledger.conf", self.script)

    def test_the_source_boards_name_servers_are_dropped(self) -> None:
        self.assertIn('printf "%s\\n" "# Generated by NetworkManager" > "$staging/etc/resolv.conf"', self.script)
        # A symlink (systemd-resolved) is left alone.
        self.assertIn('[ ! -L "$staging/etc/resolv.conf" ]', self.script)

    def test_the_image_notes_say_first_setup_sets_root(self) -> None:
        self.assertIn("web password and root's password to the same value", self.script)

    def test_the_data_partition_leaves_the_rest_to_the_image_store(self) -> None:
        self.assertIn("ROOT_SECTORS=$((12 * 1024 * 1024 * 2))", self.script)
        self.assertIn("DATA_SECTORS=$((8 * 1024 * 1024 * 2))", self.script)
        self.assertIn("msd_sectors=$((IMAGE_BYTES / 512 - msd_start))", self.script)

    def test_the_first_boot_grows_the_image_store(self) -> None:
        self.assertIn(
            '"$script_dir/hardened-blikvm-grow-image-store.sh" '
            '"$staging/usr/libexec/hardened-blikvm/hardened-blikvm-grow-image-store"',
            self.script,
        )
        firstboot = FIRSTBOOT.read_text(encoding="utf-8")
        grow = firstboot.index("/usr/libexec/hardened-blikvm/hardened-blikvm-grow-image-store ||")
        # A failure is logged; the marker is still cleared.
        self.assertLess(grow, firstboot.index("rm -f /etc/hardened-blikvm/.firstboot-pending"))
        unit = FIRSTBOOT_UNIT.read_text(encoding="utf-8")
        self.assertIn("After=var-lib-hardened\\x2dblikvm\\x2dgadget-images.mount", unit)

    @unittest.skipUnless(shutil.which("sfdisk"), "sfdisk is not installed")
    def test_the_last_partition_takes_the_rest_of_the_disk(self) -> None:
        def table(disk: Path) -> list[str]:
            dump = subprocess.run(["sfdisk", "-d", str(disk)], check=True, capture_output=True, text=True)
            return [line.split(":", 1)[1].replace(" ", "") for line in dump.stdout.splitlines() if " : start=" in line]

        with tempfile.TemporaryDirectory() as directory:
            disk = Path(directory, "card.img")
            with disk.open("wb") as card:
                card.truncate(200 * 1024 * 1024)
            layout = "label: dos\nstart=2048, size=40960\nstart=43008, size=40960\nstart=83968, size=40960\n"
            subprocess.run(["sfdisk", "-q", str(disk)], input=layout, check=True, text=True)
            subprocess.run(["sh", str(GROW), str(disk), "3"], check=True, capture_output=True)
            self.assertEqual(
                table(disk),
                [
                    "start=2048,size=40960,type=83",
                    "start=43008,size=40960,type=83",
                    "start=83968,size=325632,type=83",
                ],
            )
            again = subprocess.run(["sh", str(GROW), str(disk), "3"], check=True, capture_output=True, text=True)
            self.assertIn("already takes the rest of the card", again.stdout)

            # Less than 64 MiB of room is left alone.
            with disk.open("wb") as card:
                card.truncate(0)
                card.truncate(100 * 1024 * 1024)
            subprocess.run(["sfdisk", "-q", str(disk)], input=layout, check=True, text=True)
            small = subprocess.run(["sh", str(GROW), str(disk), "3"], check=True, capture_output=True, text=True)
            self.assertIn("already takes the rest of the card", small.stdout)
            self.assertEqual(table(disk)[2], "start=83968,size=40960,type=83")

    def test_the_fakeroot_program_has_no_stray_quote(self) -> None:
        # The staged root is prepared by one single-quoted `sh -c` program; a
        # bare apostrophe (in a comment, too) ends the quote and breaks it,
        # which `sh -n` does not notice. Apostrophes are written '"'"'.
        start = self.script.index("fakeroot -s \"$work/fakeroot.state\" -- sh -eu -c '") + len(
            "fakeroot -s \"$work/fakeroot.state\" -- sh -eu -c '"
        )
        end = self.script.index("' sh \"$staging\"", start)
        program = self.script[start:end].replace(QUOTE, "")
        self.assertNotIn("'", program)

    def test_the_image_catalog_starts_empty(self) -> None:
        self.assertIn('"  \\"entries\\": []"', self.script)
        self.assertIn('> "$staging/etc/hardened-blikvm-gadget/catalog.json"', self.script)
        # Only in its former place, for tmpfiles to copy (CRIT-01).
        self.assertIn('rm -rf "$staging/var/lib/hardened-blikvm-gadget/catalog"', self.script)


if __name__ == "__main__":
    unittest.main()
