# Retained service resource baseline

On 2026-09-08, read-only SSH job `084dd80d4c2843c597392128db30a42c`
completed 61 snapshots over 615.220 seconds, with a ten-second sleep between
snapshots. The observer did not generate HTTP, status or video workload. This
does not exclude independent user activity, and is not a loaded soak result.

The selected link and running server executable were bound to confirmed
application sequence five. Boot identity, each service's main PID/start time,
and restart counters remained stable. Cgroup membership and process identities
were checked before/after each sample. No device files, configuration or
services were changed by the observer. No environment, command-line or open
descriptor contents were read.

| Service | Peak RSS (KiB) | Peak FDs | Peak threads | Peak processes |
| --- | ---: | ---: | ---: | ---: |
| Current application server | 1,876 | 7 | 5 | 1 |
| Platform broker | 784 | 7 | 5 | 1 |
| Video proxy | 524 | 7 | 5 | 1 |
| Nginx | 14,300 | 19 | 2 | 2 |

RSS and FD growth were zero in all four services. Their cgroup `usage_usec`
counters were unchanged across the observation, yielding 0.000% mean and
maximum interval CPU in single-core units. These are sampled observations,
not a proof of leak freedom or a performance budget. RSS is summed across
cgroup processes and may count shared resident pages more than once. CPU
intervals use sequential snapshots rather than an atomic whole-system sample.

Observer source SHA-256:
`c836ebe72f2fd45d6d7316c9adc48eed6d94e0fc250cdbb2be7f0537536af4e1`.
The private receipt is `evidence/private/retained-resource-baseline.json`;
the host recomputed its summary and validated the bounded numeric schema.
The complete host gate passed in job `8a7b10e35a0046f29011c1669fc7ba43`.
Six focused regressions cover CPU scaling, summary/identity/clock failures,
strict receipt validation, process-stat parsing, cgroup aggregation/races,
and the observer's read-only boundary.

Not covered: transient status workers, KVMD/uStreamer/Janus, whole-system
resources, temperature, concurrent authenticated status/video workload,
long-duration stability, or the plan's physical/24-hour acceptance gates.
Loaded observation and deliberate status-unavailability acceptance remain
separate required work.

## Backend IPC load observation

`tools/capture_retained_backend_load.py` performs a separate observation of one
continuously drained MJPEG IPC connection plus 61 serial status IPC reads.
The existing four-service sampler runs concurrently with that workload. The
controller runs the workload child as the server UID and shared group with no
supplementary groups and an empty environment. Source is supplied in memory;
the probe does not stage device files or change services or configuration.
Only that child is terminated if it fails to exit within its cleanup deadline.

Status framing requires request write-EOF; video framing must keep the write
side open because the proxy treats EOF as cancellation. Both clients have
bounded absolute request deadlines. The workload checks the confirmed
sequence-five state on every read and requires increasing video byte counts
between samples. Receipts contain counters and signed status metadata, never
video payloads. The host independently validates both resource and workload
summaries and requires successful child exit.

This scope excludes authenticated HTTPS/server/nginx load, frame decoding,
transient status-worker resource peaks, uStreamer resource use and 24-hour
stability. It sets no new CPU/RSS performance budget.

On 2026-09-08, device job `fdcf189ae8a84473a6e9bd7060659cbb` completed
successfully: 61 resource snapshots over 647.089 seconds, with 61 stable status
responses and 17,029,948 video-transport bytes consumed. Byte counts increased
at every sample; the longest status request took 814.562 ms. The workload
child exited successfully. These measurements use the device's monotonic
sample clock, not the job runner's duration counter.

Boot identity, main process identities and restart counters remained stable.
All four services had zero sampled RSS and FD growth across the loaded
interval. Peak RSS was unchanged from the preceding no-generated-workload
baseline. The video proxy had nine FDs during the loaded interval, versus
seven in that earlier baseline; its mean CPU was 0.026% of one core and its
maximum interval CPU was 0.038%. The other three services' sampled CPU counters
did not advance. This is consistent with bypassing the HTTP server/nginx path,
not evidence that those services are cost-free under browser workload.
No post-child-exit resource snapshot was taken, so this result does not claim
that the proxy's FD count returned to seven after disconnect.

The host revalidated the complete receipt and recomputed both summaries. Its
three source hashes matched the exact local modules:

- `observe_service_resources.py`:
  `c836ebe72f2fd45d6d7316c9adc48eed6d94e0fc250cdbb2be7f0537536af4e1`
- `retained_load_ipc.py`:
  `982204b071b5aa3d41263924d77f13473c510254ae53ef824437c277f4fc6cf6`
- `observe_retained_backend_load.py`:
  `0f892cbc3987039995b83018158dd4696a0dbaf8402cb08a078919490349bd46`

Private receipt: `evidence/private/retained-backend-load-resources.json`.
Full host verification passed in job `2b265f6fce5347dab72006173f347231` before
this device run. Fourteen focused tests cover framing/deadlines, retained-state
validation, resource/workload summary integrity, video reader termination,
in-memory source loading, least-privilege process launch and owned-child cleanup.
