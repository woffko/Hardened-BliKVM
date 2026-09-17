# Bounded network writes (DNS and IPv6), v1

Status: DNS and IPv6 implemented on the host, device gate staged; firewall,
SSH/mDNS, Wake-on-LAN and wifi are the remaining items of phase E3.

## Why a separate design

The reference GUI changes the resolver configuration and disables IPv6 per
interface. Our server runs unprivileged with `ProtectSystem=strict`, so it
cannot write `/etc/resolv.conf` or the sysctl tree. The write therefore travels
the same reviewed path as the privileged system actions: the unprivileged
server validates and forwards, and the root board owner performs exactly one
bounded write and reports what the device shows afterwards.

## Contract

One request carries a single target:

```json
{"type": "network_write", "request": {
  "target": {"kind": "dns", "servers": ["1.1.1.1"]},
  "sessionBinding": 123, "confirmationBinding": "…32 bytes…", "issuedAtUnixMs": 0
}}
```

```json
{"type": "network_write", "request": {
  "target": {"kind": "ipv6", "interface": "wlan0", "disabled": true},
  "sessionBinding": 123, "confirmationBinding": "…32 bytes…", "issuedAtUnixMs": 0
}}
```

Validation happens on both sides and never trusts the other:

- Bindings: the session binding and a per-target confirmation binding are
  derived from the authenticated session token (domains
  `hardened-blikvm-network-write-v1` and `-op-v1`), so a request cannot be
  replayed for another session or another target, and the operation id is
  unique per call.
- Resolver content: one to three entries, each a literal IP address
  (`nameserver` lines are rendered by the owner), no comments, no whitespace,
  bounded line and file sizes. Anything else is refused before any write.
- Interface names: the kernel's own charset, at most fifteen characters.

Writes are applied once and read back; the caller reports the read-back, never
the intent.

- Resolver: the file is replaced through a sibling temporary file and a rename,
  and a symlinked or non-regular target is refused, so the reviewed path cannot
  be redirected.
- IPv6: exactly one interface's `disable_ipv6` node is written.

Outcomes map to the fixed control responses: `applied` (with read-back),
`backend_rejected` for a refused write, `capability_disabled` when the profile
is not armed or no writer is installed.

## Gating

- The capability manifest carries a `network` section; `dns` and `ipv6` become
  `read_write` only when the restricted control surface, a control socket and
  `BLI_KVM_NETWORK_WRITES=1` are all present.
- `POST /api/network/dns` and `POST /api/network/ipv6` require an authenticated
  session and an explicit `confirm`; a missing confirmation is refused before
  any IPC.
- The owner compiles the surface only with the `network-write-profile` feature,
  which the shipped board profile carries.

## Device gate

`tools/stage_device_network_write_gate.py` is plan-only by default. With
`--confirm-write` it performs the reviewed write and immediately restores the
observed value, so the device ends where it started, and passes only when both
writes and both restores applied:

- resolver: read the current `nameserver` lines, write the reviewed resolver,
  read it back, restore the observed list;
- IPv6: read one interface's `disable_ipv6`, flip it, read it back, restore the
  observed value.

The receipt records the preflight facts, the plan and the four booleans
(`dns_applied`, `dns_restored`, `ipv6_applied`, `ipv6_restored`).

## Remaining items

Firewall, SSH/mDNS, Wake-on-LAN and wifi follow the same shape: validate a
bounded target in the owner, forward from a capability-gated server route, and
gate with a write-and-restore probe. Interactive wifi stays out of scope until a
reviewed helper exists, and every item is limited to what the hardware supports.
