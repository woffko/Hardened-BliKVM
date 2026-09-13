# Phase 3 restricted-control test profile (HID + ATX)

Status: test profile deployed on the enrolled H616 device for manual operator
testing on 2026-09-13. It is not a shipped or release-accepted artifact:
`releaseAccepted` and `deviceAccepted` are both false, and the shipped
prototype and application bundles keep the restricted control surface compiled
out.

## Build inputs

- Features: `restricted-control-surface`, `hid-transition-profile`,
  `atx-transition-profile`.
- Bundle:
  `bundles/hardened-blikvm-restricted-control-profile-aarch64.tar.gz`, SHA-256
  `72270c8f16859bebf2f919d9244b63d443dbac859662f278f853ac7b6fc43ce2`,
  profile `restricted-control-test`, source commit
  `0af9717348d1e451382846deea79443d6451c517`.
- Contents: feature-built `bli-kvm-server` and `bli-kvm-control-broker`, the
  `hardened-blikvm-control-transition.service` unit, its tmpfiles entry and the
  `hardened-blikvm-server-current.service.d/90-restricted-control.conf` drop-in.
- Build entry point: `scripts/build-restricted-control-profile.sh`; host test
  `tools/test_build_restricted_control_profile.py`.

## Host-side coverage

- The broker serves HID and ATX from one state through
  `run_with_control_profile`; the combined end-to-end test proves an attested
  HID lease grant and a live ATX route against one broker.
- `BLI_KVM_HID_PROFILE` and `BLI_KVM_ATX_PROFILE` selection is covered by
  `profile.rs` tests; the server opt-in `BLI_KVM_RESTRICTED_CONTROLS` is gated by
  the `restricted-control-surface` feature and a configured control socket.
- `docs/api/control-v2.md` records the test-profile environment contract.

## Reversible device deployment

`tools/stage_device_restricted_control_profile.py` (host test
`tools/test_stage_device_restricted_control_profile.py`):

1. uploads the bundle and stages the binaries under a removable
   `/tmp/hardened-blikvm-rc-<sha16>` runtime root;
2. writes transient units into `/run/systemd/system/` for the combined broker,
   the feature server on `127.0.0.1:18082` and a temporary HTTPS proxy on
   `8443`, then removes them on teardown;
3. delivers the KVMD credential to the broker only through systemd
   `LoadCredential` from `/etc/hardened-blikvm/kvmd-*` and never writes it;
4. copies the read-only account file into the runtime state directory so the
   existing operator login works;
5. probes `GET /api/capabilities` and requires HID and power `read_write`.

The harness never touches the retained application slots, the retained guard,
the permanent write policy, the evidence store or the live 443 GUI. It runs as
the existing `hardened-blikvm-control` account with `SupplementaryGroups=kvmd`
instead of creating the dedicated production `hardened-blikvm-hid` account;
this is a deliberate test-harness deviation, while the credential still arrives
through `LoadCredential`.

Observed receipt facts
(`restricted-control-profile-gate-20260913-a.json`): `extracted=yes`,
`broker_started=yes`, `control_socket_present=yes`, `server_started=yes`,
`health=ok`, `hid_availability=read_write`, `power_availability=read_write`,
`https_ready=yes`, `persistentChangeScope=runtime-only`.

## Manual acceptance checklist (operator)

1. Open `https://<device>:8443`, accept the self-signed certificate and log in
   with the existing account.
2. Confirm the operator UI exposes keyboard/mouse and Power/Reset from the
   `read_write` capabilities.
3. HID: focus the target, send keys and mouse events, and confirm them on a
   safe visible target; confirm the lease expires and releases while idle.
4. Power/Reset: Power and Reset are physically disconnected, so an `applied`
   result or the five-second / thirty-second cooldown is the expected
   behaviour; confirm the UI does not claim a completion while indeterminate.
5. Virtual media is not part of this profile: the Image path stays the legacy
   KVMD passthrough.

## Boundaries

- The capability decision for a shipped build remains open; this profile proves
  the control path only.
- Reconciliation input for the ATX backend is operator-reviewed: the optional
  `BLI_KVM_ATX_RECONCILIATIONS` file supplies `applied|rejected` plus `on|off`
  LED evidence, and a pending operation is reported `pending_unreconciled`
  until reviewed. An automated read-only LED/state source is future work.
- Manual HID injection requires a safe, visible, authorized target; the
  operator watches the video while issuing events.
- The runtime units and `/tmp` root are transient; a device reboot removes
  them.
