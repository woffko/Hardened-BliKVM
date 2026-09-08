# Persistent read-only status acceptance

The device retains exact root-owned current/candidate server drop-ins for the
installed local status socket. The socket remains disabled as a standalone
boot unit; the servers' Wants/After dependency starts it. The worker retains
its read-only sandbox and fixed peer-UID policy. No upload or activation
authority was added to the network process.

The accepted prerequisites are the complete reversible desktop/mobile cycle
in `status-api-seq5-acceptance.md`, the real corrected early-boot restoration
in `persistent-status-boot-attempt.md`, and the retained updater dev7 reboot
in `updater-dev7-package.md`. Earlier failures remain documented separately.

Retention preparation `00487692b1c244af829dd5226f907710`, enabled browser
`a1b1c226b5ec464a81f5d0d77917ce91`, commit
`ace7bb36f5f84a12be5a8cc196320234`, cleanup
`c8b8ec64c42e47778777a28bee2d4992`, and independent observation
`9ecb03e7430648f5868ea6c01c48f87b` passed. The first later reboot exposed an
initial-recovery startup failure, so that attempt was not accepted.
After the dev7 ordering correction, reboot
`ae63cbf258b146b0bbb2f4b0f7321224` and post-boot browser
`c0ae27a7ace24bf0a1dd50e51061fe0a` passed with no manual service start.

Current accepted application identity is sequence five, version `0.1.0-dev4`,
floor five and journal generation twenty-two. API counters remain decimal
strings; selected and confirmed metadata match; no update is pending. The
desktop/mobile Update pane shows read-only status and Refresh. Login, video,
Diagnostics and logout continue to work. Account, TLS, trust, application
contents and configuration hashes were preserved across the accepted reboot.
Transaction guards and observer staging were removed; build artifacts and
private test receipts remain available.

## Ten-minute status and browser observation

On 2026-09-08, job `c0a0911c253b445993038ac1e769de7f` passed the complete
existing-account browser scenario with 61 sequential authenticated status
reads at ten-second intervals. The measured sampling duration was 631,371 ms,
maximum request latency 850 ms, and all status objects remained identical at
the exact sequence-five identity above. Desktop/mobile panels and Refresh
passed both before and after the observation. Video decode/reconnect and
logout revocation passed; logout returned 200. No control request or WebSocket
was opened. The private receipt is
`evidence/private/status-api-ten-minute-browser-reveal.json`.

The earlier job `80324399e7fc4c6c831ed564ec6f41b1` was not accepted overall:
its 61 status reads passed, but the subsequent settings interaction timed out.
Source inspection identified the desktop toolbar's five-second auto-hide.
The corrected test uses the ordinary top-center hover trigger before clicking
settings, without forced clicks or changes to stored menu preferences. The
successful repeat observed settings on-screen initially and off-screen after
the soak, then passed the full panel interaction after hover. This supports
auto-hide as the earlier test failure's cause; the old receipt did not capture
the exact timed-out substep. New receipts include bounded static substep names.
Host regressions cover reveal-before-click and failure when reveal does not
restore the toolbar. No application payload or device configuration changed.
The complete host `scripts/check.sh` gate passed in job
`28fcb7edf8904b759a6c2dbe696f4dc2` on 2026-09-08, including Rust checks/tests,
Python tests/type checks, offline systemd validation, browser-helper regressions,
and GUI build/lint/formatting. The existing large-chunk build warning remains;
it does not establish a runtime resource result.

These measurements do not establish CPU/RAM bounds, frame continuity throughout
the interval, other browser engines, or longer-duration resource stability.

Polling/video resource observation, deliberate status unavailability behavior,
additional browser engines, signed upload and activation UI, system recovery,
physical hardware gates and release acceptance remain outside this checkpoint.
