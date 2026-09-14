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
  `e8f45c7c7d58df3392936cdf09d8a345525914fbfa8bf18557d67b1a442953c3`,
  profile `restricted-control-test`, source commit
  `ccbb3fe`. Earlier bundles `72270c8f` (pre-Phase-3 slot web), `30c23c87`
  (queued-report confirmation loop) and `d3cba1bc` (per-surface HID lease)
  are superseded.
- Contents: feature-built `bli-kvm-server` and `bli-kvm-control-broker`, the
  operator web assets built from the current tree, the
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
(`restricted-control-profile-gate-20260913-d.json`): `extracted=yes`,
`broker_started=yes`, `control_socket_present=yes`, `server_started=yes`,
`health=ok`, `hid_availability=read_write`, `power_availability=read_write`,
`https_ready=yes`, `persistentChangeScope=runtime-only`.

The operator web assets are served from the bundle itself; serving the retained
slot web by mistake leaves the restricted HID/power GUI absent even while the
server advertises `read_write`.

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

## Restricted HID verification and fixes (2026-09-13)

Live verification against the running test profile:

- The KVMD internal socket accepts the enrolled credential and the fixed HID
  mutation routes directly (`send_key` and `send_mouse_relative` both return
  `{"ok":true}`), so the socket, credential and KVMD routes are not the fault.
- `tools/probe_restricted_control.py` drives login, focus attestation, lease
  acquire, a real `KeyA` press and release, and release through the server:
  every step returns `applied` with no rejection.
- `tools/playwright/restricted_control_gui_probe.mjs` drives the same flow
  through the operator UI (real keystroke plus the confirmation modal) and
  observes `GET /api/hid/focus 200`, `POST /api/hid/focus 200` and three
  `POST /api/hid/lease 200` responses.

Fixes delivered while diagnosing the operator report:

- the focus confirmation is shared across queued reports, so confirming once no
  longer reopens the modal for every buffered event;
- the keyboard, mouse and mobile trackpad share one broker lease and one
  ordered queue, because the broker admits a single active HID lease and
  per-surface leases conflicted with `lease_conflict`;
- the profile bundle now carries the operator web assets from the same source
  commit and the harness fails closed when the restricted GUI markers are
  absent;
- the test proxy sends `Cache-Control: no-store` so a stale bundle cannot
  silently persist in the operator browser.

Boundary found during debugging: a bare broker restart leaves the runtime
control socket behind because the harness state directory is not a systemd
`RuntimeDirectory`, so the next start failed with `SocketAlreadyExists`. The
harness now removes the socket and clears the failed unit before starting the
broker. The production units use `RuntimeDirectory`, which systemd removes on
stop.

## ATX/LED provenance and operator input fixes (2026-09-13)

- The ATX pinout is the original BliKVM v4 vendor configuration already on the
  device, not a woffko/Bli-PiKVM mapping. `/etc/kvmd/override.yaml` sets
  `gpiochip0` with `power_switch_pin=228`, `reset_switch_pin=272`,
  `power_led_pin=234` and `hdd_led_pin=233`, and the ATX actions call the vendor
  `/usr/bin/atx --v v4 --c power_on|power_off|power_reset` (the binary also
  exposes `led_power`/`led_hdd`). `gpioinfo` labels those lines
  `kvmd::atx::power_switch`, `kvmd::atx::reset_switch` and `kvmd::atx::leds`.
  `docs/baselines/upstream.md` pins `blikvm/blikvm` `v2.2.3-alpha` as the
  hardware reference and uses `woffko/Bli-PiKVM` only for the KVMD-on-H616
  procedure and MSD compatibility.
- The hardened layer never drives GPIO: power and reset use KVMD's fixed
  `/atx/click?button=...&wait=1` routes, and the read-only LED state comes from
  KVMD `GET /atx`. Observed: `{"enabled":true,"busy":false,
  "leds":{"power":false,"hdd":false}}`. A physical mismatch (different board
  revision, ribbon orientation, or unconnected target front-panel LED) is not
  detectable from the control path.
- The operator power menu read the legacy `/api/vm/gpio` for the power LED,
  which the hardened server does not expose; it now reads `powerLed` from
  `/api/inventory`.
- The on-screen virtual keyboard and the mobile soft keyboard sent input over
  the legacy websocket or `/api/hid/paste`, which the hardened server does not
  expose, so mobile input never reached the broker. Both now use the shared HID
  lease when the restricted capability is advertised; the mobile soft keyboard
  expands bounded US-layout ASCII to key reports and the on-screen keyboard
  sends the physical key codes for its language layouts.
- Mouse movement is coalesced to the latest report per flush so a burst of
  pointer events cannot build an HTTP backlog.

## Operator acceptance on the physical target (2026-09-13)

The operator connected a real target and confirmed keyboard input, mouse input
and ATX power/reset control through the restricted-control test profile. This
completes the HID item-5 real-target keyboard and mouse modes and the ATX
physical control check for the initial code; the isolated gates already covered
rollover, the 10,000-event soak, reconnect and stuck-input recovery.

Minor items deferred by the operator: LED presentation and other cosmetic
gaps; the ATX pinout remains the original BliKVM v4 vendor configuration
(`gpiochip0` 228/272/234/233, vendor `/usr/bin/atx --v v4`).

The capability decision for a shipped build is still open: the restricted
control surface remains compile-time disabled in shipped builds and is enabled
only in this test profile.

## Combined HID + ATX + media test profile (2026-09-14)

The restricted-control test profile now serves HID, ATX and virtual media from
one broker state through `run_with_restricted_profiles` (three independent
credential instances). The bundle enables `media-transition-profile`; the
harness creates a sample opaque-ID catalog and sets the media env on the broker
and server units. On the deployed device:

- `capabilities.virtualMedia` = `read_write`;
- `GET /api/virtual-media/catalog` returned the sample entries, and the harness
  can now deploy a real catalog with `--media-catalog`; the stand currently
  lists the board's `kubuntu.img` (CD-ROM and removable);
- connect/disconnect (`set_connected`) routes to KVMD; a `select` for a sample
  name that is not present in KVMD's MSD is expected to be rejected, so real
  attach still needs a catalog built from the board's actual images.

Receipts `restricted-control-profile-gate-20260913-i.json` (sample catalog) and
`restricted-control-profile-gate-20260914-j.json` (real catalog); bundle
`7e749916acafa06a9b5868b39a5fe39b8e8b467f0682b3de6c7f5eae643d2de6`. Real
`select`/mount for `kubuntu.img` is the next mutation gate and is not performed
without explicit confirmation.

## Real virtual-media attach/eject gate (2026-09-14)

Root cause of the earlier `backend_rejected`/`Virtual media command failed`:
`VirtualMediaCommand` serialized `Select` as `image_id` while the GUI sends
`imageId`, so the server rejected `select` with HTTP 422 before the broker.
Fix `8037129` sets `rename_all_fields = "camelCase"` and the protocol round-trip
test asserts `imageId` on the wire.

After rebuilding the profile (`543bc9fa...`) and redeploying with the real
catalog, the real gate passed: `select` CD-ROM (`/msd/set_params?image=kubuntu.img&cdrom=1&rw=0`
HTTP 200), `set_connected true` (HTTP 200, KVMD drive `connected=true`,
`image=kubuntu.img`), then `set_connected false` (HTTP 200, `connected=false`).
An attach issued immediately after select is deterministically deferred with
`cooldown_active`; a later attach applies. Receipt
`restricted-control-media-gate-20260914-a.json`; runtime-only scope.
