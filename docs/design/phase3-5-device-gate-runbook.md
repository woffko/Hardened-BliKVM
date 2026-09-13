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
