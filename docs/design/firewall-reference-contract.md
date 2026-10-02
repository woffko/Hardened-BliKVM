# Firewall reference contract and native port gates

## Pinned source evidence

The behavioral reference is `Hardened_NanoKVM` commit
`ff7f42c9605e232056ac2ebe90a70dab4c817245`, specifically:

- `server-rust/src/api/system_firewall.rs`
- `kvmapp/system/init.d/S40firewall`

The local reference working tree was at `5b8ed3a62a4deadc5f6ee8a1573f8935111135e7`
when inspected; read these files with `git show` at the pinned commit. Do not
assume that inspecting the working tree inspects the requested reference.
BLi-KVM uses the native board owner and vendor hardware implementation, not KVMD.

## Observable mode matrix

All restrictive modes permit loopback, established/related traffic and DHCP.
IPv6 additionally permits ICMPv6 errors, router discovery and neighbor discovery.
Local administration ranges are IPv4 RFC1918 plus `169.254.0.0/16`, and IPv6
`fc00::/7` plus `fe80::/10`. These are reference policies, not observed device
addresses or an inference about the physical interface name.

| Mode | Input | Forward | Output | Preconditions and application effects |
| --- | --- | --- | --- | --- |
| baseline | Accept by default | Accept | Accept | Reference IPv4 adds an established TCP source-port-8000 drop on `eth0`; assess its purpose against BLi's actual front/backend topology before porting. |
| moderate (reference default) | New local-source traffic accepted; other new input dropped | Drop | Accept | Same reference IPv4 port-8000 exception; no HTTPS prerequisite. |
| restricted | Local-source HTTPS and SSH; other new input dropped | Drop | New TCP 443, UDP/TCP DNS 53, UDP NTP 123, configured UDP syslog port, DHCP and IPv6 control | HTTPS enabled and health checked; WebRTC disabled; 60-second explicit-confirmation rollback. |
| paranoid | Local-source HTTPS only; other new input dropped | Drop | Established/related, loopback, DHCP and IPv6 control only | HTTPS enabled and health checked; WebRTC and online updates disabled; 60-second explicit-confirmation rollback. |

DHCP tuples are inbound UDP 67→68 / outbound 68→67 for IPv4 and inbound
547→546 / outbound 546→547 for IPv6. ICMPv6 types are destination-unreachable,
packet-too-big, time-exceeded, parameter-problem, router-solicitation,
router-advertisement, neighbor-solicitation and neighbor-advertisement.

The reference is not explicitly Tailscale-aware: it does not include CGNAT
`100.64.0.0/10` in local IPv4 ranges or a UDP peer-traversal exception. Its
effect on real Tailscale connectivity must be tested and resolved explicitly;
do not claim Tailscale preservation from these rules alone.

## Native adaptation boundaries

- Do not flush or replay unrelated host tables. Use the existing exclusive
  owned-table model, including owner-comment validation and handle-scoped delete.
- The existing native `baseline` toggle has different semantics from reference
  baseline. Do not silently relabel it as reference mode parity. Version the
  protocol/state transition and migrate the UI/API together.
- Obtain HTTPS and syslog ports from the native configuration owner. Do not
  copy the reference `eth0`, fixed local health URL or insecure TLS-check flags.
- A timer that checks only the existence of a shared pending file can roll back
  a newer operation. Bind pending confirmation and timeout to the exact operation
  and previous owned-table state, and define restart handling before enabling it.
- Error paths must retain recovery state and expose uncertainty; a successful
  helper exit or ownership marker alone does not prove the intended rules.
- Actual rule content, not merely table presence, must drive mode read-back.
- Reference shell commands that suppress errors or reset global filter policies
  are not safe implementation templates for the native owner.

## Device deployment (plan phase D)

- Activation is one operator-supplied file, `/etc/hardened-blikvm/firewall.env`
  (root, directory 0700), read by the board owner and both native server
  units through `EnvironmentFile=-`: `BLI_KVM_FIREWALL_MODE_CONFIG` names the
  reviewed mode configuration and `BLI_KVM_FIREWALL_MODES=1` enables the API.
  Without the file the modes stay off; an unusable configuration is logged
  and also leaves them off instead of stopping the owner.
- The mode configuration (`/etc/hardened-blikvm/firewall-mode.json`) holds the
  HTTPS port and probe (server name and the device certificate as its CA),
  the syslog port and the local-only WebRTC media range.
