# Hardened settings port plan

Status: new plan created 2026-09-13 after the Phase 3 device acceptance. It
covers the operator settings that can be ported to the hardened stack without
physical acceptance. Phases 4 device gates and every physical/hardware gate
remain deferred by the operator.

## Rules

- No visible control may call an endpoint the hardened server does not expose.
  A feature is either wired to a real hardened endpoint or hidden behind a
  capability, never left as a dead legacy call.
- New server endpoints are read-only unless a reviewed, compile-time-gated
  broker profile backs a mutation.
- Every slice ships with tests and a green `scripts/check.sh`; private evidence
  stays out of Git.

## Inventory of the operator surfaces

The operator GUI references about 110 `/api/...` endpoints while the hardened
server implements only the hardened subset. Settings tabs are already
capability-gated through `supportsSettingsTab`:

| Surface | State | Action |
| --- | --- | --- |
| about | shown | port read-only hostname, keep version/inventory |
| diagnostics | shown | already backed by `/api/diagnostics` |
| appearance | shown | client-only plus unsupported menu-icon probes to remove |
| account | shown | already backed by auth endpoints |
| device / system / network / tailscale / update | hidden on the pinned platform | keep hidden until a reviewed capability exists |
| desktop toolbar (keyboard, power, media, terminal, script) | partly ported | continue gating absent legacy calls |

## Categories

- A. Already supported: auth account/password/setup/logout, capabilities,
  health, inventory, diagnostics, MJPEG stream, restricted HID focus/lease,
  restricted application power, application-update in capability builds.
- B. Client-only, no server work: appearance theme/layout/language, local
  storage preferences, on-screen keyboard layouts.
- C. Portable now, no physical acceptance: read-only hostname and version,
  thermal/RTC/board read-only values from inventory, diagnostics export,
  system-log read (bounded), application-update status.
- D. Capability-gated, hidden on this platform: device/system settings,
  tailscale, system-update raw, network configuration, ssh/terminal/script,
  picoclaw, storage/media (Phase 5), hdmi/oled/fan/gpio (physical).
- E. Security-sensitive, out of scope unless explicitly approved: script and
  terminal unlock, ssh enable, firewall mutation, tailscale install,
  system-update raw/install, network and WOL mutations.

## Phases

0. Coherence audit: remove every dead call from a visible surface (About
   hostname, Appearance menu-icon probes) and keep the capability gates.
1. Read-only information endpoints: hostname (and, where safe, system time
   read) with server tests and GUI wiring.
2. Diagnostics and system-log parity within the existing read-only boundary.
3. Virtual media wiring (Phase 5): control-v2 media request, broker
   `media-transition-profile`, server routes, GUI; then real-target gates.
4. Board and physical features: only after explicit physical acceptance.

## Slice 0 detail

- Add `GET /api/hostname` returning the bounded system hostname from
  `/proc/sys/kernel/hostname`; no setter.
- About uses the new read-only endpoint and no longer renders a hostname editor
  unless a future writable capability exists.
- Appearance stops probing the absent terminal/download endpoints and derives
  those menu entries from capabilities instead.
- Host tests pin the route, the read-only boundary and the GUI wiring.
