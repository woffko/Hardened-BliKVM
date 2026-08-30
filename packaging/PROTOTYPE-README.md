# Hardened BliKVM prototype

This is a non-installing development bundle for BliKVM v4 H616 evaluation.
It is not firmware and is not accepted for release or recovery.

The bundle contains a static AArch64 server, a read-only KVMD platform broker,
a separately isolated read-only MJPEG proxy, a denied-by-default control policy
broker with no backend, the capability-gated web interface, service templates,
checksums, security documentation, and the non-enabled local update supervisor.
It contains no credential, production update trust configuration, signing key,
install script, enabled service, or enabled hardware mutation.

Do not modify a working device merely to inspect this archive. On-device
staging requires a separately approved write/rollback gate. Review
`MANIFEST.json`, `SHA256SUMS`, and `share/docs/packaging-boundary.md` first.
