# Code audit of 2026-11-06, checked against the code

An external code audit of 2026-11-06 reviewed the branch at `e2e1621`
(0.1.0-beta.10 and the page's link). It came as one report in two copies in
the project's folder, `code-audit-2026-11-06.md` and, over the earlier
report of that name, `code-audit-2026-10-20.md` (the same text with a few
names miscased); neither is in Git. This note checks each of its seventeen
findings against the code at `e2e1621` and the device's model. The plan for
the findings that hold is `work-plan-20261106-code-fixes.md`.

## The model

As in `audit-20261018-security-review.md`: the authenticated, unlocked web
operator has full rights over the device (root terminal, SSH, scripts,
updates), so a finding that needs that operator and gives nothing beyond it
is out of the model. In the model are unauthenticated network clients,
cross-origin pages, a locked session, the target as USB host, the servers
the board downloads from, the boundaries between the board's services
(separate Unix users), and plain functional faults: wrong behaviour, races,
lost input, hardware safety, and tools that can spoil the operator's data.

## How it was checked

Each finding was traced in the code it names, whole functions and units
read, the design notes compared, and a test named that fails on today's
code where the finding holds. The checks only read: nothing was changed,
built for the board or sent to it. The one board fact used is the
published beta.10 image's staging tree, whose `/var/lib/hardened-blikvm`
is empty (for AUD-15).

## The report's own accuracy

The report's mechanisms are often right, but many of its names, lines and
consequences are not. Its table of earlier fixes "verified intact" cites
things the code does not have, for instance an `inited` field in the
board's display driver, "MPP FFI" in the encoder (it uses its own VE client,
libvpx, x264 and libjpeg-turbo), `isFocusRequiredError`, `releaseHidSession`
and `currentLeaseId` in the page, a `menu/clipboard` page, `wait_for_topology`
in the agent, a `hid_focus` lock in the board owner, a
`picoclaw-binary.json` record, the functions `prepare_binary`,
`verify_binary_record`, `terminal_tickets.revoke_for_session`,
`picoclaw::on_logout` and `quic::end_session`, and the commit `c120d17`. It
calls the board's ST7789 SPI panel an I2C OLED. Its line numbers are mostly
approximate. No conclusion of this note rests on the report's text alone.

## Results

