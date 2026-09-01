# Prototype bundle boundary

`scripts/build-prototype-bundle.sh` creates a deterministic, non-installing
development archive. It contains only:

- the verified AArch64 static-PIE server and read-only broker;
- the non-enabled local update supervisor without a production trust config;
- the production frontend without MSW or legacy branding assets;
- reviewed systemd/sysusers templates;
- license, architecture, security, and protocol documentation;
- a machine-readable manifest and per-file checksums.

The archive deliberately contains no credential, test endpoint, private
evidence, production signing key or update configuration, install script,
device image, recovery claim, enabled service, or hardware-mutation command.
`releaseAccepted` and `deviceAccepted` remain false in its manifest.

File ownership and modes are produced in a Linux `/tmp` staging tree before
the archive is written to D:, avoiding the 0777 mode presentation of the
Windows 9p mount. The tar metadata is sorted, owned by numeric root, and uses
epoch timestamps; gzip timestamps are disabled.

This artifact may be inspected and copied to disposable test storage only
after the device-write gate is approved. It is not a firmware or public
release.

## Verified checkpoint

On 2026-08-30, two independent output directories produced byte-identical
3,229,114-byte commit-bound archives after adding the TLS reverse-proxy
template and browser-gate boundary:

`a5cca8ce9f973516213e7cc60f61909bce4a8cf012618758406e1985493d437f`

Independent extraction verified every `SHA256SUMS` entry, numeric root archive
ownership, 0755 binaries/directories, 0644 web/docs/manifest files, absence of
symlinks, legacy/MSW assets, host paths, private markers, and test-network
placeholders. The manifest binds the archive to source commit
`3cd926b2dca6c51b011c93577f69c89b2ca4582a`, while both acceptance booleans
remain false. Its server, credential-isolated read-only broker, dedicated KVMD
account rollback, runtime systemd, TLS, and real-browser slices passed the H616 gates
documented in `device-prototype.md`, `device-broker-inventory.md`, and
`device-systemd-gate.md`, plus `device-tls-browser.md`; the hash remains a
development checkpoint, not a firmware or release identifier.

## Read-only thermal checkpoint

A later byte-identical bundle pair at source commit `329194e` added the bounded
runtime-inventory v2 temperature slice and passed its focused systemd and TLS
Chromium gates. Exact artifact hashes, accepted behavior, rollback evidence,
and the independent video-throughput limitation are recorded in
`device-thermal.md`.

## Read-only external RTC checkpoint

A later byte-identical bundle pair at source commit `cc37a3c` added the fixed
PCF8563 inventory-v3 epoch/offset observation. Its focused systemd and TLS
Chromium gates passed without opening raw I2C or `/dev/rtc1` and without setting
either clock. Exact hashes, rollback evidence, and remaining drift/battery/write
limits are recorded in `device-rtc.md`.

## Host-only fan-policy checkpoint

Source commit `09cd041` added an unlinked pure fan-policy crate. Two independent
workspace builds still produced byte-identical prototype archives; the archive
does not contain the library. Separate extraction showed byte-identical AArch64
ThinLTO policy object code while non-shipped rlib metadata differed. Exact
hashes and the no-runtime/no-hardware boundary are recorded in
`fan-policy.md`.

## Unlinked fan-supervisor checkpoint

Source commit `9979c77` added a mock-backed, unlinked fan supervisor. Two
independent workspace builds retained byte-identical prototype archives; the
archive contains neither fan crate. Extracted AArch64 ThinLTO supervisor object
code was byte-identical while rlib metadata differed. Exact hashes and the
controlled-shutdown-versus-crash boundary are recorded in
`fan-supervisor.md`.

## Unlinked diagnostic-store checkpoint

Source commit `2efe1f7` added a fixed-root encrypted diagnostic store without
linking it into a process or archive. Independent target/output roots produced
byte-identical 4,799,880-byte prototype archives with SHA-256
`58b4058352bc54695b58f1384161c64275c4527fe9e44958b9490a1cc738fdc5`.
Their manifests bind the clean source commit while retaining both acceptance
flags as false. Separate store/journal rlib, metadata, and ThinLTO comparisons
also matched; exact hashes and the unprovisioned/no-device boundary are in
`diagnostic-store.md`.

## Unlinked diagnostic-key-loader checkpoint

Source commit `b3266c3` added the fixed systemd diagnostic key loader without
linking it, the store, or journal into a process or archive. Independent
target/output roots produced byte-identical 4,800,533-byte prototype archives
with SHA-256
`0f2b1c0886ddf08d497300e28e0da5e81900a7c95812aef5a504587c84ed2484`.
Their manifests bind the clean source commit while retaining both acceptance
flags as false. Separate loader/journal rlib, metadata, and ThinLTO comparisons
also matched; exact hashes and the no-key/no-device boundary are in
`diagnostic-key-credential.md`.

## Unshipped diagnostic-maintenance checkpoint

Source commit `dfb47a2` added the empty-only maintenance executable, corrected
systemd credential access, nonmutating store inspection, units, and tmpfiles
source without copying any of them into the archive. Independent target/output
roots produced byte-identical 4,800,673-byte prototype archives with SHA-256
`bc77d8bb555d64306a38d829f0687ee3372c9cd797cf1f35f327e070c834b729`.
Their manifests retain both acceptance flags as false. Separate executable and
maintenance/loader/store/journal rlib, metadata, and ThinLTO comparisons plus a
reversible H616 gate passed; exact evidence is in
`diagnostic-maintenance.md`.

## Unlinked diagnostic-coordinator checkpoint

Source commit `945eb19` added the floor-gated, cancellation-aware coordinator
without linking it or any diagnostic persistence layer into the archive.
Independent target/output roots produced byte-identical 4,802,264-byte
prototype archives with SHA-256
`7ef8812480c939dab37ac3efa479f651837cf86a7c9a3eb07185f86637b3362b`.
Their manifests retain both acceptance flags as false. Coordinator rlib,
metadata, ThinLTO, and a fully instantiated AArch64 test executable also
matched; exact evidence and the no-device boundary are in
`diagnostic-coordinator.md`.

## Unlinked diagnostic-persistence checkpoint

Source commit `57959b0` added the one-step fixed-type persistence adapter
without linking it or the lower diagnostic persistence graph into the archive.
Independent target/output roots produced byte-identical 4,802,698-byte
prototype archives with SHA-256
`06164420cfaad73eebc854751e3830386f32c48b34275c2d3a91654b0815922f`.
Their manifests retain both acceptance flags as false. Persistence rlib,
metadata, ThinLTO, and a fully instantiated AArch64 test executable also
matched; exact evidence and the no-device boundary are in
`diagnostic-persistence.md`.

## Synthetic diagnostic-persistence H616 checkpoint

Source commit `7bef2bc` added an unshipped test-only gate binary/unit and
rollback orchestrator without copying them into the archive. Independent
target/output roots produced byte-identical 4,803,076-byte prototype archives
with SHA-256
`e02729175d8a8f76acd3e2592755b74733dba2d74d039174f2f80713512288a3`.
Their manifests retain both acceptance flags as false. The byte-identical
AArch64 gate executable subsequently passed the synthetic rollback-gated H616
flow; exact evidence and production limitations are in
`device-diagnostic-persistence.md`.
