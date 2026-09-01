# Diagnostic journal v1 design

Status: design boundary for an unlinked pure Rust codec and reconciliation
state machine. This document does not authorize filesystem I/O, key
provisioning, collector acknowledgement, HTTP, UI, packaging, or enablement.

## Threat model

The journal protects bounded typed diagnostics against offline disclosure and
undetected modification of the persistent files. The attacker may read, copy,
truncate, replace, or replay either A/B file while the device is stopped. The
attacker does not possess the provisioned key and cannot observe or modify the
live Platform Broker process memory.

This format does not protect plaintext from a compromised root account or live
broker process. It cannot detect rollback of both valid copies to one complete
older filesystem snapshot without a separately provisioned monotonic anchor.
The A/B selector detects corruption, same-generation divergence, generation
gaps, and rollback below a caller-supplied minimum; it must not claim more.

No hostname, address, account, wall clock, path, free-form label, raw log, or
I/O error enters plaintext. Each entry contains only a local monotonic entry
ID, collector instance/window binding, omission/record counts, terminal or
acknowledgement state, and a canonical `DiagnosticBundle`.

## Cryptographic decision

V1 uses RustCrypto `chacha20poly1305` 0.11 with
`XChaCha20Poly1305`, a 256-bit key, 192-bit nonce, and 128-bit detached tag.
The crate is `no_std`; default `alloc` and `getrandom` features are disabled,
and only its key-zeroization feature is enabled. The pure journal receives a
caller-supplied nonce and performs no RNG or I/O.

RustCrypto documents XChaCha20Poly1305 as a software-friendly extended-nonce
variant and reports an NCC Group audit with no significant findings. It also
states that XChaCha has no authoritative specification, so this choice is
recorded explicitly rather than represented as an RFC-standard construction:

- [RustCrypto chacha20poly1305 documentation](https://docs.rs/chacha20poly1305/0.11.0/chacha20poly1305/)
- [RustCrypto AEAD repository and MSRV table](https://github.com/RustCrypto/AEADs)
- [RFC 8439 ChaCha20-Poly1305](https://www.rfc-editor.org/rfc/rfc8439)

Every encryption requires a fresh OS-random 24-byte nonce. An all-zero nonce is
rejected, and a write plan rejects reuse of any nonce retained under the active
key. The later filesystem adapter must obtain randomness from the OS for every
new generation and must never derive a nonce from a restartable counter alone.
The 192-bit random domain makes accidental reuse negligible, but uniqueness is
still a required invariant.

AES-256-GCM-SIV was considered because RFC 8452 is nonce-misuse resistant.
RustCrypto's current crate documentation says that the crate itself has never
received a security audit, while only some dependencies were audited. It is not
selected for v1:

- [RustCrypto aes-gcm-siv security warning](https://docs.rs/aes-gcm-siv/latest/aes_gcm_siv/#security-warning)
- [RFC 8452](https://www.rfc-editor.org/rfc/rfc8452)

The provisioned credential contains one uniformly random 32-byte AEAD key with
a nonzero 32-bit key ID and may carry one previous slot during rotation. No
password KDF is used. Both IDs and key material must be distinct so changing an
ID cannot bypass per-key nonce reuse protection. New writes always use the
active key. Key and cipher material plus transient encode/decrypt plaintext
buffers are zeroized; the typed in-memory journal is payload-redacted in
`Debug` and never appears in errors. The fixed systemd credential envelope is
specified separately in `diagnostic-key-credential-v1.md`.

## Fixed encrypted copy

Each encrypted copy has one exact size. The 64-byte clear header contains only:

- eight-byte magic and format version;
- algorithm ID;
- nonzero key ID;
- nonzero journal generation;
- 24-byte nonce;
- the fixed ciphertext length; and
- zero reserved bytes.

The whole header is associated data. A fixed 16,472-byte plaintext is encrypted
in place and followed by a detached 16-byte tag, producing a 16,552-byte copy.
The fixed size hides entry count and bundle lengths from an offline reader.

The plaintext has a 32-byte canonical header and two 8,220-byte slots. Each
used slot contains a state tag, local entry ID, collector instance/window,
omission count, record count, bundle length, and an 8,192-byte bundle area.
Unused bytes and slots are zero. Decoding must reconstruct typed records,
re-encode the entire plaintext, and compare exact bytes.

## Redundant copies and generations

Initialization writes the same authenticated generation to A and B. Every
update encrypts exactly once with a new nonce, writes the non-selected copy
first, then the selected copy. Repair copies the already authenticated selected
bytes; it does not re-encrypt or consume another nonce.

Selection rules mirror the accepted system-action journal:

- no valid copy fails;
- one valid copy is degraded and repairable;
- equal generations must have byte-identical authenticated copies;
- unequal generations must differ by exactly one;
- a selected generation below the external floor fails; and
- an encrypted copy naming an unavailable key fails closed instead of being
  treated as generic corruption.

A full rollback of both copies remains outside these rules unless a later
external floor survives that rollback.

## Entry retention and acknowledgement states

At most two entries are retained in strictly increasing local-ID order. Only
terminal `acknowledged` or `orphaned` entries may be evicted, oldest first.
`pending_ack` and `ack_ambiguous` entries are never silently evicted. At most
one nonterminal entry may exist because the collector exposes one sealed
window.

The pure reconciliation planner enforces write-before-ack and accepts only a
previously decrypted and selected journal:

- a new sealed binding plans a durable `pending_ack` write;
- the same durably selected pending binding permits one acknowledgement;
- an ambiguous acknowledgement plans `ack_ambiguous`, then requires a read;
- rereading the same sealed binding plans a durable return to `pending_ack`
  before another acknowledgement can be permitted;
- observing no sealed window cannot distinguish an applied ambiguous
  acknowledgement from collector restart, so it plans terminal `orphaned`
  rather than claiming acknowledgement;
- a different binding plans the old nonterminal entry as `orphaned` before the
  new binding may be stored; and
- mismatch plans another collector read without mutation.

The planner cannot create an acknowledgement action from a newly constructed
or merely prepared journal. Only state returned by authenticated A/B selection
can authorize it. The future adapter must still enforce write, readback,
file/directory sync, and re-selection before invoking that action. The unlinked
store now enforces those persistence steps; no live caller invokes the action.

## Deferred boundaries

The first implementation remains a pure `no_std` crate. Sibling unlinked layers
now supply fixed-root atomic storage, OS randomness, and canonical systemd
credential parsing. Still-deferred, separately gated work includes a production
key/rotation ceremony, persistent credential protection, native store gate,
live broker coordination, acknowledgement ambiguity recovery across process
crashes, external rollback floor, authenticated HTTP, UI, retention controls,
packaging, privacy review, soak, and reboot.
