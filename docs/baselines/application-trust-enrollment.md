# Application public-trust enrollment checkpoint

Source commit `0ee863dfe78f5e0ccab11e4c07cf984c11fdb3c2` adds the pure Rust
`bli-kvm-update-trust-config` contract and
`api/application-trust-enrollment-v1.md`. No real production key was selected,
created, stored, or enrolled, and no device operation occurred.

## Closed public-only request

The enrollment request contains only the exact product/target, initial
generation, null replacement, verifier-compatible key identifier, 32-byte
lowercase Ed25519 public key, and SHA-256 of those public bytes. The validator:

- limits the document to 4096 bytes and rejects unknown fields;
- accepts only schema/generation 1 and no replacement key;
- decodes only lowercase fixed-length hexadecimal input;
- validates the Ed25519 point and key identifier through the same
  `ApplicationTrustRoot` used by artifact verification; and
- recomputes and constant-shape compares the public-key fingerprint.

Private-key, path, timeout, sequence, algorithm, URL, command, and arbitrary
policy fields are rejected by the closed schema. The fingerprint detects
transfer or transcription mistakes; it is not a signature or proof of custody.

## Canonical supervisor configuration

After validation, the library renders compact supervisor schema 1 JSON plus a
single newline. Product, target, slot/state/runtime roots, initial system
sequence, and 60-second health timeout are compile-time constants. The request
cannot select them. The rendered document contains the public key and key ID
required by the existing supervisor, but no private key, rotation claim,
credential, endpoint, or activation instruction.

Four focused tests cover the accepted initial request and exact configuration,
identity/generation/replacement rejection, unknown private/path fields, invalid
encoding/point/key ID/fingerprint, empty/oversized/non-JSON input, and absence
of private or rotation fields from output.

## Host and AArch64 evidence

Full host Longrun job `f6f4b676058e4614ac795f7a5e469d5c` passed the complete
locked Rust workspace, all-feature checks, tests, Clippy with warnings denied,
systemd/source boundaries, 312 Python tests, Python compilation, BasedPyright,
frontend production build, ESLint, and formatting.

Independent remapped AArch64 workspace jobs
`56cf5fc7faf74199a6a81af84f2c7f02` and
`6e1ac72315af4e019ac53a0b96b9d5d5` passed. All six shipped runtime ELF hashes
matched across the builds. The new library archives and metadata had equal
sizes but differed in non-shipped Rust metadata. Their single 118,044-byte
ThinLTO LLVM-bitcode members were byte-identical with SHA-256:

`c70f8ff99d4a5a917adb014fd1e7b50d7563d5622363de5f478983d38a51af38`

This establishes repeatable target code generation for the public-only
contract without implying that the library is shipped in the current inert
update package.

Source `14a8dc8` then added the stateless stdin/stdout CLI with exact request
hash and explicit reviewed-public-key confirmation. Full host job
`093e24d5d64e4e88964124828c7f2cc0` passed. Independent AArch64 jobs
`1ad61b9db4724114a8f0d4b0c303178c` and
`092304f1f774476188a91f743d3e89d1` produced byte-identical 503,800-byte
stripped static-PIE executables with SHA-256:

`f468ff8eec21a28e97acb6efc4ed6c2ca2fdcced1f1a778aa9ac9bcc70682071`

Both executables passed the AArch64 ELF and host-path privacy gate. The normal
AArch64 workspace builder verifies this CLI, but the prototype bundle and
installed update-runtime package intentionally do not ship it.

## Remaining boundary

This checkpoint defines validation and rendering only. It does not identify a
production key, prove offline private-key custody, write a configuration,
initialize state/slots, enable a unit, stage or activate a candidate, expose an
update API/GUI, or change the H616. A later host provisioning tool must consume
an exact user-reviewed public request and use marker/hash/account-bound rollback
while all update units remain inert. Rotation/revocation requires a later
contract. ATX Power/Reset and equivalent attached-target power control remain
excluded.
