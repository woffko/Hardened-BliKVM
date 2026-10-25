# Remote control from a phone (2026-10-24 to 2026-10-25)

After 0.1.0-beta.9 the operator, using the test board from an Android phone
(Chrome) over QUIC, reported that remote control turned off by itself while
the picture seemed to blink, and asked for a simpler way to take control
than the toolbar's button: the first tap or click on the screen, with the
confirmation. The fixes below changed, in turn, what the operator saw; each
step was installed on the test board at the operator's word and the next
report came from it. Times are UTC.

## What was found and done

| Item | What was wrong | What changed | Commit |
| --- | --- | --- | --- |
| RC1 | A QUIC session recorded a lease only from its own acquire. After a reconnect the page renewed and reported its lease over the new session; when the gateway closed the old one, the server released the lease the page was still using, and the next renew was refused (`LeaseConflict`). | A renew or report applied over a session makes that session the lease's, and takes it from the web session's other sessions. | `e2404c7` |
| RC2 | Any close of `/api/ws` (a mouse mode switch, a reset of the HID, an image mounted, a network drop) released the web session's lease, whatever transport held it. | The socket is closed at once; the lease waits 10 s for a socket of the same web session to open again and is released only if none does. | `3858f55`, `9ae743d` |
| RC3 | The gateway's journal said a session ended, not why. | It says the gateway's reason, the connection's (`timed out` after the idle timeout, `closed` when the page closed it) or the page's own end. | `50c0ac2`, update runtime dev43 `b996ffa` |
| RC4 | The page turned control off when its window lost focus and on any failed lease operation; control was taken with a toolbar button. | The toolbar button is gone: the first press on the screen (a key, a click, a tap) asks for the confirmation and reaches nothing; a press that only closes a menu asks nothing. Remote control is given back from the Mouse menu. A window without focus gives the lease back and keeps the confirmation; its next input takes the lease again without asking. | `f4a1c2c`, `ebb6227`, `e6a1a2c` |
| RC5 | After RC4 the phone's trackpad cursor drifted from the target's: a relative move that met a lost lease was dropped, and a lost relative move puts the two cursors apart for good. | No report is dropped silently: one that meets a lease that ended is sent again with a new lease after 0, 250, 500, 1000 and 2000 ms, or control turns off with "Remote control was lost: the board did not answer". A trackpad move while control is off moves neither cursor. | `75db3b4` |
| Layout | With the target agent not running, a layout kept on the board from two extended monitors ("part of a desktop") put the absolute mouse off the operator's duplicated monitors. The operator found it and chose "This screen is the whole desktop". | From the agent's monitors, one monitor or monitors duplicated is the whole desktop, whichever is the board's. The page keeps the agent's layout on the board when it differs from the kept one, so the mouse stays right once the agent is gone. While the agent's layout applies, the Mouse menu's Desktop entry says so and offers no layout of its own. | `c81918b` |
| Panel | The board's own display showed its status screen upside down. The configuration's MADCTL `0x70` comes from Waveshare's `LCD_1IN3_SetAttributes`, which the vendor's `kvmd-main` carries but never calls; the vendor initializes the panel through `spilcdInit` (MADCTL `0x00`) and turns its picture with `Paint_SetRotate(270)`, a quarter turn the other way. | Every frame is sent from its last pixel to its first, half a turn, without the address offset that a MADCTL mirroring the controller's 320-line side would need. The operator confirmed the panel the right way up. | `75519dc` |
| Ending report | The gateway sees only that a page closed its connection or went silent. | The first token request after a lost session carries how the page lost it (no pong, stream ended, read or write failed, transport closed, gateway closing), how long it was open, the time since the last pong, whether the page was hidden and what the browser said; the server writes it in its journal. | `d015ebb` |
| Blink | Each reconnect dimmed and blurred the screen until the new session's first key frame. | A picture shown before keeps its last frame for 1.5 s while the new session starts it again. | `0890612` |
| Path | The phone's page twice heard no pong for 6.2 and 7.0 s while it was shown, and the next connection opened within one or two seconds. | As RustAdmin's QUIC transport does for paths through a VPN, the gateway starts the path's MTU at 1200 and searches it up to 1360 instead of quinn's 1452, and the journal's line of a session's end says the path: round trip, MTU, congestion window, packets lost of sent, congestion events, black holes, MTU probes lost. | `f089796`, update runtime dev44 |

RustAdmin's other QUIC choices were not taken: its loss-based congestion
control (BBR stays: on a path losing 3 % of packets, Cubic drew 5.9 frames a
second against BBR's 30, `docs/design/quic-transport-v1.md`), its video
protocol, and its client's 30 s of patience without data (the next
connection opens at once while the stalled one is still stuck).

## What the board showed

The board's cables were ruled out for the breaks: the Ethernet link did not
drop since boot (no error, no carrier loss), the encoder ran steadily around
every break, and the kernel logged no USB reset. All the breaks were the
phone's: its page closed the connection (`closed`), or it went silent for
the 30 s idle timeout (`timed out`, its screen off). With the ending report
installed, the phone's page said "its pings went unanswered" twice (05:12
and 05:18 UTC, after 208 and 32 s); with dev44, once "its connection
closed ... WebTransportError: Connection lost." 220 ms after the last pong
(05:47 UTC), when the phone could not reach the board again for 39 s,
on a path of 6 ms round trip, MTU 1360, 0.7 % of packets lost and no black
hole. The cause of the stalls without a pong is not proven.

The diagnostic lines are written at a session's end and at the next token
request, about 250 bytes each (17 in the day of use on 2026-10-25). The
board's journal lives in memory: Armbian's nightly job takes journald off
`/var/log/journal`, and `/var/log` itself is a zram disk; nothing of it
reaches the card but a daily copy of `/var/log` of under 100 KB. The lines
do not outlive a reboot.

## Checks

- Unit tests of each change: `a_lease_used_over_a_later_session_stays_when_the_earlier_one_closes`,
  `a_socket_that_closes_gives_the_lease_back_only_when_none_opens_again`,
  `the_journal_says_why_a_session_ended`,
  `the_journal_says_what_the_path_was_at_the_end`,
  `the_journal_says_how_a_page_lost_its_last_session`,
  `a_frame_is_sent_half_a_turn_round`, `tools/test_agent_layout.mjs`,
  `test_dev43_carries_the_files_of_dev42`, `test_dev44_carries_the_files_of_dev43`.
- Page contracts: `restricted_control_lifecycle` (rewritten: a press asks,
  Cancel reaches nothing, a click closing a menu asks nothing, blur keeps the
  confirmation, a lost lease taken again with the report sent again, a lease
  that cannot be had back turns control off with the notice),
  `quic_transport` (the ending said once, with the first request after the
  loss), `target_agent_reads` (each layout kept on the board, the Desktop
  entry the agent's), `screen_area`, `agent_typing`, `quic_input`,
  `quic_input_stack`, `quic_video`, `quic_share`, and the input contracts
  that take control by the first press.
- The test board took slots 102 to 105 and update runtimes dev43 and dev44
  at the operator's word, each with the board's state read before and after
  and differing only in the slot; no unit failed.

## Release

Recorded below when 0.1.0-beta.10 is published.
