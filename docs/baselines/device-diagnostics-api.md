# Authenticated current-only diagnostics API checkpoint

On 2026-09-01, source commit
`f0973ce30ce485ca8a4d7cbddae7047498acb76a` added the platform-neutral
diagnostics-view v1 contract, authenticated GET-only `/api/diagnostics`, its
TypeScript contract/client, and packaged API documentation. The wire contract
is in `../api/diagnostics-v1.md`.

The endpoint derives only from the server's existing read-only inventory broker
request. The network process links no diagnostic persistence, key-loader,
store, export-client, floor, acknowledgement, process-control, or hardware
crate. Persistent history is fixed
`unavailable / persistence_not_provisioned`.

## Closed current-health schema

Diagnostics view schema one contains:

- temperature as available plus bounded millidegrees Celsius, or unavailable
  plus null;
- external RTC as one paired bounded epoch/offset observation, or unavailable
  plus two null values;
- fixed video, HID, power-observation, virtual-media and power-LED fields using
  only `active`, `inactive`, or `unavailable`; and
- fixed `current_only` availability and unprovisioned-history state.

It omits platform ID, software versions, bundle text, collector instance/window
bindings, omission counters, paths, account/numeric identities, device
identifiers, addresses/topology, credentials, keys/nonces, raw I/O errors, and
vendor logs. Invalid or incoherent inventory maps to the existing generic
sanitized 503 response.

The route is inside the existing opaque HttpOnly session middleware.
Unauthenticated access returns 401; state-changing methods are not routed. No
collector read/acknowledgement or persistence action is reachable from HTTP.

## Host verification

Five new core tests cover sanitized mapping, explicit missing metrics, invalid
inventory/view coherence, privacy serialization, canonical decoding, and
unknown-field rejection. Three new server tests cover authentication, exact
typed/private output from a framed broker fixture, GET-only routing, and
sanitized backend failure. The server suite contains 27 tests and the core
suite contains 17 tests.

The TypeScript v1 type and GET-only client compile in the production frontend;
ESLint and Prettier pass. Thirty-seven packaging tests prove the protected-route
position, exact source type, server dependency exclusion, frontend contract,
API documentation, device-gate assertions, and prototype documentation copy.

Source Longrun job `9ce28d88917246d28003d6cba2156a6a` exited zero with the
complete locked Rust/all-features/Clippy, source/systemd, 171-Python-test,
TypeScript, BasedPyright, frontend build/lint, and formatting matrix. Tooling
job `cf7eb08aa4184541a1d0cea1c17ac2e5` repeated that matrix after adding the
reversible device assertions and also exited zero.

Final evidence job `de66f771b2bc48479c711aca37312358` repeated the complete
matrix from clean API-evidence commit
`3a486608fd9b84ba56439ea10f89ed2398743aac` and exited zero.

The project LSP server was retried for the new Rust contract and returned
`document limit exceeded: 100/100`; LSP diagnostics are not claimed. Compiler,
tests, Clippy, TypeScript and direct boundary checks are authoritative here.

The locked graph remains 175 packages: 58 workspace and 117 external, with no
missing external license expression. The view adds no dependency; the broker
protocol is test-only in Server. `cargo audit` loaded 1,233 RustSec advisories
and reported no vulnerability in the 175 locked dependencies.

## Reproducible AArch64 evidence

Longrun jobs `bbdbfa1dea2341fa8dbdadc0e4017f35` and
`0b5862b647144c829c5c8d5827098287` used independent target and bundle roots.
The byte-identical 4,805,206-byte prototype archives have SHA-256:

`0372390059283922e95e4d7163aa83fe60a7be2af764f89b316f1d3288802209`

Both manifests bind the exact clean source commit and retain
`releaseAccepted=false` and `deviceAccepted=false`. The archive now contains
the current-only API documentation and updated server/web payload, but no
diagnostic persistence/gate binary, unit, key, credential, or journal.

The byte-identical 1,822,472-byte AArch64 static-PIE servers have SHA-256:

`5b9d0904308175fe62c805d3b8dfd0792a58d6b21654ce27996643b22ad729be`

The complete 1,812,712-byte core rlibs and 1,725,964-byte standalone rmeta
contain target-directory-dependent derive metadata and therefore differ:

- rlib A:
  `781af1dda46e2d7b99380030e3a7c058019f4fd2786bad1b57f2db58c01e45d2`;
- rlib B:
  `a2e50e5cd982d0d80a9d3f68c9a4d869aff959ca3dd3bf13206e4891e501ef80`;
- rmeta A:
  `07b5a5639a29a37a081dc8e259bcb579753561148cb80582d822b93377dee5d8`;
- rmeta B:
  `a538a465de4171cf7243d35137b778837447cace268c0ec7a3d49ed129c94f2b`.

The extracted 62,992-byte core ThinLTO codegen member is byte-identical:

`8e76c6656168d01106cae84d506db67c3a23141ac0326658f67af71b165ae094`

Shipped server and complete archive reproducibility are accepted; full core
metadata reproducibility is explicitly not claimed.

## Reversible H616 evidence

Final Longrun job `8c5ee6fe988f40edaafd9165da90bfdf` exited zero using the
exact archive above on the designated test-only H616. The private mode-0600
receipt is `evidence/private/device-diagnostics-api.json` and contains only
allowlisted semantic facts and public artifact hashes.

The gate proved:

- unauthenticated diagnostics returned HTTP 401;
- an authenticated GET returned HTTP 200, schema one, `current_only`, typed
  fixed component states, and `persistence_not_provisioned` history;
- the independently sampled diagnostics temperature and RTC values were
  available and within the accepted bounds;
- the response passed the identity/version/bundle/binding/path/credential/raw
  privacy denylist;
- existing inventory and capability contracts remained valid;
- the read-only video path decoded 74 frames and session revocation passed;
- the server had no KVMD group or credential, while broker/video/server
  sandbox and empty-capability invariants passed; and
- no automatic diagnostic persistence or acknowledgement action occurred.

The separate final-state request confirmed the temporary account and runtime
units were removed, htpasswd restored byte-identically, port/runtime/stage and
watchdog absent, KVMD/KVMD Nginx active, and rootfs read-write. No physical
Power/Reset, reboot, shutdown, KVMD restart, hardware action, clock write,
network mutation, or raw vendor log was invoked.

Two earlier live attempts failed inside the new diagnostics assertions and
both reported `remoteCleanupConfirmed=true`. The first had only a broad
`diagnostics` phase. Refined phases then showed the second failed because it
required bit-identical temperatures from two sequential sensor reads. The
accepted replacement validates each current temperature/RTC sample
independently and boundedly, while host unit tests prove exact single-inventory
mapping. Private mode-0600 receipts retain both failed attempts.

## Remaining boundary

This accepts the authenticated current-health API on ordinary H616 runtime and
TLS-independent loopback transport. It does not add persistent history,
production key/floor/runtime custody, the dedicated Diagnostics tab, browser
rendering, packaging permanence, privacy/soak, reboot, power-cut, or release
acceptance.
