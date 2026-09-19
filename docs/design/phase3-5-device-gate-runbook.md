# Phase 3-5 device-gate runbook

Procedure for the remaining device-bound gates. It is a checklist, not an
approval: every step below mutates the test device or injects into a target and
requires explicit operator confirmation in the current session. Private
receipts, endpoints, accounts and credentials stay outside Git.

## Preconditions

- SSH and GUI test credentials supplied only on stdin, for example through
  `project-memory-exec`; endpoint and account come from the enrolled test
  asset, never a public file.
- Enrolled state confirmed for the retained update path: slots seq4-seq11 plus
  `current`, retained guard and evidence store present.
- For ATX gates: Power and Reset physically disconnected.
- For HID/mouse gates: a safe, visible, authorized target on the other side of
  the KVM; the operator watches the video while issuing events.

## 0. Read-only log health (no mutation)

Before any reboot gate, confirm log health without touching the device:

```
project-memory-exec --project BLi-KVM --secret ssh=<id>:ssh_password -- \
python3 -B tools/check_device_log_health.py \
  --endpoint ssh://<host>:22 --username <user> \
  --known-hosts evidence/private/ssh-known-hosts \
  --output evidence/private/<receipt>.json
```

Expected: `logrotateValid=true`, `logrotateServiceOk=true`,
`journaldStorage=persistent`, `journalMountOnDedicatedPartition=true` on the
dedicated partition, `failedUnits=0`, and optionally
`previousBootJournalPresent=true` once a reboot has been observed.

## 1. HID transition service

Isolated gates already passed (see the checkpoint). The deployable profile is
verified with:

```
project-memory-exec --project BLi-KVM --secret ssh=<id>:ssh_password -- \
python3 -B tools/stage_device_hid_profile_gate.py \
  --endpoint ssh://<host>:22 --username <user> \
  --known-hosts evidence/private/ssh-known-hosts \
  --bundle /mnt/d/debug/blikvm/hardened-blikvm/bundles/hardened-blikvm-hid-transition-profile-aarch64.tar.gz \
  --expected-sha256 <sha256> \
  --output evidence/private/<receipt>.json
```

Expected facts: `broker_present=yes`, `refused_without_credentials=yes`,
`started_with_credentials=yes`, `persistentChangeScope=none`.

## 1a. Combined HID + ATX manual test profile (reversible)

For operator-driven manual testing of both restricted capabilities at once on a
separate port, without touching the retained slots or the live GUI:

```
project-memory-exec --project BLi-KVM --secret ssh=<id>:ssh_password -- \
python3 -B tools/stage_device_restricted_control_profile.py \
  --endpoint ssh://<host>:22 --username <user> \
  --known-hosts evidence/private/ssh-known-hosts \
  --bundle /mnt/d/debug/blikvm/hardened-blikvm/bundles/hardened-blikvm-restricted-control-profile-aarch64.tar.gz \
  --expected-sha256 <sha256> \
  --output evidence/private/<receipt>.json
```

Expected facts: `extracted=yes`, `broker_started=yes`,
`control_socket_present=yes`, `server_started=yes`, `health=ok`,
`hid_availability=read_write`, `power_availability=read_write`,
`https_ready=yes`, `persistentChangeScope=runtime-only`. The profile then serves
the operator UI at `https://<host>:8443` (self-signed). Run the same command
with `--teardown` and a new output path to stop the transient units and remove
the `/tmp` runtime root. See
`docs/baselines/phase3-restricted-control-test-profile.md` for the manual
checklist and boundaries.

## 1b. HID lease supersede gate (P3 input fix)

This gate proves the reviewed fix for the "lost lease id" input drop: the owner
supersedes the *same* session's stale lease but still refuses a *different*
session. It writes `evidence/private/hid-lease-supersede-gate-<date>.json`
(probe `hardened-blikvm-hid-lease-gate`, `result=passed`):

```
project-memory-exec --project BLi-KVM --secret ssh=<id>:ssh_password -- \
python3 -B tools/stage_device_hid_lease_gate.py \
  --endpoint ssh://<host>:22 --username <user> \
  --known-hosts evidence/private/ssh-known-hosts \
  --base-url https://<host> --api-username admin \
  --ca-file evidence/private/<host>.crt \
  --confirm --output evidence/private/hid-lease-supersede-gate-<date>.json
```

