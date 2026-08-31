# Pure system-action journal checkpoint

On 2026-08-31, source commit
`b9106ab6263edb3c56553fe8b0f849c57931af15` completed the storage-neutral
system-action journal model. The `bli-kvm-system-action-journal` crate is
`no_std`, forbids unsafe code, depends only on the system-action policy and the
no-default-features SHA-256 implementation, and is not linked into any server,
broker, package, service, filesystem adapter, or actuator.

The policy crate now exports and restores explicit record snapshots only
through its existing structural validation. That bridge does not add a public
enabling constructor and cannot create a dispatch intent. Invalid IDs,
bindings, timestamps, phases, duplicates, or multiple pending records remain
rejected before they can become policy state.

The journal uses one fixed canonical representation:

- a 2,026-byte policy payload: 10-byte schema/generation header plus sixteen
  fixed 126-byte record slots;
- a 2,076-byte journal copy: 18-byte magic/schema/generation header, the policy
  payload, and a 32-byte checksum;
- big-endian integers, explicit enum and option tags, zero-filled absent fields,
  and rejection of unknown or noncanonical bytes;
- magic `BLKSAJ01` and the domain
  `hardened-blikvm-system-action-journal-v1` for SHA-256 separation;
- journal generation exactly one greater than the embedded policy generation.

Decoding verifies length, magic, versions, SHA-256, canonical re-encoding, the
generation relationship, and all policy invariants. The checksum detects torn
or corrupt records. It is deliberately not claimed as authentication against a
writer with root-equivalent access.

Two-copy selection accepts one valid copy only as degraded state. Equal
generations must contain equal policy state. Same-generation divergence is a
split-brain hard stop, and unequal generations may differ by exactly one only.
Callers must provide an external minimum generation; selection below that floor
is rejected as rollback. The model cannot make that externally stored floor
physically rollback-resistant.

Initialization plans generation one in A-then-B order. An update carries the
expected prior generation, requires exactly the next policy generation, and
writes the non-selected copy before the selected copy. This preserves one old
valid generation before the first verified replacement and one new valid
generation before the second replacement. A repair plan copies only the
selected generation into the invalid/older side. Exact canonical readback is
required by the verification helper. These are write plans only: no replacement,
sync, lock, or readback I/O exists in the crate.

Ten focused journal tests cover:

- canonical fixed-size round trips for empty, prepared, dispatching, accepted,
  and rejected policy states;
- every one of the 2,076 single-byte corruption positions;
- checksum-valid noncanonical slots, unknown actions, and generation mismatch;
- initialization and older/non-selected-first update ordering;
- torn first-copy fallback and torn second-copy recovery of the new state;
- split brain, generation gap, rollback floor, and zero-valid-copy rejection;
- repair of one degraded copy without advancing state;
- stale expected generation and skipped policy generation rejection;
- exact readback verification and generation exhaustion;
- public fixed-layout constant relationships.

The policy suite now has thirteen focused tests, including snapshot round-trip
through policy validation. The full host suite passed with 90 Python tests plus
workspace format/check/test/clippy, source, packaging and systemd boundaries,
BasedPyright, production TypeScript build, ESLint, and Prettier. Two attempts to
obtain LSP diagnostics for the new journal file hit the project server's
100-document limit, so new-file LSP diagnostics are not claimed; compiler and
Clippy checks with warnings denied passed.

Two clean independent AArch64 workspace/bundle builds produced byte-identical
4,777,145-byte prototype archives:

`e570c4c20fe7b9a3d422a25a234a3aea3579e8f8809db436f98e4bed7fd737dc`

The updated unshipped policy rlibs were byte-identical at 213,992 bytes:

`0621cc3b181fe4ae606ea525c299978c9ddfa4abaa8554f7d299c1e4209a7de3`

Their 175,520-byte AArch64 `lib.rmeta` and 36,664-byte ThinLTO member were
byte-identical with SHA-256, respectively:

`6c452bd889e35aaefa9c882db6e6b3fb8fcb51aa1192bfb7e65f6acac4a8969b`

`ca55d0691e1128778751a5d75530a424faf93b80cf2583ee3ee54370a793be13`

The unshipped journal rlibs were byte-identical at 110,602 bytes:

`ea45ec0e67eaab8bc6a0d2473a7413b7a018f54afbab41ac563e6446cfc197f5`

Their 71,688-byte AArch64 `lib.rmeta` and 37,784-byte ThinLTO member were
byte-identical with SHA-256, respectively:

`ef6e4576784ee3ce4c3a0a85c08a2ae8b0919df391fd71845210eba059210cb5`

`c76a3d82d04fb71618cbca280fc522521db715125f728ea453987110aa9bef42`

Direct byte comparisons passed for both archives, both rlibs, both pairs of
metadata members, and both pairs of codegen members. Neither the policy nor the
journal is present in the prototype archive. Its manifest identifies exact
source commit `b9106ab6263edb3c56553fe8b0f849c57931af15` and retains
`releaseAccepted=false` and `deviceAccepted=false`.

No device or system action was executed. A future adapter still needs an exact
root-owned path, no-follow and owner/mode checks, exclusive locking, atomic
same-directory replacement, file and directory sync, exact readback, bounded
recovery, an independently justified generation floor, crash/fault injection,
systemd confinement, and separate device acceptance. Until then the journal is
not durable storage and reboot/shutdown remain unavailable.
