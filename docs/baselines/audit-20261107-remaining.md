# What the code audit of 2026-11-06 left, carried out

The record of `work-plan-20261107-remaining.md`: the optional items of the
audit's plan, RUSTSEC-2026-0385, and the page contracts and the server test
that failed now and then while 0.1.0-beta.11 was checked
(`audit-20261106-code-review.md`). Each commit names the test that failed on
the code before it, or the stated reproduction that did; where a decision or
a write was first moved behind a seam, the test failed on it as it was.

## Fixes

| Item | What | Commit | Test or reproduction that failed before | State |
| --- | --- | --- | --- | --- |
| R1 | AUD-11.3: the firewall mode renamed into place once its change took effect | `a615704` | the persisted mode changes only once the change took effect (a leftover `.new` was kept; during the batch the file already named the new mode) | fixed |
| R2 | AUD-17.1: resolv.conf put back when its write fails | `2067757` | a resolver file whose write fails keeps its previous contents (it was left as `nameserv`) | fixed |
| R3 | AUD-17.3: no TLS switch-off that leaves port 80 redirecting | `e9c797b` | switching off a redirect it did not write is refused (it answered Applied, wrote and reloaded) | fixed |
| R4 | AUD-14: the restore tool's `debugfs` operands quoted | `b573462` | a work directory with a space still restores (exit 1, "the data partition holds no apply.json") | fixed |
| R5 | RUSTSEC-2026-0385: `ctutils` 0.4.8 | `e95409e` | `cargo audit --no-fetch --deny warnings` (one vulnerability) | fixed |
| R6 | the contracts' waits on a steady clock | `32538ee` | `tools/test_eventually.mjs` (the module was missing; with it alone, fourteen files still timed waits by `Date.now()`; a wall-clock wait gives up under a stubbed 3.6 s step) | fixed |
| R7 | the screen area contract's cleared layout | `152de05` | with the board's answer 600 ms late, 3 of 3 runs failed "the whole desktop again: 16345" | fixed in the contract (the page maps with the layout the board answers with, as designed) |
| R8 | the screen enlargement contract's keys | `3610376` | a key pressed in a field the moment it takes the focus (3 of 3 runs sent it on) | fixed in the page: the cause, found in 40 runs (one failure, the Delete of a field being cleared), was the keyboard's listener seeing the field's "keyboard off" a render late |
| R9 | the QUIC input contract's ended session | `2ad5633` | with the login's answer 1.5 s late, 3 of 3 runs sent two or three acquires | fixed in the page: a 401 is final and no lease is asked for until the page loads; the 15 s page load of 2026-11-07 did not come back in 40 runs (not confirmed) |
| R10 | the server's logout test under load | `8808c2f` | with the setup slowed by a stated 4.5 s, the test timed out | fixed in the test: the bound covers the exchange (5 s of it the server's own discard), not the setup |

Found on the way: `no_change_is_taken_while_the_start_still_reverts` (N6's
test) gave the revert's failure up before it asked for the refused change
and waited 8 s at most; it failed once in a loaded broker run and now asks
first and waits for the outcome (`9d0bffc`). The broker's
`restricted_profiles_attach_media_alongside_hid_and_atx` timed out once in
the same run ("mock KVMD did not finish"); it predates this work and passed
in the runs after.

Decisions 1 to 3 were taken as written. R8 and R9 changed the page, so they
go into slot 108 with R1 to R3.

## Gate B

On the test board, at the operator's word, read-only probes only and the
board's settings left as found.

- Slot 108 (0.1.0-beta.12) was built from a clean tree at `8808c2f` after a
  green `scripts/check.sh` (the slot's tar `6a60a246…e587`, its manifest
  `0d045bb3…a422`). The update runtime that build makes is the staged
  dev46's but for the package manifest: no system part changed.
- The first install was refused by the health check, and the board went
  back to slot 107 by itself: the new owner's readiness was rejected
  because the owner's ledger held key releases it could not write, the
  target's USB not being configured (the controller in state `default`,
  every write refused). The board had restarted about ten hours before,
  near this host's own restart, and a cable had come loose. Once the
  operator had seated it, the controller was configured, the releases went
  through, and the second install was confirmed. The failed activation
  request of the first attempt was reset.
- The state as found but for the slot (107 to 108). The watching probes
  passed: the GUI walks on a desktop and a phone, WebRTC with H.264 and with
  VP8, QUIC video, touch reach. R1 to R3 were not tried on the board: its
  firewall, network and TLS settings are not changed there.
