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

## Host worker implementation

The `upload-socket --peer-user USER --spool-root PATH` CLI entrypoint now
authorizes the kernel Unix peer using the same identity checks as status,
before config, admission or body access. It loads only trusted public config,
then locks an independent updater-owned 0700 spool directory. Component-wise
`openat` rejects symlinks and pins the directory. `O_TMPFILE` creates an anonymous
0600 file with zero links; unsupported filesystems fail closed. No named spool
is left after success, failure or process exit.

After complete body and EOF, the worker rewinds the spool, opens the stager
lock, checks staging filesystem headroom, and stages with the 8-slot / 512 MiB
logical payload quota. It never opens the update manager or health store.
Preliminary physical-space checks account for spool, snapshot and extraction,
allocation/inode overhead and 64 MiB free headroom. They are availability
checks, not reservations against unrelated writers. Physical exhaustion can
still fail a retry that would fit the logical quota; even a retry needs spool
and verification snapshot space.

A real CLI socket-pair test under the unprivileged host UID covers signed
publication, same-archive retry, invalid archive rejection, empty spool and
untouched state/runtime directories and current pointer. This does not yet
prove installed-worker identity/sandbox/lifetime or HTTP/browser acceptance.

The host CLI test also observes the child's anonymous spool during a partial
upload and acquires the slot-root lock independently, then proves disconnect
does not publish the incomplete candidate. A complete upload with a deliberately
lost response is recovered as `AlreadyPresent` on retry.

Inert updater package revision `0.1.0~dev8` includes separate upload socket and
service templates; dev1-dev7 file maps remain unchanged. The socket admits one
connection. The service uses a disk-backed private state directory (not `/run`
tmpfs), `Type=simple` with a 180 s process limit and 5 s stop timeout, bounded
memory/file size, and write access only to the slots and spool. No journal or
health directory is writable. Offline systemd syntax/security verification and
package-map tests cover these templates; no service is enabled by packaging.
An installed filesystem/sandbox check is still required, especially anonymous
file support and timeout behaviour under the actual service identity.

## Host HTTP transport

The opt-in `BLI_KVM_UPDATE_UPLOAD_SOCKET` configures
`POST /api/application-update/upload`; it requires a separately configured
status socket. Both are disabled by default. The protected router checks the
session, CSRF token and existing origin policy before body polling or IPC.
The handler requires a single exact decimal `Content-Length` of 1–128 MiB and
one `application/octet-stream` content type. Content/transfer encoding,
multipart, declared or actual trailers and short/surplus bodies are rejected.
It adapts HTTP frames to bounded reader chunks without collecting the archive.

The separate stage-only client admits one request, bounds connect/metadata to
2 s, individual body reads/writes to 4 s, body transfer to 115 s and the whole
operation including receipt to 175 s. It deliberately withholds the final
archive byte until HTTP EOF is validated: otherwise closing IPC on a surplus
body or trailer error could impersonate a valid archive followed by EOF.
Session expiry/revocation is checked during body reads and every 100 ms even
while stalled; cancellation drops IPC and releases admission. Cancellation
after complete delivery/publication cannot undo a staged slot. An unavailable
response is not proof that publication failed; retry the same archive.

Host tests cover authorization/headers before body or IPC, exact signed-slot
receipt serialization without u64 rounding, raw stage-only IPC, malformed
headers/trailers, incomplete/surplus input, cancellation/admission, logout
during a stalled body and oversized/mismatched/surplus receipts. The successful
HTTP route serialization test uses a fake IPC peer, not the installed updater.
These checks do not establish native or browser acceptance. Writable-update capability remains
disabled; the upload does not expose an activation endpoint.

An additional host integration now connects the real authenticated Axum route
and streaming client to a subprocess running `upload-socket`, using synthetic
signed archives and disposable unprivileged roots. It verifies publication,
same-slot retry, rejection of an untrusted signature and short/surplus bodies,
empty spool/state/runtime directories, unchanged slot-root layout after the
first publication and no current pointer. This is not an installed systemd
sandbox or a real TLS/reverse-proxy test. Client tests also isolate idle,
transfer and overall response deadlines, and prove that an upstream error
after the last declared byte still withholds that byte from the worker.

## Browser UI source

The Bli update panel now includes a capability-gated upload component with a
single File request, explicit upload action, progress, verification wait,
cancel/retry and a validated staged-slot receipt. Upload does not send multipart,
paths, URLs or activation commands. A separate confirmation flow now handles
generation-fenced activation; see `../api/application-activation-v1.md`.
The browser supplies the exact File length.
Zero/oversized files are blocked locally as well as by the server. Decimal
counter and slot/hash identity validation is repeated before rendering success.
100% transport progress is labelled as awaiting verification, never installation.
Cancellation or response loss warns that staging may have completed and retains
the same File for an idempotent retry. Unmount/capability loss aborts the request
and stale callbacks cannot overwrite a newer transfer. English and Russian
messages explicitly separate staging from activation.

This component remains hidden under the current read-only capability. No
source change here enables writable update support on the device. The bounded
`tools/playwright/local_application_upload.mjs` fixture exercises the real
component and browser HTTP adapter against synthetic local receipts; it is not
an installed updater or signature-verification acceptance test.

The local Chromium scenario passed with seven browser-visible sends: publish,
lost response, retry, cancel, retry, rejection and unmount cancellation. In the
lost-response case Chromium performed seven transport attempts for one logical
POST; assertions distinguish those retries from duplicate UI submissions.
Exact raw body, browser-generated length and CSRF header were observed, along
with the verification wait, decimal u64 receipt, 390 px layout and capability
gate. Native HTTP-to-worker and explicit activation acceptance remain required.

## Remaining installed and browser transport integration

The supervisor library now provides a bounded reception primitive: it checks
the closed metadata before creating a sink, copies in 64 KiB chunks, requires
exact body length and EOF, and drops the owned sink on failure. Header, idle
and absolute total deadlines are independent; progress does not reset the
total deadline. Its defaults are 2 s / 5 s / 120 s. Socket-pair tests cover
truncation, surplus, missing EOF, stalled metadata, continuously progressing
uploads exceeding total time, and sink destruction on failure. This primitive
does not itself authorize peers or create the private spool; the CLI wrapper
now performs those steps. Blocking local filesystem calls additionally
require an installed worker process lifetime bound.

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

The host tests do not establish installed-worker or browser-upload acceptance.
No upload endpoint is enabled on the device by these source changes.
