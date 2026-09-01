# Dedicated Diagnostics GUI checkpoint

On 2026-09-01, product source commit
`8339018c74bdc4edd9889a418d7fdbce633cb769` completed the dedicated,
authenticated Diagnostics tab for the Hardened BliKVM web interface. The tab
uses only the accepted current-only `/api/diagnostics` contract. It does not
read inventory, the diagnostic export socket, the encrypted store, or vendor
logs directly.

## User-visible boundary

The separately navigable Settings tab renders:

- bounded SoC temperature;
- external RTC time and its bounded offset from system time;
- typed video, HID, power-observation, virtual-media, and power-LED state; and
- an explicit statement that persistent diagnostic history is not provisioned.

Every response is checked by a closed TypeScript runtime validator before it
is rendered. The validator rejects unknown fields, incoherent available/null
states, out-of-range temperature or RTC values, unknown component states, and
any history state other than
`unavailable / persistence_not_provisioned`.

The request lifecycle is generation-based. A manual refresh or successful
ten-second timer advances one generation; React cleanup aborts the preceding
generation before the next GET starts. Hiding or unmounting the tab aborts the
current request and clears its timer. A failed request keeps the last accepted
snapshot visibly stale and does not schedule an automatic retry. A first-load
failure shows a sanitized error without backend text.

The tab has no Power, Reset, reboot, shutdown, log, bundle, path, identifier,
topology, account, credential, or persistence action. Its only button requests
another authenticated GET.

## Localization and responsive layout

Twenty-three Diagnostics strings are present in all 26 canonical web locale
packs:

`ca`, `cz`, `da`, `de`, `en`, `es`, `et`, `fi`, `fr`, `hu`, `id`, `it`,
`ja`, `ko`, `nb`, `nl`, `pl`, `pt_br`, `ru`, `se`, `th`, `tr`, `uk`, `vi`,
`zh`, and `zh_tw`.

The tuple type enforces exact key parity at compile time. Existing `no -> nb`
and `sv -> se` aliases are preserved. Locale aliases that are not valid BCP 47
identifiers are normalized before `Intl.DateTimeFormat`; the bounded UTC
fallback prevents a malformed or unknown locale from crashing the tab.

The same component uses a one-column narrow layout and two-column wider layout.
The H616 Chromium gate rendered a 390 by 844 `zh_tw` viewport without document
overflow and with no action button.

## Host verification

Longrun job `e8c485567f6040079ae0ee5b6257a0f6` exited zero from the clean product
source commit. It passed the complete locked default/all-features Rust matrix,
Clippy with warnings denied, source/systemd checks, 172 Python tests,
BasedPyright with zero diagnostics, the production TypeScript/Vite build,
ESLint, and both Prettier checks.

Focused evidence also includes:

- 17 core and 27 server tests for the accepted API boundary;
- the Diagnostics route/lifecycle/privacy/localization packaging test;
- 12 TLS-browser orchestrator tests and Node syntax validation;
- a real local Chromium mock flow proving one manual GET on desktop and one on
  the 390 by 844 `zh_tw` layout; and
- six KVMD-client tests, including 12 consecutive five-endpoint inventory
  reads over keep-alive responses.

The KVMD read client now requests `Connection: close` and awaits cancellation
of its HTTP/1 connection task on every post-handshake result path. This is an
independently tested resource-lifetime hardening. It was investigated during
the browser gate but was not the cause of the early browser failures described
below.

Project LSP was retried during this work and remained at its configured
100-document limit. LSP diagnostics are therefore not claimed; compiler,
Clippy, BasedPyright, exhaustive tests, and browser evidence are authoritative.

## Reproducible AArch64 evidence

Longrun jobs `f81d30996d7449d689966aea44ecb735` and
`3311d78ffb304836904d74aa3a4e0f88` used independent target and output roots.
The resulting prototype archives are byte-for-byte identical, each
4,818,271 bytes, with SHA-256:

