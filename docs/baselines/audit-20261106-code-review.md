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

## Fixes

The plan's items as they were carried out on `wip/corrective-audit-checkpoint`
(each commit names the test that failed on the code before it and passes
after; where a decision was moved into a function first, the test failed on
the decision as it was). Slot 107 (`db50e2c`) carries phase A; update
runtime `0.1.0~dev46` (`5f6e516`) and the encoder carry phase Y; phase T is
host tools. What the board showed is in "Gates" below.

| Item | Finding | Commit | Test that failed before | State |
| --- | --- | --- | --- | --- |
| N1 | AUD-05, AUD-09 | `b8f7cdb`, `fcaf333` | a refused renewal is forgotten: the Monitors menu takes a new lease before it asks the agent; a QUIC session lost and back takes a new lease before the next key | fixed |
| N2 | AUD-10 | `82cf738` | a press the board refused for want of a confirmation is confirmed again and sent once more; a refusal that stays is said | fixed |
| N3 | AUD-08 | `ec21aca` | an mDNS switch leaves HID leases answered (the lease waited 1.9 s); TLS and the board's status run on the slow settings' worker, `openssl` with a timeout | fixed |
| N4 | AUD-11.4 | `d065e92` | a unit that cannot be set does not stop the others, and a failed restore is tried again | fixed |
| N5 | AUD-11.1 | `7f621f6` | a change back to the first addressing ends the window | fixed |
| N6 | AUD-11.2 | `f3bf3a4` | no IPv4 change, and no Wi-Fi preference, is taken while the start still reverts | fixed |
| N7 | AUD-06 | `ed63a97` | the activity LED lights its pulse once a period; a press held for its hold wakes the display once; an active-low button wakes once on a press | fixed |
| N8 | AUD-02 | `7f531a3` | Manual straight after the start keeps the fan running; Auto after Manual writes the policy's command | fixed; not tried on the board (fan settings are not changed there) |
| N9 | AUD-03 | `2ece6c4` | swept bindings are kept again for a live lease | fixed |
| N10 | AUD-04 | `c2fe076` | an old connection does not release a new one of its session | fixed |
| N11 | AUD-16 | `db50e2c` | the agent's layout is written again when the agent comes back, and after a failed write | fixed |
| N12 | AUD-01 | `73d9e8e` | a runtime that is not the pinned release is not onboarded; onboarding runs the checked copy and removes it | fixed; not run on the board (PicoClaw is not installed there) |
| N13 | AUD-07 | `925818e` | a client that leaves is no failure | fixed |
| N14 | AUD-12 (encoder) | `996d590` | a forced key frame that was missed lets the next capture through | fixed |
| N15 | AUD-12 (gateway) | `84bcf0c` | groups given up forget their unreceived frames | fixed |
| N16 | AUD-15 | `7071b60` | the cleanup removes dotfiles and checks the account's directory | fixed |
| N17 | AUD-14 | `fb3189e` | a root partition in use is refused and left unchanged | fixed |

`5f6e516` names the update runtime dev46 (test
`test_dev46_carries_the_files_of_dev45`). The optional items were left out
(decision 3).

## Gates

On the test board, at the operator's word for each install; read-only
probes only, the board's settings left as found.

- **Gate A (slot 107).** Built from a clean tree at `db50e2c` after a green
  `scripts/check.sh` (the slot's tar `3cb2d2f7…4271`, its manifest
  `8b06be6e…5bc8`). The board's update runtime dev45 had the files the tree
  builds, but for the package manifest, and the encoder its hash. Installed
  through the API and confirmed; the state as found but for the slot
  (106 to 107). boardd ran with the fan in Auto, active (30 % at 52 °C), the
  LED and the button present. The watching probes passed: the GUI walks on a
  desktop and a phone, WebRTC with H.264 and with VP8, QUIC video, touch
  reach. The display button (N7): a press of about a second did not wake the
  panel (the hold is 1000 ms after a 50 ms debounce), a press of 1.5 to 2 s
  woke it once; before N7 it took 2 to 3 s. The walks left one failed
  `hardened-blikvm-update-status@` instance on dev45, the defect N13 fixes.
- **Gate Y (update runtime dev46, encoder).** Built from a clean tree at
  `5f6e516` after a green `scripts/check.sh` (three runs before it stopped
  on load flakes under another session's load: the server's
  `logout_upload_returns_unauthorized_over_real_http1` twice, the phone keys
  contract once; each passed alone). Against dev45, the admin helper (N12),
  the QUIC gateway (N15), the update supervisor (N13) and the manifest
  differ; `system-update` does not, so the board's initramfs still reads the
  RAW stage. Staged by hand: the package over dev45, the encoder
  (`1e579d33…`, N14) through `deploy_vp8.sh` once the board's video units and
  Janus configuration were found to be packaging's, the QUIC gateway
  restarted, dev45's failed status instance reset; no reboot. The state as
  found; the probes passed again, and after the GUI walks no status instance
  had failed (N13). N8 and N12 were not tried on the board: fan settings are
  not changed there, and PicoClaw is not installed.
- **Gate T (host tools).** The tests of N16 and N17 pass. The card image of
  0.1.0-beta.11, taken from the board after gate Y with KVM pages closed
  and built at `82b9d64` with N16's cleanup, has an empty
  `/var/lib/hardened-blikvm`; it carries dev46, the N14 encoder, slot 107 as
  current, the system sequence 11, packaging's `nginx.conf`, no host keys
  and an empty home for root (img `13d43769…6f06`, root `679aec46…eb49`; the
  compressed image unpacks to the image, whose first partition is the root).
- **The phone keys contract.** `tools/playwright/mobile_keys_contract.mjs`
  failed in about half its runs whatever the load, on this branch and at
  `7c0dff1` before any fix: its Mac section counted the release of every key
  that taking control and opening the keys send as `Cmd+C`'s own report.
  `82b9d64` counts an action's reports from a quiet point; 10 of 10 runs
  passed.
- **The RAW system update of sequence 11** (0.1.0-beta.11, at the operator's
  word), built from that image's root, was installed from beta.10's system
  (sequence 10, with dev46 staged) by beta.10's boot step: back after
  1990 s, it kept the certificate, the host keys, the account, slot 107, the
  host name, the session lock and SSH, and the state differed only in the
  temporary IPv6 addresses, the system's version and sequence and an encoder
  not started yet; confirmed (the floor is now 12). Root's home was saved
  before and put back after (the update carries only its SSH keys). The
  RAW switch was found on and left on. The watching probes passed on the new
  system.
