# Status outage: first browser attempt and recovery

The first guarded device attempt used host commit `5872439` and Longrun job
`e3c2cb15ba9549fe890b392a95b25b32` on 2026-09-08. It did not pass browser
acceptance. Operation `d2d2b586ead97917f39005ab` stopped only the status socket,
verified worker drain, restored the socket and verified unchanged application
status and serving identities. Browser evidence contained three expected
status API 503 responses and one unexpected MJPEG 503. No control requests,
WebSockets or page errors were recorded. The exact failed browser assertion
was not captured, so intermediate GUI acceptance must not be inferred.

The separate recovery job `b9c2f8872b5e44cf81a97038a10a3d65` passed: fixed
socket restoration, original status/serving identities, and removal of this
operation's guard were verified. The original failed receipt is preserved;
the recovery receipt resolves its historical `guardMayRemain: true` flag.

A subsequent read-only browser diagnostic, job
`3165e80492b64e3cbfb610fdabbcdaec`, failed the original-response continuity
assertion before opening a second page. It recorded four successful primary
video responses before that phase, no additional responses during the phase,
and no HTTP 5xx responses. No status outage or restore timer was used.

## Controlled local reproduction

`tools/playwright/mjpeg_completion_probe.mjs` served synthetic alternating red
and blue JPEG frames to the installed Chromium. `Response.finished()` resolved
while the server connection remained open and both colors continued decoding.
One run sent 14 frames; CDP reported 11,508 bytes, including 10,686 bytes after
the completion signal. Navigation then closed the server connection and emitted
`Network.loadingFailed` for that request.

The browser harness now checks new CDP `Network.dataReceived` bytes for the
same request and rejects replacement, failure, or a bounded lack of progress.
Reload/logout checks observe request cancellation independently. Unit tests
cover first-part completion, later byte progress, replacement, stall and
disconnection. This repairs an invalid test invariant; it does not excuse or
explain the separate unexpected MJPEG 503.

A separate Rust test also verified peer EOF after cancellation and timeout
before response headers. The suspected pre-header connection leak was not
reproduced, and no video-client production code was changed on that hypothesis.

The two-page video diagnostic and real GUI stale/error/recovery gate still
need to pass with the corrected observer. No broader device, hardware, image,
or release acceptance is claimed here.

The complete host check for the corrected observer passed in job
`c24b95999f55455ab25618cbc610c6da` (exit 0).

## Second read-only diagnostic and source fix

Job `ee19be6ad0634f35be256e251abae23f` completed the two-page continuity
checks with the same primary response and continuing bytes. The secondary page
received one video 200 and one video 503. Logout returned 200, but the observer
timed out waiting for cancellation, so the whole diagnostic still failed.

The local fixture's `graceful` mode established a separate normal-EOF path:
the first `Network.loadingFinished` event has encoded length zero, while normal
EOF emits a later completion with positive encoded length and no
`Network.loadingFailed`. The observer now distinguishes that pair from the
early first-part event; cancelled streams still use `loadingFailed`.

The fixture's `contention` mode reproduced the secondary 503 with a two-stream
budget. Chromium keeps the old image stream until the replacement has a first
frame; the replacement waits for a slot occupied by that old stream. The
primary stream continued receiving frames during this reproduction.

`Mjpeg` unnecessarily incremented its URL nonce in its initial effect, even
though the initial src already started a request. Its input snapshot now skips
that redundant initial replacement, while preserving dependency-triggered
reconnects and frame-detection behavior. The isolated React-component fixture
failed against the old component (initial nonce 1 instead of 0) and passed
against the source fix: exactly one initial request, resolution/writability
reconnects, error reset, and the expected frame-detection calls.

The component fix has not been installed on the device. The corrected logout
observer and device-level two-page/outage acceptance remain pending. This fix
does not claim unrestricted concurrent resolution changes with a full proxy.

The complete host check for the component fix passed in job
`8aa3185a029d45b2ab6b79c03d3d6a3e` (exit 0). Installation remains pending.