`9f13b56432dc0a73e700e881da23d4f31f57400b0cf69b6fd73dbf42fcd7f2e9`

Both manifests bind exact source commit
`8339018c74bdc4edd9889a418d7fdbce633cb769` and retain
`releaseAccepted=false` and `deviceAccepted=false`. Their web trees match at:

`86ec6a33ad9bb1e7626c4452073b9d49cd3df4c7d94497dcd687d5c7bd8e08cd`

All six shipped ELF pairs also match. The two changed/relevant artifacts are:

- 1,822,472-byte server:
  `5b9d0904308175fe62c805d3b8dfd0792a58d6b21654ce27996643b22ad729be`;
- 1,000,144-byte platform broker:
  `8d41c82220c0fa14c9427d22b66f33b748dfc0c30b6ba64e3fdef43b00ee9b7e`.

Archive equality covers the complete shipped web tree, binaries,
configuration, units, and documentation. It does not promote this development
prototype to a release.

## Reversible H616 browser evidence

Final Longrun job `d9d6e3667e4d4ea8ab0b9ef3aa449f3d` exited zero using the exact
archive above. Extended state job `e69c92dc79994a418b4be8baf59cfc12`
then repeated the full live flow with deterministic page-local loading,
unavailable, stale, and error fixtures. The final private mode-0600 receipt is
`evidence/private/device-diagnostics-ui.json`; it contains only allowlisted
semantic facts and public artifact hashes.

The accepted gate proved:

- TLS 1.2 and TLS 1.3 through a device-loopback Nginx listener reached only by
  strict-host-key SSH forwarding;
- secure HttpOnly SameSite=Strict session cookies and secure readable
  SameSite=Strict CSRF cookies;
- the live authenticated current-only Diagnostics response passed its closed
  schema and privacy checks;
- desktop and 390 by 844 `zh_tw` layouts rendered temperature, RTC, component
  health, and explicitly unprovisioned history;
- desktop and mobile Refresh each caused a new GET and HTTP 200 response;
- loading, unavailable, stale-last-good, and sanitized first-load error states
  rendered correctly against page-local fixtures;
- four MJPEG connections decoded/reconnected, password rotation and logout
  revoked sessions, and the active stream ended on revocation;
- 38 API responses were observed with no 404, no WebSocket, and no control
  request; and
- no Power/Reset or other hardware/system action was invoked.

The bounded browser receipt records `diagnosticsMobileUi` as
`zh-tw-responsive-action-free`, `diagnosticsLoadingUi` and
`diagnosticsUnavailableUi` as rendered, `diagnosticsStaleUi` as
`rendered-last-good`, and `diagnosticsErrorUi` as `rendered-sanitized`.

Preflight and final-state checks confirmed AArch64, clean temporary state,
KVMD and KVMD Nginx active, rootfs read-write, ports free, and all temporary
runtime, diagnostic, TLS, account, process, and staging state removed. No KVMD
restart, reboot, shutdown, clock write, network mutation, or persistent path
change occurred.

## Failed-attempt lesson

The early browser attempts all completed rollback. A broad phase label first
made a later modal-close timeout look like a Diagnostics response timeout.
Event-count deltas proved the GET and HTTP 200 had completed; a phase placed
immediately before every awaited action then identified Escape-based modal
closing. After switching to the explicit close button, the first mobile run
exposed a separate test-design conflict: two pages competed for the temporary
single-reader MJPEG proxy. Releasing the desktop stream before opening the
mobile page produced the accepted flow.

The durable rule is to phase every awaited browser action, compare request and
response counts relative to a captured baseline, and never start a second
video reader until the first has been released. A speculative KVMD
connection-leak theory was explicitly disproved as the cause.

## Remaining boundary

This checkpoint accepts the dedicated current-only Diagnostics GUI and its
temporary H616 browser runtime. It does not provision encrypted history, a
production key or external rollback floor, permanent services/files, release
metadata, reboot persistence, long soak, power-cut behavior, other browser
engines, or physical Power/Reset behavior.
