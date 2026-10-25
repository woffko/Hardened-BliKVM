Hardened BliKVM agent
=====================

The agent tells the Hardened BliKVM board what it cannot see on this
computer (Windows version, monitors, keyboard layouts) and does what the
operator asks from the board's page: switch the keyboard layout, change
the monitors' mode (undone after 15 s unless kept), carry clipboard text,
lock the session. It runs with your rights, needs no network, and shows
itself in the notification area, whose menu stops it or ends it.

Start hardened-blikvm-agent.exe from this drive. The drive is read-only;
the board presents it while its "Target agent" switch is on.

SHA256SUMS.TXT gives the program's SHA-256, the same as in the release.
The program is built reproducibly from the project's source.
