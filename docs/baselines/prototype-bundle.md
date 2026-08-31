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
