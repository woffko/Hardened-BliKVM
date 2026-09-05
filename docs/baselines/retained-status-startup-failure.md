# Retained status: startup failure remains a reboot acceptance blocker

Persistent wiring was committed by `ace7bb36f5f84a12be5a8cc196320234`, cleaned
by `c8b8ec64c42e47778777a28bee2d4992`, and independently observed by
`9ecb03e7430648f5868ea6c01c48f87b`. A later single-reboot attempt
`482e6ca2e7374fdeb08b52d48824d817` was accepted by systemd but did not pass its
bounded post-boot comparison. It must not be reported as a successful reboot
acceptance merely because subsequent manual recovery worked.

Read-only diagnosis proved the persistent drop-ins remained, the status socket
was listening, but the initial application recovery service failed with exit
20. The confirmed server stayed inactive due to its failed dependency. Nginx
and SSH remained available. Exit 20 only identifies a manager-open failure in
the installed supervisor and does not prove a particular underlying variant.

Source inspection found no ordering between that initial recovery and the
two-second periodic watchdog; both open the same locked manager. Contention is
a plausible cause, not proven retrospectively by the coarse exit code. The
source correction makes the periodic watchdog require and follow successful
initial recovery. Direct manager failures now use the existing specific
60–89 exit-code domain instead of the generic 20, preserving coarse public
messages without printing state/configuration contents.

One controlled recovery attempt checked valid confirmed sequence five and
generation twenty-two before starting the installed recovery service and then
the confirmed server. Its immediate observation raced process startup and
failed the environment check. Separate observation
`f0a5e3a2a97d4edd9491b14f5978419e` then verified the healthy retained state;
browser job `6f67692620ab4c249c85c752e977aa10` passed status desktop/mobile,
video and logout. No further reboot was sent during diagnosis or recovery.

The diagnostic driver also needed to handle intentionally absent SSH stderr;
the observer now returns only bounded failure classes and source coordinates.
The source fixes and dev7 package remain subject to full host, reproducible
build, guarded upgrade and fresh reboot/browser acceptance. The working device
is still on dev6 and confirmed application sequence five at this checkpoint.
