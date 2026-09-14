# HID transition boundary

The HID transition source exists to replace the GUI's raw report assumptions
with a bounded, recoverable adapter. It is not connected to the control broker,
network server, or GUI, and the H616 HID capability remains unavailable.

## Report translation

The adapter accepts only the fixed keyboard, relative-mouse, and
absolute-mouse report shapes from `control-v1.md`. Its key table is pinned to
the `KEYMAP` observed in the KVMD 3.291 field image. Unknown USB usages and the
unsupported Back/Forward mouse buttons fail closed.

Translation is stateful:

- key and button releases are emitted before new presses;
- unchanged reports emit no event;
- absolute 0..32767 coordinates map to the complete KVMD
  -32768..32767 range;
- invalid or unsupported reports revoke the lease and emit release-all for
  every previously held key and button;
- only the five fixed internal KVMD routes are representable:
  `send_key`, `send_mouse_button`, `send_mouse_move`,
  `send_mouse_relative`, and `send_mouse_wheel`.

There is no generic URI, key string, query name, device path, or passthrough
body. Each KVMD response must be bounded JSON with `ok=true`.

## Lease boundary

A report requires an exact lease identity containing a non-zero 128-bit lease
ID, a non-zero session binding, and a non-zero 256-bit target binding. The
lease is exclusive and lasts at most 30 seconds. Renewal requires the exact
same identity. Expiry, session revocation, disconnect, identity mismatch,
translation failure, or backend failure cannot transfer a lease to another
session or target.

The future server must derive the session and target bindings from an explicit
safe-target/focus confirmation flow. Caller-supplied values are not an
attestation and cannot enable the backend.

## Release and crash recovery

Before a held-state transition, the engine atomically writes and fsyncs the
union of the old and new release sets to a private JSON ledger. After the
backend batch succeeds, it atomically stores the exact new release set. Thus a
crash at any point leaves a safe superset of key/button releases.

On restart, the ledger becomes a mandatory pending-release queue. No new lease
is granted until every release succeeds and an empty ledger is atomically
persisted. Backend failures revoke the active lease, attempt release-all
immediately, and retain any unsuccessful releases for the next recovery.
Relative, symlinked, group/world-writable, oversized, unknown-schema, and
non-release ledger content is rejected.

## Remaining gates

Before HID can be wired or advertised:

1. add a control protocol revision carrying lease acquire/renew/report/release
   messages plus a broker-side target attestation verifier — provided by
   `control-v2.md`, the `bli-kvm-control-broker` attestation registry, and the
   `bli-kvm-control-focus` binding derivation, all still unwired;
2. provision a dedicated transition credential and sandbox without sharing it
   with the network server — the fixed-path loader, the dedicated
   `hardened-blikvm-hid-credential.service` sandbox and a reversible H616
   delivery gate exist, and the gate passed on the test device without leaving
   an account, unit or staging path behind;
3. test startup-ledger replay and disconnect release on H616 without injecting
   into an uncontrolled external host;
4. establish a safe visible target/focus and then run keyboard/mouse mode,
   rollover, 10,000-event, reconnect, reboot, and stuck-input recovery gates;
5. align the GUI key/button set to the adapter's accepted table before changing
   the HID capability.

## Backend seam

The HID transition engine now writes report batches through a backend seam
(`HidExecutor`) so the KVMD executor used during migration can later be
replaced by the Rust USB gadget writer without changing lease policy or broker
wiring. The KVMD constructor and the fixed key routes are preserved, and a test
exercises the engine through an injected backend.

## Gadget path

`HidReportTranslator` maps fixed USB reports to typed events for KVMD.
`KeyboardReportComposer` is its inverse for the USB boot keyboard interface: it
composes the standard 8-byte boot report (modifier byte, reserved byte, six
keycodes) from lease events, tracking held keys and modifiers, and refuses a
seventh simultaneous key. The layout is the USB HID boot protocol; mouse
composition waits on the device report descriptors.

The feature-gated `gadget-hid-profile` adds `HidExecutor::Gadget`: it composes
boot keyboard reports from lease events and writes them to `hidg0`. Pointer
events fail closed until the device report descriptors are captured. It is
compiled out of shipped builds and covered by host tests.

The privileged broker exposes a feature-gated `run_with_gadget_hid_profile`
that serves HID from the Rust gadget (no KVMD): it composes boot keyboard
reports and writes them to the function device. A host test drives target
attestation, lease acquisition and a key report through the control socket and
asserts the report reaches `hidg0`; pointer events fail closed.
