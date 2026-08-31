# Trusted rollback-floor topology decision

On 2026-08-31, the rollback-floor audit compared pinned vendor source, the
field-image kernel/DTB, and sanitized live H616 metadata. It found no currently
usable independently trusted monotonic counter. System actions therefore remain
unavailable; a floor stored beside the journal is not an acceptable substitute.

The pinned vendor application source at
`blikvm/blikvm@3e34c04a1dfa84636050585d12d5e585098258ea` contains no TPM,
RPMB, OP-TEE, TEE secure-storage, or monotonic-counter integration.

The field image contains kernel `5.19.4-sunxi64`. Its saved configuration has:

- built-in TPM framework and TPM-backed trusted-key support;
- TIS and TIS-SPI TPM transports as modules;
- built-in MMC block and sunxi MMC support;
- `CONFIG_TEE` explicitly disabled and no usable OP-TEE configuration.

Framework configuration is not hardware evidence. The field image's H616
Mangopi DTB has three MMC controller nodes and no TPM, OP-TEE, TEE, RPMB, or
secure-counter node. Its H616 eMMC-compatible controller is disabled in that
DTB.

The live metadata-only probe at source commit `1bf489a` observed:

- one MMC block device, but zero RPMB sysfs entries and zero RPMB `/dev` nodes;
- no `mmcblk0rpmb`, `mmcblk1rpmb`, or `mmcblk2rpmb` node;
- no TPM class entry, `/dev/tpm0`, or `/dev/tpmrm0`;
- the generic TPM core module present, but no loaded TIS core and no TPM device;
- TEE disabled in the running kernel config, with zero TEE class entries,
  `/dev/tee0`, `/dev/teepriv0`, OP-TEE module, or device-tree OP-TEE node;
- rootfs still read-write.

The probe used only kernel-config greps, fixed path existence/counts, module
directory presence, and `stat` type/mode metadata. It did not open or read any
TPM, TEE, RPMB, MMC, block, or character device; did not read CID, CSD, serial,
TPM endorsement, TEE UUID, or another identifier; and did not write files,
provision keys/objects, call rootfs helpers, change services, or invoke a
physical/system action.

The private receipt is mode 0600, Git-ignored, and contains no routing,
credential or device identity. Kernel TPM support without a bound device, an
ordinary MMC device without an exposed RPMB interface, and software state on
the same rollbackable filesystem cannot provide the required external floor.

Future eligibility would require different verified hardware or firmware that
exposes a supported TPM NV counter, authenticated eMMC RPMB, or an equivalent
secure element/TEE together with a reviewed one-way provisioning ceremony,
key custody, monotonic semantics, recovery/replacement plan, fault injection,
and physical acceptance. No key or irreversible object may be provisioned
during exploratory work.