Expected facts: `superseded=true`, `secondLeaseId` differs from `firstLeaseId`,
`reportsApplied=5` with `reportRejections=0`, `secondSessionRefused=true` with
`secondSessionRejection=lease_conflict`, and the lease released at the end.

## 1c. Tailscale surface gate (fail-closed)

The board ships no Tailscale package, so the surface must be readable and must
refuse every management action. The gate writes
`evidence/private/tailscale-surface-gate-<date>.json` (probe
`hardened-blikvm-tailscale-gate`, `result=passed`):

```
project-memory-exec --project BLi-KVM --secret ssh=<id>:ssh_password -- \
python3 -B tools/stage_device_tailscale_gate.py \
  --endpoint ssh://<host>:22 --username <user> \
  --known-hosts evidence/private/ssh-known-hosts \
  --base-url https://<host> --api-username admin \
  --ca-file evidence/private/<host>.crt \
  --confirm --output evidence/private/tailscale-surface-gate-<date>.json
```

Expected facts: the status answers with `installed=false` while no package is
present, `actionsAttempted=9` with `actionsRefused=9`, `firstRejection` set, and
`mutationsPerformed=false`. Once a reviewed package source exists, this gate is
replaced by the enable/disable proof.

## 1d. Video capability matrix (operator request)

The operator asked for the real delivered rate per reviewed resolution instead
of trusting the UI list. The sweep applies each reviewed mode against the
requested FPS values, records the engine's `captured_fps`/`queued_fps` and the
server `capturedFps`, and restores the original settings. It writes
`evidence/private/video-matrix-<date>.json` (probe
`hardened-blikvm-video-matrix-gate`, `result=passed`):

```
project-memory-exec --project BLi-KVM --secret ssh=<id>:ssh_password -- \
python3 -B tools/capture_device_video_matrix.py \
  --endpoint ssh://<host>:22 --username <user> \
  --known-hosts evidence/private/ssh-known-hosts \
  --base-url https://<host> --api-username admin \
  --ca-file evidence/private/<host>.crt \
  --confirm --output evidence/private/video-matrix-<date>.json
```

Expected facts: one row per reviewed resolution (1080/720/600/480) and requested
FPS (10/20/30/40/50/60), each with `engineResolution` equal to the reviewed mode,
`engineDesiredFps` equal to the requested value, `capturedMatchesServer=true`,
and the original settings restored at the end. This sweep restarts the streamer
per combination, so it is run as one approved window.

## 1e. Firewall baseline gate (after nftables is installed)

With `nftables` installed the owner can apply its reviewed baseline ruleset.
The gate applies `baseline`, requires the confirmation and the active state, and
restores the mode it found. It writes
`evidence/private/firewall-gate-<date>.json` (probe
`hardened-blikvm-firewall-gate`, `result=passed`):

```
project-memory-exec --project BLi-KVM --secret ssh=<id>:ssh_password -- \
python3 -B tools/stage_device_firewall_gate.py \
  --endpoint ssh://<host>:22 --username <user> \
  --known-hosts evidence/private/ssh-known-hosts \
  --base-url https://<host> --api-username admin \
  --ca-file evidence/private/<host>.crt \
  --confirm --output evidence/private/firewall-gate-<date>.json
```

Expected facts: `backendAvailable=true`, `nftInstalled=true`, `applied=true`,
`confirmed=true` and the mode restored to the value it had before the run.

## 1f. Panel status gate (ST7789)

Proves the daemon paints a defined frame: it reads the panel state, draws the
fixed test pattern, restarts `hardened-blikvm-boardd`, and requires the daemon
to re-initialize and light the panel by itself. It writes
`evidence/private/panel-status-gate-<date>.json` (probe
`hardened-blikvm-panel-status-gate`, `result=passed`):

```
project-memory-exec --project BLi-KVM --secret ssh=<id>:ssh_password -- \
python3 -B tools/stage_device_panel_gate.py \
  --endpoint ssh://<host>:22 --username <user> \
  --known-hosts evidence/private/ssh-known-hosts \
  --confirm --output evidence/private/panel-status-gate-<date>.json
```

Expected facts: `initializedBefore=true`, `testPatternCode=0`,
`daemonRestarted=true`, `initializedAfter=true`, `backlightAfter=true`. The
operator confirms the visible image.

## 2. HID keyboard and mouse modes (safe target only)

Prerequisite: a controlled target and its video visible to the operator.

- Keyboard: press and release each accepted key group from the aligned GUI set,
  confirm the expected characters on the target, hold modifiers, and check
  six-key rollover plus release-before-press ordering.
