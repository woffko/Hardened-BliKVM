# Bounded diagnostic window policy checkpoint

On 2026-08-31, source commit
`a00e1a8eb5942d83cd5590b39fbcf9c5cf055fbf` completed the first pure Rust
diagnostic window and handoff state machine. The
`bli-kvm-diagnostic-window-policy` crate is `no_std`, forbids unsafe code,
depends only on the three previously verified pure diagnostic crates, and is
not linked into any server, broker, process, package, API, UI, or device
runtime.

The manager owns one live collector and at most one sealed unacknowledged
bundle. Window IDs start at one, advance without wrapping, and acknowledgement
must match the exact sealed ID. A sealed bundle remains byte-stable while the
next live window accepts events. If that live window also needs rotation before
the sealed bundle is acknowledged, producers receive explicit backpressure and
no state advances.

Each accepted local connection is represented by an instance-bound session.
The manager assigns a nonzero source epoch, creates `Started`, owns per-window
sequence and relative monotonic offset, and creates `Stopped` on disconnect.
Only one session per source is active. Duplicate, stale, cross-instance, and
exhausted sessions fail closed. Rotating a window deterministically re-emits
`Started` for every still-active source at offset zero without exposing epoch
or sequence in output.

The capacity invariant is:

`live record count + active source count <= 32`

One `Stopped` slot therefore remains reserved for every active source. A full
live window can close all five sources even while another window waits for
acknowledgement. Producer events are committed through clone-then-swap: accepted
advances exactly one state, while event-policy, backpressure, or unavailable
responses leave the original manager unchanged.

The exact 24-hour boundary is accepted. Greater age forces rotation. Clock
rollback rejects producer events without mutation. EOF is different: a peer
must never remain logically connected after its stream disappears. If an
overdue or rollback-affected disconnect cannot record `Stopped`, the source is
released, a bounded omitted-lifecycle counter makes the live window visibly
incomplete, and every subsequent write requires rotation. No lifecycle loss is
silently presented as a complete bundle.

The state is intentionally ephemeral support diagnostics, not an audit log.
Manager restart starts at live window one with no sealed bundle or sessions.
An internal nonzero instance binding prevents stale session tokens crossing a
manager replacement when the runtime supplies a fresh value. No persistence,
filesystem, identity resolution, socket, process, clock read, network, device,
path, text, or arbitrary byte interface exists.

Eleven focused Rust tests cover:

- exact initial state and nonzero instance binding;
- connect, recorded disconnect, reconnect epoch, and stale-session rejection;
- accepted event output plus state-neutral role/lifecycle rejection;
- event clock rollback and visible incomplete EOF release;
- five active sources, lifecycle reservation, atomic capacity rotation, and
  hard second-window backpressure;
- exact sealed acknowledgement and reuse of one future rotation;
- accepted maximum age and overdue backpressure;
- overdue EOF with a pending seal, omission metadata, and forced recovery
  rotation;
- window-ID and source-epoch exhaustion;
- manager restart and cross-instance token rejection;
- deterministic snapshots for identical collector-owned inputs.

The full host suite passed with 120 Python tests plus workspace
format/check/test/Clippy with warnings denied, source, packaging and systemd
boundaries, BasedPyright, production TypeScript build, ESLint, and Prettier.
Two attempts to open the new Rust file through the project LSP hit its
100-document limit, so LSP diagnostics are not claimed; compiler and Clippy
checks passed.

Two clean independent AArch64 workspace/bundle builds produced byte-identical
4,778,548-byte prototype archives:

`c4750b529dbb3bb03e8a72d84f4518bebf130e0d209b0a92bb070dacc3045254`

The unshipped window-policy rlibs were byte-identical at 101,736 bytes:

`9145c1b345592dc07838859f701777de8fffb382282afa73353be8d39a64f335`

Each window rlib contained one 87,248-byte AArch64 `lib.rmeta` member,
byte-identical with SHA-256:

`fff6540af869d15ec274680ea66fd468c68615d42b92fb09d58f4d5af6191b4e`

The single extracted 13,532-byte window ThinLTO LLVM-bitcode member was
byte-identical with SHA-256:

`2958e2a30791f196854c8d129259d08eb690969a023a55a20b058f7a775c854d`

Protocol, collector-policy, and export rlib, metadata, and ThinLTO pairs also
remained byte-identical with their previously recorded hashes. Direct byte
comparisons passed for both archives and every layer. The archive manifest
identifies the exact source commit and retains `releaseAccepted=false` and
`deviceAccepted=false`. No diagnostic crate is present in the prototype.

No device test was run because the state machine is pure and unlinked. No live
log, configured path, process output, identifier, or device state was read.
There is still no `SO_PEERCRED` adapter, Unix socket, collector process, real
monotonic clock, server export protocol, authenticated API/UI, systemd unit,
package, or advertised capability. Runtime restart, concurrency, timeout,
confinement, privacy, browser, and H616 gates remain mandatory.

Peer-policy source `fcfde44543b64f1a93eec05ea47c9f92ea6efe7b`
now supplies the pure source mapping intended to precede `connect`; it does not
link a socket or runtime. See `diagnostic-peer-policy.md`.

Adapter source `b62d827c107d035a9b1580476fc7662054e4e315` now provides the
safe source intended to precede `connect`; it remains unlinked from this state
machine and from every process. See `diagnostic-peer-adapter.md`.

Runtime-core source `6de71c28df1eb521108a955d046e098feb3bacb3` now composes this
manager with the authenticated peer token and exact producer frames. The
private session no longer implements `Debug`; updated AArch64 metadata and
reproducibility evidence are recorded in `diagnostic-runtime-core.md`. No
shipped process links the composition.

Listener source `89050939af218ddbdd7143fb63e320e1ca9eac67` now supplies the
real monotonic tick owner, serializes time under the policy mutex, and exposes
only bounded snapshots/exact acknowledgement; see
`diagnostic-listener-runtime.md`.