| Finding | Holds | Model | What it means on a board | Where the fix goes |
| --- | --- | --- | --- | --- |
| AUD-01 | partly | in | On PicoClaw's first setup and after an uninstall, the configuration helper runs the installed binary unchecked, as the runtime's user: whoever can write the install directory (the `pico-install` account, a boundary Y4 relies on) chooses what runs with the assistant's keys and tokens. The claim that the unit's sandbox breaks `onboard` is refuted (it worked on the board, slots 31 to 33). | system (admin helper) |
| AUD-02 | partly | in, low reach | boardd's manual fan mode starts at 0 % and leaves the supervisor's record stale, so back in Auto the fan can stay at the manual duty while the status says otherwise; in Manual no thermal failsafe acts. Neither the page nor the API reaches the fan: only root on the board (the bring-up gate tool). | slot (boardd) |
| AUD-03 | partly | in, low | Focus bindings swept on expiry cannot be extended again (`extend_bindings` finds nothing), and a renew does not extend them. The page re-confirms focus with every renew, so the claimed "ATX refused after 30 s of watching" does not happen with this page. | slot (server) |
| AUD-04 | partly | in, low | PicoClaw's lock and relay are keyed by session id alone: an old relay's teardown can release the lock a reconnect of the same session just took, which then closes with 4004. The report's case B cannot happen; case A is a passing 409. | slot (server) |
| AUD-05 | yes | in | A renew that fails ends the lease inside `HidLeaseSession` without telling `useHidLease`: control still shows as on, "Keep these monitors" and the Monitors menu get 409 and the agent's monitors and layouts stop updating without a notice. Input recovers through RC5. | slot (page) |
| AUD-06 | partly | in | The LED and the display button are ticked only with the 1 s thermal sample: the LED's 0.1 s blink every 2 s becomes irregular 1 s flashes with long gaps, and the button must be held 2 to 3 s instead of 1. The active-low inversion is real in code but this board's button is active-high. | slot (boardd) |
| AUD-07 | yes | in | The update status worker exits 96 on any I/O error, a client that left included (an aborted page request), which is the failed `hardened-blikvm-update-status@` instances seen on the board since beta.9. | system (update supervisor) |
| AUD-08 | partly | in | Switching mDNS restarts `systemd-resolved` under the board owner's one lock, freezing keyboard and mouse and the HID expiry for the restart (usually under a second, up to 60 s). Reading the TLS state runs `openssl` there without a timeout. The Tailscale part is refuted (the server uses the admin helper); board status is off on the board and needs root. | slot (board owner) |
| AUD-09 | partly | in, low | A QUIC session that ends releases the lease at once, and an `/api/ws` gap of over 10 s releases any lease: both are the documented design. What remains is the page not learning of it (AUD-05) and a stale focus cache (AUD-10). "WsClient gives up after 5 tries" is refuted (it retries for ever). | slot (page) |
| AUD-10 | partly | in | Power and Reset keep a 15 s client cache of the focus confirmation; when the server cleared focus (a QUIC reconnect, the `/api/ws` grace, a lock) the click is refused with 400 and nothing is said: the menu items drop the rejection. The report's trigger (a release route) does not exist. | slot (page) |
| AUD-11.1 | yes | in, narrow | An IPv4 change back to the original addressing inside the confirmation window starts a new 120 s window instead of ending it (Wi-Fi ends it): DNS set with that change is rolled back unless kept. | slot (board owner) |
| AUD-11.2 | partly | in, narrow | After a restart with a revert pending, the owner retries the revert for 55 s holding only its runner lock: an IPv4 or Wi-Fi edit in that time can be undone or lose its revert record. | slot (board owner) |
| AUD-11.3 | no | - | A rejected firewall confirmation rewrites the previous mode (`forget`). Left: a power loss between writing the mode and confirming keeps the unconfirmed mode for the next boot. | optional |
| AUD-11.4 | partly | in | After a RAW update, a switch whose restore failed once (SSH, QUIC, mDNS) is recorded as the new image's and never retried. The Tailscale part is refuted (nothing calls it). | slot (board owner) |
| AUD-12 | partly | in, low | Encoder: a forced key frame the VE fails to make is retried only with the next 2 s refresh on a still screen (a new viewer waits about 2 s more). Gateway: a group given up keeps its unreceived records until the page's next receipt, inflating one lag sample (negligible). The `asked` and `skips_key` parts are refuted (design). | system (encoder, gateway) |
| AUD-13 | no | - | Locking revokes the other sessions, and their QUIC sessions, terminal and PicoClaw relay end within 1 s through the shared revoked flag; the functions the report compares with do not exist. | - |
| AUD-14 | partly | in, low | The restore tool opens the root partition without an exclusive claim: if the host's auto-mounter mounted it, the host can write over the restored root (the card still does not boot; running the tool again repairs it). The unquoted `debugfs` operand is no data risk. | host tool |
| AUD-15 | partly | in | The card image's cleanup globs skip dotfiles, and `/var/lib/hardened-blikvm` has no emptiness check: a server temporary such as `.account-*` (the password hash) left by a crash would pass into a published image. beta.10's image has none. | host tool (image) |
| AUD-16 | partly | in, low | The page keeps the agent's layout key after the agent leaves, so the same layout is not written to the board when the agent returns; after it leaves again the page maps with the stale kept area until reloaded. | slot (page) |
| AUD-17 | mostly no | out | resolv.conf truncation needs network writes switched on (not on the board) and an I/O error as root; the mass-storage size check is refuted (no such limit, the kernel refuses short images); the redirect marker point concerns a TLS switch that is off. | optional |

Refuted or out of the model, and not planned: AUD-13; AUD-01's sandbox
claim; AUD-03's ATX consequence; AUD-04's case B; AUD-05's point 6 (`hold`
already resets focus and turns control off with a notice); AUD-08's
Tailscale and board status parts; AUD-09's "WsClient gives up"; AUD-11.3 as
written and AUD-11.4's Tailscale part; AUD-12's `asked` and `skips_key`
parts; AUD-14's quoting; AUD-17.2 and AUD-17.3 as reachable.
