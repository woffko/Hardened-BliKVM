# Prototype bundle boundary

`scripts/build-prototype-bundle.sh` creates a deterministic, non-installing
development archive. It contains only:

- the verified AArch64 static-PIE server and read-only broker;
- the production frontend without MSW or legacy branding assets;
- reviewed systemd/sysusers templates;
- license, architecture, security, and protocol documentation;
- a machine-readable manifest and per-file checksums.

The archive deliberately contains no credential, test endpoint, private
evidence, updater, install script, device image, recovery claim, or mutation
command. `releaseAccepted` and `deviceAccepted` remain false in its manifest.

File ownership and modes are produced in a Linux `/tmp` staging tree before
the archive is written to D:, avoiding the 0777 mode presentation of the
Windows 9p mount. The tar metadata is sorted, owned by numeric root, and uses
epoch timestamps; gzip timestamps are disabled.

This artifact may be inspected and copied to disposable test storage only
after the device-write gate is approved. It is not a firmware or public
release.

## Verified checkpoint

On 2026-08-30, two independent output directories produced byte-identical
3,227,624-byte commit-bound archives after the static-PIE CRT and KVMD internal
route corrections:

`b3428d947c021d0e391330a872a00d9cb2f7fb467e061d3de14ed09dd129f181`

Independent extraction verified every `SHA256SUMS` entry, numeric root archive
ownership, 0755 binaries/directories, 0644 web/docs/manifest files, absence of
symlinks, legacy/MSW assets, host paths, private markers, and test-network
placeholders. The manifest binds the archive to source commit
`c3c8e8bafaec423af2c0af8cb16e5ebc0b68fc46`, while both acceptance booleans
remain false. Its loopback-only server and credential-isolated read-only broker
slices passed the temporary H616 gates documented in `device-prototype.md` and
`device-broker-inventory.md`; the hash remains a development checkpoint, not a
firmware or release identifier.