- Mouse: verify absolute tracking, relative motion, both button states and
  wheel in both directions.
- Run a 10,000-event soak with the target under observation and confirm no
  stuck key or button after stopping.
- Force a backend outage, confirm release-all, then confirm reconnect
  recovery leaves no held input.

Record the receipt with the target's outcome and stop on any stuck input.

## 3. BliKVM reboot (requires explicit reboot confirmation)

The device reboot gate is separate from every other step and is never run
implicitly.

- Confirm the retained state and a healthy baseline before reboot.
- Request exactly one software reboot.
- After boot, poll the read-only verified snapshot until the boot identity
  changes and the retained state is still confirmed.
- Record a private receipt; do not repeat the reboot to "make it pass".
- If the device does not return, a manual power cycle is the recovery; then run
  `tools/stage_device_hid_reboot_gate.py --verify-only` to record the
  post-reboot state without another reboot. The journal is now persistent on
  the `/mnt` partition, so the next boot can be post-mortemed with
  `journalctl -b -1`.

## 4. Virtual media (safe target only)

- Attach/eject a catalogued image through the approved GUI path and confirm the
  target observes the media change.
- Interrupt an attach, then reconnect: the Pending journal must block other
  mutations and require explicit reconciliation.
- Boot the target from the attached image (BIOS boot) and confirm read-only vs
  writable modes and storage-exhaustion behaviour.
- Force a KVM reboot during a pending operation and confirm reconciliation by
  read-only MSD observation, never automatic retry.

## 5. Phase 4 video

- Hotplug and resolution changes: confirm no-signal and recovery on both the
  browser and a second concurrent client.
- Two-client: confirm both viewers stream independently without interference.
- Latency and CPU/memory: measure a bounded window, then run the 24-hour soak
  on the test device and archive the receipts.

## Reporting

Each executed step records a private receipt and updates the checkpoint in
`docs/baselines/phase3-hid-atx-host-checkpoint.md`; anything not executed stays
listed as remaining. Device gates are never marked passed from host tests
alone.

## 6. Board owner (KVMD-free) handoff

The Rust board owner replaces KVMD for the composite gadget and the ATX lines.
This section is the physical gate; every step needs explicit operator
confirmation and produces a private receipt.

A plan-only pre-flight (no `--confirm-handoff`) against the current bundle
records the exact owner environment, steps and acceptance flags without touching
the device; the latest is
`evidence/private/board-handoff-plan-20260915-c.json` (bundle `39991edf…`, media
and ATX-bench acceptance enabled).

First run `tools/capture_device_handoff_preflight.py` and require
`handoffReady=true`: it is the read-only check that the target really is the
BliKVM H616 stack (KVMD and the vendor daemon present, a gadget and UDC bound,
configfs mounted) and not a substitute host.

### 6a. Read-only capture (no mutation)

- Confirm the stock gadget is up and capture its exact HID function parameters
  and report descriptors (`hid.usb0/1/2`: `protocol`, `subclass`,
  `report_length`, `report_desc`) into a private receipt with
  `tools/capture_device_hid_descriptors.py`. Those values pin the gadget spec;
  they are protocol facts and must not be guessed.
- Capture the reviewed ATX pulse timings (power short, power long, reset) and
  the line identity/polarity from the stock behaviour on the bench, with the
  target's Power and Reset physically disconnected.
- Re-run `tools/capture_device_kvm_topology.py` and store the receipt.

### 6b. Ownership handoff

- Build the board-owner test bundle with `scripts/build-board-profile.sh`
  (features: restricted-control-surface, gadget-media-profile,
  gadget-hid-profile, chardev-atx-profile) and verify it with
  `tools/verify_board_profile_bundle.py` (members, MANIFEST profile/features,
  SHA256SUMS) and plan the exact owner environment and steps with
  `tools/plan_board_profile_handoff.py` (validates the timings and every device
  path against the reviewed allowlist without touching the device). Fill in the
  operator `atx.env.example` with the confirmed timings and install it under
  `/run` for the reversible gate; the retained slots and permanent write policy
  stay untouched.