- The pending journal and frozen configuration live in
  `/run/hardened-blikvm-firewall` (0700, created by the operator's tmpfiles
  entry `packaging/tmpfiles.d/hardened-blikvm-firewall.conf`; the owner unit
  cannot create it with a start hook, because the update supervisor refuses
  application units with one): they
  survive an owner restart but not a reboot, and neither do the kernel rules.
  The reference re-applies its saved mode at boot; the owner therefore keeps
  the last confirmed mode in `/var/lib/hardened-blikvm-gadget/firewall-mode.json`
  (a mode needing no confirmation is kept at once, a restrictive one when the
  operator confirms it) and, on start with no owned table and no pending
  change, re-applies it as a journaled change without the HTTPS preflight
  (the front may still be starting; the mode was confirmed with it running).
  A failed restore rolls back to the open baseline.

## Settings page

With the four modes enabled the server answers the reference page's own
contract: `GET /api/system/firewall` (mode flags, backend, HTTPS port and the
IPv4, IPv6 and nft rule listings, which the owner serves in chunks of at
most 2 KiB because one control message is limited to 4 KiB),
`POST /api/system/firewall {mode}` and `POST /api/system/firewall/confirm`.
The web shows the reference page unchanged; it confirms a restrictive
change itself right after applying it, which proves that the page still
reaches the board through the new rules, and the owner's 60-second rollback
still covers a page that cannot. The mode API
(`/api/system/firewall/mode`) stays for probes. The device configuration
leaves `webrtcMedia` unset, so Restricted and Paranoid block WebRTC as the
reference and its texts say; the local media range remains an explicit
configuration option.

Serving the page (plan of 2026-09-27, phase F): each `nft` run goes through
a transient systemd unit (about half a second on the board), so the owner
reads the ruleset once per status and serves the preview and the three
listings from that snapshot (later chunks reuse it for 10 seconds). The
firewall state, baseline and listing requests run on the firewall's own lock
and a blocking worker, like the mode runtime, so HID and media requests never
wait on `nft`. The server calls the firewall with a 30-second client, reads
the status afresh after a change (once more if that read fails) and answers
other reads, such as the video menu's, from a 5-second cache. Both WebRTC
transports (H.264 and VP8) are unavailable in the video menu while
`webrtcBlocked` is set, and a page using one falls back to MJPEG.

## QUIC transport (plan of 2026-10-01)

The QUIC transport (`docs/design/quic-transport-v1.md`) listens on UDP 443
while its switch is on. No rule is added for it in any mode:

| Mode | UDP 443 from the page's network | The page |
| --- | --- | --- |
| baseline | accepted, like everything | QUIC can be chosen |
| moderate | accepted from the local ranges, like every new local-source connection; dropped from elsewhere | QUIC can be chosen from a local address |
| restricted | dropped: only local-source HTTPS (TCP 443) and SSH come in | only Standard is offered, with the reason |
| paranoid | dropped: only local-source HTTPS comes in | only Standard is offered, with the reason |

- The firewall status reports `quicBlocked` beside `webrtcBlocked`, true in
  Restricted and Paranoid. The web server answers a session request with
  `blocked` while it is set and `GET /api/transport` says so, which is how
  the page knows why before it tries.
- The rule and the refusal are tested on the host
  (`the_quic_port_is_closed_in_the_restrictive_modes_only` and the transport
  tests of the server, and the page's contract `quic_transport_contract.mjs`,
  which plays a blocked port). On the board
  the transport was exercised in Baseline only: the plan forbids leaving a
  mode change in place, and a mode change was not needed to show the rule.
- Whether Restricted should admit UDP 443 from the local ranges, as it
  admits TCP 443, is left to the operator; it would be one rule beside the
  HTTPS one and `quicBlocked` false in that mode.

## Remaining implementation and acceptance gates

1. Typed four-mode control request/status, explicit confirmation identity and
   coherent API/UI schema; reject unsupported or ambiguous state.
2. Pure bounded IPv4/IPv6 rule generation with explicit native port inputs and
   regression fixtures derived from the matrix above.
3. Atomic owned-table transitions, previous-owned-state rollback, persistent
   pending metadata and restart reconciliation; injected failure/race tests.
4. Semantic verification of actual nft rules and helper output under the native
   systemd sandbox. Local `nft` is currently absent; no syntax/runtime gate passed.
5. Device connectivity checks for HTTPS, DHCP, IPv6 discovery, Tailscale and
   WebRTC in each mode, including intentional restrictions, timeout rollback,
   confirmation, restart and recovery. Done on 2026-09-27 (plan phase D in
   `docs/baselines/corrective-parity-audit-20260920.md`), except a live DHCP
   renewal (12-hour lease) and tailnet peer traffic, which needs a Tailscale
   login and is blocked by the restrictive modes as in the reference.

This document records verified reference behavior, the native adaptation and
the device checks of plan phase D.
