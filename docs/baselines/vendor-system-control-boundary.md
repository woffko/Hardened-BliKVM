# Vendor system-control boundary audit

The audit is pinned to
`blikvm/blikvm@3e34c04a1dfa84636050585d12d5e585098258ea` and web-server
submodule `932a47740020268ba89c254809d175a371944681`.

No vendor route or system command was executed. Existing sanitized H616
baselines already establish architecture, OS/runtime versions, rootfs mode,
systemd generation, storage geometry, thermal/RTC observations, service health,
and bounded resource facts. A new live system-info capture was intentionally
not added because the vendor response combines those facts with identity and
private topology.

## Audited routes

| Route | Vendor behavior | Hardened classification |
| --- | --- | --- |
| `POST /api/reboot` | invokes `reboot` through a generic shell helper, with no response, confirmation, idempotency, cooldown, update guard, or audit receipt | unavailable; irreversible action policy required |
| `GET /api/systeminfo` | returns CPU/memory/storage/time facts together with hostname/FQDN, board serial, MAC and IP addresses, network state, and configuration flags | route rejected; only separately allowlisted sanitized adapters may supply facts |
| `POST /api/hostname` | may remount root read-write, invokes `hostnamectl`, rewrites `/etc/hosts`, and restores read-only mode only when originally read-only | unavailable; transactional staging, rollback and connectivity/TLS gates required |
| `POST /api/logs` | reads a path from mutable configuration, shells out to unquoted `tail`, and returns raw log content | unavailable; only fixed-source typed events and bounded export may be considered |
| client `POST /shutdown` | client code exposes shutdown, but the audited server route table has no matching handler | unavailable; never infer server capability from client UI |

The shared `executeCMD` helper accepts arbitrary shell strings. Hostname input
has an RFC-1123-shaped validator, but the overall path still crosses root shell,
filesystem, name-resolution, and management-connectivity boundaries. Log
content can contain addresses, usernames, endpoints, request data, tokens or
other sensitive material even when the file path itself is valid.

## Hardened capability matrix

Read-only facts may be implemented only as independent exact allowlists:

- architecture and public OS/component versions;
- bounded uptime, memory/storage totals, thermal and paired RTC observations;
- fixed service-health booleans and previously sanitized interface classes;
- rootfs mode and recovery/update eligibility without identity-bearing paths.

The following data remains private and absent from public responses/evidence:

- hostname, FQDN, machine/board serials and machine ID;
- MAC/IP addresses, gateways, DNS, routes, SSIDs/BSSIDs and endpoints;
- usernames, process arguments, environment, raw logs and mutable config;
- disk/device serials, UUIDs, tokens, credentials and unique identifiers.

System mutations remain unavailable. Reboot/shutdown need explicit eligibility,
confirmation, persisted idempotency/cooldown, update and storage guards, visible
feedback, and post-boot acceptance. Hostname changes need a typed staged plan,
atomic file handling, exact rollback, name-resolution/TLS/connectivity checks,
and reboot persistence. Arbitrary log text is not accepted for regex redaction;
diagnostics require a fixed local source mapped to semantic events, size/time
bounds, authorization, and no shell path. Clock writes retain their separate
RTC policy and physical gate.

This audit changes no runtime capability. The first pure reboot/shutdown state
policy now implements the non-I/O portion of this boundary; see
`system-action-policy.md`. Its separate redundant journal model defines
canonical corruption detection and write ordering; see
`system-action-journal.md`. An unlinked exact-root atomic store now implements
host-tested metadata, lock, sync and readback semantics; see
`system-action-store.md`. Its full harness also passed in a cleaned H616 `/tmp`
stage, but the production root remained absent. None is provisioned, packaged,
system-action-accepted or connected to a privileged adapter, and none
authorizes reboot, shutdown, hostname, service, filesystem, log, or clock
actions. The unshipped maintenance boundary adds only empty initialization and
empty-state inspection under static confined units; see
`system-action-maintenance.md`. Its reversible H616 test restored the original
absence after both operations; no system action was introduced.

The separate trusted-floor audit found no live TPM, TEE/OP-TEE, or RPMB
interface despite generic TPM kernel support. No same-filesystem floor is
accepted; see `trusted-floor-topology.md`. Reboot/shutdown stay unavailable.

The first pure diagnostic-export model now enforces the semantic, bounded,
no-path/no-message portion of the log boundary; see `diagnostic-export.md`. It
performed no live capture. One consumer is an unlinked pure policy for
five fixed process roles, exact lifecycle epochs/sequences, global ordering and
a closed event matrix; see `diagnostic-collector-policy.md`. A separate pure
codec defines fixed event and mandatory accepted/rejected frames without source
or identity; see `diagnostic-protocol.md`. The pure two-window manager adds
lifecycle reservation, exact handoff acknowledgement, hard backpressure and
visible incomplete EOF metadata; see `diagnostic-window-policy.md`. The peer
policy maps validated exact UID/GID/PID facts to only the semantic source; see
`diagnostic-peer-policy.md`. There is still no account resolver, kernel
credential call, runtime collector, API, UI, package, or advertised diagnostic
capability.