- Copy the verified bundle to `/var/lib/hardened-blikvm-gadget/gate/bundle.tar.gz`,
  then run the reversible gate. The reviewed copy-paste form for the full
  acceptance window (media/HID plus the native ATX bench) is:

  ```
  project-memory-exec --project BLi-KVM --secret ssh=<record>:ssh_password -- \
  python3 -B tools/stage_device_board_profile_gate.py \
    --endpoint ssh://<host>:22/ --username <user> \
    --known-hosts evidence/private/ssh-known-hosts \
    --bundle /mnt/d/debug/blikvm/hardened-blikvm/bundles/hardened-blikvm-board-profile-aarch64.tar.gz \
    --output evidence/private/board-handoff-gate-<date>.json \
    --udc musb-hdrc.2.auto --confirm-handoff --takeover-udc \
    --accept-media-hid --accept-timeout-seconds 300 --accept-image kubuntu.img \
    --accept-atx-bench --confirm-physical-disconnect
  ```

  Run the GUI probes from section 6c while it waits, then run the same command
  with `--teardown` and a fresh output path.

- The gate itself is then:`
  `tools/stage_device_board_profile_gate.py --confirm-handoff` (stops KVMD,
  starts the Rust board owner, verifies the owner socket and the `hardened`
  gadget, and never touches the `kvmd` gadget). Without `--confirm-handoff` it
  only validates and prints the plan; `--teardown` restores KVMD. Add
  `--accept-media-hid --accept-timeout-seconds 300 --accept-image kubuntu.img`
  to make the same run wait for the composite gadget the owner creates on the
  first media attach, then record its functions, the attached catalog image and
  the hidg devices, and write one neutral (release-all) keyboard report. While
  the gate waits, drive the reviewed GUI actions (attach `kubuntu.img`, press a
  key, move the pointer) and confirm the target sees them; the gate fails closed
  if any recorded fact is missing or unexpected.
- The owner resolves a catalog entry to `<images dir>/<name>`, and that
  directory is empty on a fresh board while the reviewed image lives in KVMD's
  MSD directory. Materialize the verified link (never a copy) with
  `tools/stage_device_owner_image.py --image kubuntu.img --source kubuntu.img
  --expected-sha256 <catalog sha> --expected-bytes <catalog size>`; the tool
  hashes the source on the board, refuses any mismatch, and records the receipt.
  Undo with `rm -f /var/lib/hardened-blikvm-gadget/images/kubuntu.img`.
- The board has a single UDC and KVMD holds it, so the owner cannot bind its
  gadget without it: `--takeover-udc` unbinds the `kvmd` gadget right after the
  KVMD stop, stops `kvmd-otg`/`kvmd-main` (the vendor OTG unit removes its gadget
  with it), then requires that no gadget directory binds the UDC (`udc_bound=none`)
  and records `kvmd_udc_unbound`; the kernel's per-UDC state file is stale on this
  board and is not trusted. Teardown
  restarts all three units, waits for and requires `kvmd_udc_bound=yes`, so KVMD
  always owns its gadget again afterwards.
- For the native ATX bench add `--accept-atx-bench --confirm-physical-disconnect`
  to the same gate run: it executes the bundled
  `bli-kvm-atx-gpio-bench-gate` (argument-free, `BLI_KVM_ATX_BENCH_CONFIRM=1`)
  against the real GPIO character device with the reviewed timings and records
  the measured hold per action plus a within-tolerance verdict; any drift fails
  the gate and rolls back. Power and Reset must be unplugged for this step.
- Bring up the board owner unit with the operator `EnvironmentFile` holding the
  confirmed timings; verify each backend independently before enabling the
  capability in the shipped build:
  - media: select, attach, eject and host re-enumeration; the server may also
    accept an operator upload when `BLI_KVM_MEDIA_DIRECTORY` points at the
    owner's image directory — the owner re-reads the catalog before each applied
    operation, so the upload is selectable immediately. The GUI keeps its upload
    control behind the hidden remote-download capability, so use
    `tools/upload_media_image.py --base-url <stand> --username <operator>
    --file <image.img> --output evidence/private/…` with the password on stdin;
    it posts `POST /api/download/file`, streams one bounded `.img`/`.iso`, and
    records the file name, size, SHA-256 and the returned catalog ID;
  - HID: keyboard report, rollback on write failure, ledger replay after
    restart;
  - ATX: `power_short`, `power_long`, `reset` timing and cooldown, with a
    rollback to the KVMD route ready.
- Record the before/after topology and gadget-state receipts.

### 6c. Acceptance

- Target boots and responds through the Rust gadget; media mounts and boots;
  keyboard and mouse work on the real target; a reboot and a stuck-input
  recovery pass.
- Automated media actions during the `--accept-media-hid` window: run
  `tools/playwright/virtual_media_acceptance.mjs` (module path, Chromium path,
  stand URL, username file, output under `evidence/private`, image name; the
  password arrives on stdin). It logs in through the GUI, opens the image menu,
  attaches the catalog image and ejects it again, and records every
  `/api/virtual-media` response, so the attach/eject half of the acceptance is
  reproducible instead of hand-clicked. `restricted_control_virtual_keyboard_probe.mjs`
  covers the HID half the same way, and `restricted_power_probe.mjs` covers the
  power half (menu entry, explicit confirmation, focus challenge, ATX request).
  All three read the wrapped `{code,data}` response the server returns and were
  validated against the built GUI with `tools/playwright/app_harness.mjs`; the
  power probe needs Power and Reset unplugged like the bench.
- Only after these pass, enable the capability flags in the shipped build and
  remove `kvmd.service` from the board profile.

## 7. Trust rotation after KVMD retirement (E4)

The retained supervisor reads its trust anchor from
`/etc/hardened-blikvm/update-supervisor.json` through a systemd credential
(`update-supervisor-config`). The reviewed `stage_device_update_trust_config.py`
tool validates and writes that file but also renders the nginx/web cutover and
requires a cutover baseline with `kvmd_active=active`, which no longer holds on
this board, so it cannot be used after retirement.

Swapping the single `keyId`/`publicKeyHex` to a new key was checked on the
device on 2026-09-19 and is **not** sufficient by itself: every retained slot
(`app-...-03` .. `app-...-11`) is signed by the previous key, so the supervisor
rejects the existing state (`GET /api/application-update/status` returns 503 and
an upload answers `archive verification failed`). The change was rolled back.

**Re-signing the retained slots is not viable**: `bli-kvm-update-verifier`
requires `manifest.keyId == trust_root.keyId`, and the manifest `keyId` is part
of the signed bytes, so changing it would change the manifest SHA and therefore
the slot id (`app-<seq>-<manifest-sha>`) and the state that names it. The only
reviewed path left is:

1. **Re-initialise the application state under the new key**
   (`tools/initialize_application_state.py`, the first-signed-slot
   orchestrator). This provisions a fresh state with the new trust anchor and
   discards the retained slot history (floor/generation reset). It needs the
   operator's explicit approval because the retained slots are removed.

A **non-destructive** rotation was performed on 2026-09-19 that keeps the old
key, config and roots intact: a second config with the new key and fresh roots
(`/var/lib/hardened-blikvm-{slots,update}-v2`, `/run/hardened-blikvm-update-v2`,
owner `hardened-blikvm-update`) was validated, the seq13 bundle was staged and
the fresh state initialised (floor 13, generation 1). The live
`/etc/hardened-blikvm/update-supervisor.json` (root-owned, owner `0` is the only
trusted config owner in release) now points at the new key and fresh roots, and
the authenticated status surface reports the seq13 state
(`evidence/private/update-trust-rotation-20260919-a.json`). The previous config
is backed up and the previous roots are untouched, so the rotation is
reversible.

The slot-serving cutover was then performed on 2026-09-19. The running board
server executes the confirmed slot through a reviewed, reversible drop-in
(`/etc/systemd/system/hardened-blikvm-board-server.service.d/90-slot-serving.conf`)
that resets `ExecStart` to `<slotRoot>/current/bin/bli-kvm-server` and points
`BLI_KVM_WEB_ROOT` at the slot. The gate
`tools/stage_device_slot_serving_gate.py` refuses a slot whose binary or web
entry differs from the installed artifact, or a `current` that is not the
confirmed slot; `--rollback` removes the drop-in and restores the installed
server. It recorded `evidence/private/update-slot-serving-20260919-a.json`
(`result=passed`). To undo: run the gate with `--rollback`.

The slot root mode matters: the shipped design uses `drwxr-x---` (0750, group
`hardened-blikvm`) so the serving user can execute the slot artifact. The
rotation script created the fresh `-v2` slot root with 0700, which made the
server fail with `203/EXEC` (`Permission denied`); the mode was corrected to
0750. The update root stays 0700.

Note the ownership rule that the direct CLI exposes: a release supervisor
trusts only a root-owned config, while the stager requires the slot root owned
by the effective user, so a root CLI needs root-owned roots and the socket
services (user `hardened-blikvm-update`) need those roots owned by that user.
