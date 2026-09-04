# Application public-trust provisioning gate v1

## Status and stop condition

This gate publishes only the initial application-update supervisor
configuration. Its source and host tests can be completed without a real key,
but device preflight or enrollment requires all of the following external
inputs:

- one user-reviewed public-only enrollment request;
- its exact SHA-256 supplied separately;
- explicit `--confirm-reviewed-public-key`; and
- the exact hash-bound AArch64 validator from the accepted source commit.

The gate must stop before contacting the device when any input is absent or the
review confirmation is false. A synthetic test key cannot authorize persistent
enrollment. A production private key is never an input and must never be
provided to this project or tool.

## Host key ceremony helper

`tools/generate_production_update_signing_key.py` creates one encrypted
PKCS#8 Ed25519 key in the invoking operator's owner-only local data directory,
outside the repository and `/tmp`. It requires an interactive terminal so
OpenSSL receives the passphrase directly, refuses to overwrite an existing key
directory, uses exclusive no-follow file creation, and leaves the encrypted
private key mode 0400. The helper emits only the public SPKI, raw 32-byte public
key, closed enrollment request, request hash, and a public review receipt.

The helper does not make an online workstation an offline signer and does not
establish custody by itself. Before enrollment, the operator must separately
back up the encrypted private key, retain the passphrase through a different
secure channel, review the exact public request and both SHA-256 values, and
confirm that this key is the intended production trust root. The private key
must never be copied to the repository, `/tmp`, Project Memory, device staging,
the test device, logs, chat, or an update bundle. The public request remains an
external input to the provisioning gate even when the helper created it.

## Local asset boundary

The validator must be a regular owner-only static-PIE AArch64 snapshot directly
under `/tmp`, named by its SHA-256. The request must be a regular owner-only
mode-0600 file directly under `/tmp`, also named by its separately supplied
SHA-256. Both have strict size and single-link limits. The orchestrator does not
parse or reinterpret the public key; the accepted Rust contract is the sole
renderer of canonical supervisor configuration.

## Read-only device preflight

`--preflight-only` verifies the reviewed local assets and then confirms:

- exact runtime and inert update-runtime packages;
- absent supervisor configuration, update state, slots, update runtime, and
  confirmation runtime;
- all update/current/candidate units inactive and not enabled;
- healthy persistent broker/video/server/Nginx with exact PIDs and restart
  counters captured;
- unchanged account, Nginx, and TLS hashes;
- a root-owned mode-0700 `/etc/hardened-blikvm` parent; and
- absence of reboot, web-cutover, package, runtime-upgrade, or trust-enrollment
  guards.

The read-only mode returns before upload and records that no device state was
changed.

## Validation and guarded publication

A mutating run creates one unique `/tmp` stage, uploads only the hash-bound
validator and public request, and invokes:

```text
validator --expected-sha256 HASH --confirm-reviewed-public-key
```

The request is stdin and canonical config is captured into the stage; neither
is placed in argv, environment, logs, or receipts. Before persistent write, the
gate binds the config hash into a root-owned rollback marker/program, an
early-boot service, and a ten-minute timer.

Publication copies only the validated bytes to a unique same-filesystem part,
sets root ownership and mode 0600, syncs it, and atomically renames it to
`/etc/hardened-blikvm/update-supervisor.json`. The gate then requires the exact
hash, type, mode, owner, and link count. Update state/slots/runtime stay absent;
all update units stay inactive and not enabled; all persistent runtime
PIDs/restarts, account/Nginx/TLS hashes, ports, health, and control boundary
remain unchanged.

## Rollback and acceptance

Rollback removes the published file only if its hash and metadata still match
the validator output. It removes only marker-owned staging and guard paths and
then re-establishes configuration absence, inert update units, absent update
runtime/state/slots, and healthy persistent service state. If an unexpected
config or marker is found, rollback stops and retains evidence.

Success disarms and removes both guards and all transient assets, then opens a
new SSH transaction to repeat the persistent config and unchanged-runtime
checks. It does not initialize update state or slots, enable a unit, stage or
activate an application, restart the GUI/KVMD/device, expose an update route,
or invoke ATX Power/Reset or equivalent attached-target power control.
