# Board-owner acceptance window (H616, 2026-09-16)

First end-to-end acceptance of the Rust board owner on the real board: our
composite USB gadget was created and bound by our own media attach, our native
ATX executor pulsed the real lines inside tolerance, and the reversible gate
restored KVMD and its gadget afterwards. Everything below is device evidence;
the private receipts live under `evidence/private` and are not committed.

## What ran

Command (operator-approved window, `--takeover-udc` handles the single UDC):

```
tools/stage_device_board_profile_gate.py --confirm-handoff --takeover-udc \
  --accept-media-hid --accept-timeout-seconds 240 --accept-image kubuntu.img \
  --accept-atx-bench --confirm-physical-disconnect
```

1. `tools/stage_device_board_bundle.py` staged the verified bundle
   (`sha256 53d5e50a…`, 4.12 MB) into the gate runtime root; the remote hash was
   compared before the archive was moved into place
   (`board-bundle-transfer-20260916-k.json`).
2. `tools/stage_device_owner_image.py` linked the catalogued image into the
   owner's image directory after hashing it on the board
   (`owner-image-20260916-a.json`, `kubuntu.img`, 6 841 958 400 B, sha256
   `ef89de85…`).
3. The gate stopped KVMD, stopped `kvmd-otg`/`kvmd-main` to release the single
   UDC, started our owner, ran the native GPIO bench and waited for the
   composite gadget.
4. With the owner running, a temporary server instance was pointed at the owner
   control socket (`/run/hardened-blikvm-gadget/control.sock`) behind the
   existing HTTPS front, and the three GUI probes ran against it.

## Results (all receipts under `evidence/private`)

- Gate receipt `board-handoff-gate-20260916-k.json`: `acceptanceFacts`
  `gadget_present=yes`, `udc_bound=yes`,
  `functions=hid.usb0,hid.usb1,hid.usb2,mass_storage.usb0`,
  `msd_file=kubuntu.img`, `hidg_devices=/dev/hidg0,/dev/hidg1,/dev/hidg2`,
  `hid_write=ok`; `atxBenchFacts` every action `ok` and `within_tolerance=yes`
  (500 ms / 5000 ms / 500 ms, `run_rc=0`); `takeoverUdc=true`,
  `physicalDisconnectConfirmed=true`.
- GUI probes: `virtual-media-acceptance-20260916-a.json` (passed, select +
  attach + eject applied), `restricted-keyboard-20260916-a.json` (passed),
  `restricted-power-20260916-a.json` (passed, ATX applied through our GPIO).
- Teardown `board-handoff-teardown-20260916-a.json`: `owner_stopped=yes`,
  `hardened_gadget_removed=yes`, `runtime_removed=yes`, `kvmd_restarted=yes`,
  `kvmd_udc_bound=yes`.

Acceptance report `phase8-acceptance-20260916-b.json` now shows phases 0, 1, 2,
3, 5, 6 and 8 complete; only the KVMD removal remains open
(`overall=incomplete`, `hostMissing=0`, `deviceUnrun=1`).

## Device-only defects this window found and fixed

None of these could fail on the fake configfs used by the host tests; each is
now pinned by a host test.

1. The staging tool used `systemctl start` on an already-active unit, so a
   deployed bundle never actually replaced the running binary (`server_exe_matches`
   now proves the running process is the staged one).
2. The single UDC is rebound by the vendor OTG helper: `kvmd.service` alone is
   not enough, `kvmd-otg`/`kvmd-main` must stop, and the freed controller must be
   judged from gadget bindings (the per-UDC state file is stale on this board).
3. The HID report descriptor was written as hex text into the binary
   `report_desc` attribute; it is now written as raw bytes.
4. Mass-storage LUN attributes were written after `lun.0/file`, which the kernel
   then rejects with `EBUSY`; `ro`/`cdrom`/`removable` now precede the file.
5. A function symlink target must be the absolute configfs path; every relative
   form is rejected with `ENOENT`.
6. `MAX_LINES` (the ioctl payload array size) was misused as an offset bound, so
   this board's lines 228/272 were rejected; offsets are now bounded by the
   chip's reported line count.
7. The chip-info ioctl number had the wrong type byte, which made the owner fail
   to start at all.
8. The ATX bench fact-key set contained trailing-underscore names, so a passing
   bench was rejected by the gate's own parser.

## Steady state after the window

KVMD and its gadget are running again and own the UDC; our runtime root, the
`hardened` gadget and the temporary server instance are gone. The image symlink
under `/var/lib/hardened-blikvm-gadget/images` and the KVMD helper journals
(`atx-journal.json`, `media-journal.json`, `hid-ledger.json`) are the only
leftovers and are documented cleanup items.

## Remaining device gate

`KVMD removal` (`tools/stage_device_kvmd_removal.py --confirm-removal`) requires
the replacement board-owner unit to be installed and serving first: the tool
stops and disables the four KVMD units, backs up their unit files and refuses to
finish unless our owner and its gadget are still alive. That is the next window
and the last open gate.
