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
