# Signed application upload boundary

The network integration remains separated into status, upload/staging and
explicit activation. Upload does not restart services, change the current slot,
write the update journal, advance the rollback floor or dispatch apply units.
The existing status request enum and socket remain read-only.

## Source implemented so far

`ApplicationSlotStager::stage_reader` accepts a reader and an exact archive
length rather than a client-selected path. Invalid zero/oversized declarations
are rejected before reading. Copying consumes at most the declaration plus one
byte; short and surplus streams fail. A private, identity-checked, read-only,
unlinked snapshot feeds the same verifier and immutable atomic publication path
as local file staging. Repeating the same verified archive returns the same
slot as `AlreadyPresent`. Publication is not activation.

`stage_reader_with_quota` additionally limits new publication by verified slot
count and total logical payload bytes under the same exclusive root lock.
It checks the candidate and existing slot identity before applying the quota,
so a verified retry works at capacity. It neither removes slots nor changes
the current pointer. Invalid policy is rejected before consuming the reader.
This logical quota is not a filesystem free-space guarantee: the upload worker
still needs a physical-space check including spool/snapshot/extraction overhead.

The independent `bli-kvm-update-protocol::upload` metadata codec describes one
length-prefixed JSON request, raw archive bytes, write-half EOF and one framed
JSON response. The only request is `stage` with `archiveBytes`; no name, path,
URL, command, unit or activation flag is accepted. The wire limit is 128 MiB.
The metadata frame retains the existing 16 KiB cap. Schema and nonzero request
IDs are checked on both encode and decode. Responses carry either a closed
error code or a signed-slot identity, version, sequence, minimum system
sequence, disposition, manifest digest and payload byte count. The digest must
match the digest component of the validated slot ID. HTTP must serialize all
64-bit counters as decimal strings, as the status boundary already does.

## Required transport integration, not yet implemented

- Authenticate the HTTP request and enforce the existing same-origin/CSRF
  policy before consuming the archive or opening updater IPC. Do not accept a
  user-selected upstream URL. Enforce an exact content length and media type;
  reject encoded, multipart and surplus bodies unless explicitly specified and
  tested later. Stream with backpressure rather than buffering 128 MiB in RAM.
- Use a different Unix socket and worker entrypoint from status. Authorize the
  server's kernel peer UID before reading metadata. No network listener runs as
  the updater identity. Keep socket access, writable directories and services
  explicit in installed unit/configuration templates.
- Receive to a private anonymous spool **before acquiring the slot-root lock**.
  A slow HTTP upload must not hold the lock needed by watchdog and status.
  Bound simultaneous uploads, total wall time and idle time independently.
  Closing the transport before complete body/EOF must not publish a slot.
- Enforce installed slot-count and disk-reserve quotas under the existing
  slot-root lock, including snapshot and extraction peak usage. Preserve
  idempotent uploads at the quota boundary; never automatically delete active,
  confirmed, previous or otherwise protected slots to make space for an upload.
- Reuse the verified stager, public trust and safe root validation. Transport
  and verifier size limits must have an explicit consistency test. Never call
  `activate`, `initialize`, `confirm`, watchdog or apply from the upload worker.
- Send only validated, sanitized receipts/errors. Preserve a published slot
  after a lost response; retrying the same archive must identify it safely.
  Client cancellation after publication is not rollback or proof of failure.
- Keep writable-update capability disabled until upload **and** the separate
  explicit activation flow pass host, installed native and browser gates.

## Acceptance still required

Host integration must cover authorization-before-body, UID rejection,
malformed/truncated/oversized metadata and bodies, idle and total deadlines,
concurrency and disk quotas, untrusted signatures, interrupted receipt delivery,
idempotent retry, spool cleanup, immutable slot publication and unchanged
serving/journal state. Native acceptance must additionally prove the actual
service identity, sandbox, storage limits and watchdog/status availability
during a slow or interrupted upload. Browser acceptance must cover progress,
failure/retry, logout/cancellation and a clear separate activation confirmation.

The current stream and codec tests do not establish any HTTP, installed-worker
or browser-upload acceptance. No upload endpoint is enabled on the device by
these source changes.
