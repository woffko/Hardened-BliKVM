# Application trust enrollment v1

## Scope

This contract validates one reviewed initial Ed25519 application public key and
renders the exact existing supervisor configuration schema. It never accepts,
derives, stores, transmits, or renders a private key. The implementation is a
pure Rust library with no filesystem, network, process, state, slot, unit,
device, or activation authority.

## Enrollment request

The closed JSON object contains exactly:

- `schemaVersion`: `1`;
- `product`: `Hardened BliKVM`;
- `target`: `bli-h616-pikvm`;
- `generation`: `1`;
- `replacesKeyId`: `null`;
- `keyId`: the existing application verifier key identifier;
- `publicKeyHex`: 32 lowercase hexadecimal Ed25519 public-key bytes; and
- `publicKeySha256`: lowercase SHA-256 of those exact 32 bytes.

The document is limited to 4096 bytes and rejects unknown fields. Generation 1
cannot replace another key. Rotation, multiple active keys, revocation, and
grace periods require a later version rather than overloading this initial
enrollment format.

The public-key encoding, Ed25519 point, key identifier, and fingerprint are
validated through the same `ApplicationTrustRoot` used by artifact
verification. A matching fingerprint detects transfer/transcription errors;
it is not a signature or proof of custody. The operator must separately review
the public key and its offline ceremony before any persistent enrollment.

## Rendered supervisor configuration

Successful validation renders canonical compact JSON plus one final newline
with fixed values:

- config schema `1`, product, target, key identifier, and public key;
- slot root `/var/lib/hardened-blikvm-slots`;
- state root `/var/lib/hardened-blikvm-update`;
- runtime root `/run/hardened-blikvm-update`;
- initial system sequence `1`; and
- health timeout `60000` milliseconds.

The request cannot select a path, timeout, system sequence, algorithm, product,
target, state layout, endpoint, URL, unit, or command. The rendered document
contains no enrollment generation or rotation claim because the current
supervisor schema supports exactly one active trust root.

## Remaining gates

This pure validation does not identify a real production key or establish
private-key custody. Before persistent use, a later host tool must require the
exact reviewed request hash, render a new root-owned mode-0600 configuration,
arm marker/hash/account-bound rollback, leave every update unit disabled, and
independently verify the file. State/slot initialization, enabling units,
candidate signing/staging, interruption/reboot, update GUI, recovery, and
release acceptance remain separate. ATX Power/Reset and equivalent
attached-target power control are outside this contract.
