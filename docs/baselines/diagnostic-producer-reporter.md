# Bounded diagnostic producer reporter checkpoint

On 2026-08-31, source commit
`5ccd68b737a7735d3fda0345a83af80858b0c0f4` completed the first bounded
non-blocking reporter around the unlinked typed producer client. No server,
broker, proxy, supervisor, unit, package, API, UI, or device runtime links the
reporter or client.

`try_report` performs one fixed event validation and one non-blocking
`try_lock`. It returns `Queued` or a typed invalid/busy/full/closed drop receipt;
it never waits for the collector. One fixed ring contains exactly eight
`Option<DiagnosticEvent>` slots. Events allocate no task, string, vector, map,
box, path, label, or identity.

One Tokio worker drains the ring in order through the verified producer client.
It never retries a not-sent or delivery-ambiguous result. Fixed saturating
atomic counters distinguish queued, four drop classes, accepted, three
collector rejections, not sent, delivery ambiguous, worker failure, and forced
shutdown abort. The snapshot contains no timestamps, raw frames, response
bytes, paths, identities, arbitrary keys, or text.

Closing the owner rejects every later handle submission and lets the worker
drain already queued events. Shutdown waits at most three seconds; a stuck
worker is aborted, which uses the client's cancellation-safe stream ownership.
Dropping the owner closes the queue and aborts immediately, so diagnostics
cannot retain process liveness. `Notify::notify_one` preserves a wake permit
across the empty-queue/close race. Starting outside a Tokio runtime fails
without creating a worker.

Eight focused reporter tests, in addition to the eleven transport tests, cover:

- every accepted/rejected/not-sent/ambiguous semantic counter with one call
  per event and no retry;
- a real client collector-outage result counted once;
- invalid, busy, full, and closed non-blocking receipts;
- a full ring behind a stuck worker and bounded shutdown abort;
- fixed-ring order plus drain-on-close;
- owner drop abort and handle closure;
- worker panic converted to one bounded counter; and
- saturating counter behavior at `u64::MAX`.

The complete 19-test crate passed twelve additional consecutive runs. The full
host suite passed with 134 Python tests plus locked workspace format/check/
test/Clippy, source/packaging/systemd boundaries, Python compilation and
BasedPyright, production TypeScript build, ESLint, and Prettier. Two LSP
diagnostic attempts hit the project server's 100-document limit, so LSP
diagnostics are not claimed; compiler and Clippy checks passed.

The lock graph remains 156 packages: 48 workspace and 108 external, with no
missing external license expression. No dependency was added. `cargo audit
--deny warnings` loaded 1,233 advisories and accepted the exact graph.

## Independent AArch64 builds

Two clean independent workspace/bundle builds produced byte-identical
4,781,022-byte prototype archives:

`08ab47c9c4bd5c5f3b3ba168cadd0106b1e66df1838f9fe47d17ea6eabba4813`

Neither archive contains a producer-client or reporter artifact. Their
manifest identifies the exact source commit and retains
`releaseAccepted=false` and `deviceAccepted=false`.

The updated unshipped producer-client rlibs were byte-identical at 265,892
bytes:

`5008d6deeb3eac192991501c4350bcbb9580ab06b9ad0342751e0e8e4a4b1eb8`

Each contained one 158,624-byte `lib.rmeta` member with SHA-256:

`264e7854bc20a47de26792555f5a63c2f8c6dc863e4bb4cccc3beac0a0b2de3c`

The single 105,256-byte ThinLTO member matched with SHA-256:

`e9a42b395cd27adc32ad1650d2936013e0dde9edfb55a7ced92f1aeea0657e6e`

Protocol and export rlib/member pairs remained byte-identical with their
previous hashes.

No device test was run because no real process links this boundary. The next
checkpoint may integrate only the deny-only control broker. It must prove that
control responses complete independently of reporter saturation, collector
outage, ambiguous delivery, and reporter shutdown before any package or
capability is accepted.
