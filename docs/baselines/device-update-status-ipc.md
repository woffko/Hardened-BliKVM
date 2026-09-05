# Native read-only application-update status IPC

The H616 native status gate passed on 2026-09-05 with source `9d1c145`
and gate binding `d3bb1a9`. This is a temporary socket-worker acceptance,
not installed HTTP/GUI acceptance or permission to enable update mutations.

## Evidence

- Full host suite: `748e54470d1f4a4e9b904e9c9b5e493e`, exit zero.
- AArch64 build and ELF checks: `5cf09361fe334562bdbf616c8b99c22f`, exit zero.
- Supervisor SHA-256:
  `2a1fe1ca3b255c6b93b8b89b8cb2f85476fbed7453344cb0ec60fbe64033c181`.
- Native gate: `f1843b32a6b04c3483397eef4fe75a42`, exit zero.
- Four authenticated local reads returned identical generation ten, floor two,
  selected/confirmed sequence two, no pending operation, and `boot_changed` as
  the previous rollback reason.
- A root peer was rejected despite filesystem access; the fixed server UID
  succeeded. Mutation-shaped and oversized messages received no response.
- The independently repeated operational snapshot was identical before/after.
  Temporary socket, service, deadline, runtime and controller staging were
  removed. No production service restart or target power operation occurred.

## Failure found and fixed

The first native attempt `f11cf1398e7e433aa286d8d81f5868e8` failed on a valid
status request, with cleanup and unchanged operational state verified.
`read_status` opened a second slot stager while its manager already held the
exclusive slot-directory lock. Independent open descriptions contended even
within one process. Source `9d1c145` reads metadata through the manager's
existing stager and trust root without dropping the lock. A regression test
covers repeated metadata reads and invalid slot identifiers without changing
manager status or the selected pointer. Independent advisory review confirmed
the diagnosis; the successful native socket test supplies the full-path proof.

## Remaining gates

Independent-build byte comparison, installed status socket/API wiring,
authenticated HTTP and desktop/mobile rendering, and retained-package upgrade
acceptance remain. Signed upload and activation IPC are separate unfinished
work. Status has no apply/recovery dependency or writable filesystem allowance.
