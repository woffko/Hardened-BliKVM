# Persistent status boot restoration: first attempt not accepted

The online persistent-wiring rehearsal passed on source `0ef069b`: preflight
`8d48fc93b14c43d38ee1c686de9856ad`, guarded preparation
`ddcacb05911a468bb4748264580ca93f`, enabled browser
`0587a6f28568454ab6afd54c3fef52b1`, online restoration
`74d12057b6e843a5af6035b5e11cb607`, restored browser
`ed19ee32fa4a4be4999bba9c53082fe4`, and cleanup
`39852b2c9af84d78b0a30d3f9d4ba179` all passed.

Boot-test source `bb601d6` passed host checks and preparation
`4f860704966b408982b0d6736495d88a`. Job
`f244942de3a8437e879f217c476cdab2` sent exactly one accepted software reboot,
but did not obtain full restoration proof within its bounded window.
Read-only diagnosis `e59aa9cc06614667bfae852ee721d1b8` proved a changed boot
identity, removed server drop-ins, a restored marker and running server/Nginx.
The boot receipt was recorded at 17.422 seconds, before server start at 42.290
seconds. Nevertheless the status socket was still active, so acceptance failed.

The evidence is consistent with a queued socket start surviving removal of its
Wants drop-in and daemon-reload. The original controller intentionally avoided
synchronous socket operations during early boot. The correction cancels the
socket job with a non-blocking stop without waiting on an ordered-after unit.
Committed-state recovery still preserves the socket and persistent wiring.
Boot-proof schema two records this cancellation and rejects schema-one evidence.

Online recovery `1c2aff776bac4dd68bd1453014b0dd88` subsequently restored the
disabled/inactive socket and verified protected state. This manual restoration
does not turn the failed boot attempt into a success. A fresh boot test with
the corrected controller is still required before persistent retention.

Failed-test cleanup `b5a47e43a2a04e729239e94d7055ac83` removed the exact guard
files after online restoration and verified the runtime. Host failure receipts
and the sanitized boot diagnosis were retained. No further reboot was issued
as part of this recovery or cleanup.
