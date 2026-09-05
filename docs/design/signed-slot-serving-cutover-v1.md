# Serving the initial signed application slot

Prerequisite: accepted persistent bootstrap with generation/floor one, the
known signed slot, no pending activation, updater dev2 installed, and healthy
package-backed GUI with preserved account and TLS.

Nginx currently requires `hardened-blikvm-server.service`. Starting the
conflicting current-slot server could therefore stop Nginx. The cutover must
first replace only that lifecycle dependency in the root-owned local Nginx
unit. Nginx will be ordered after both current and candidate service names but
will not require either one. Enablement of the confirmed service supplies
boot startup; candidate transitions may then stop either backend without
tearing down the HTTPS listener. Proxy configuration and TLS remain identical.

The gate must bind the exact original and resulting unit bytes and service
enablement states, arm early-boot and deadline recovery, then reload systemd,
disable the package-backed server and enable/start the signed current server.
It must verify the actual process executable/hash, account, signed-slot proof,
initial journal, unchanged broker/video services, HTTPS health and listeners,
and inactive candidate/updater infrastructure. Browser login, Diagnostics,
decoded video, reconnect and logout checks use the existing account harness.

Rehearsal restores the original local Nginx unit and package-backed server
enablement while disabling the current-slot server, then independently checks
the prior runtime. The retaining run keeps the new dependency arrangement and
current-server enablement only after the same checks. Unknown unit content or
state changes must stop recovery rather than overwrite unrelated changes.

This operation restarts the serving backend but does not activate a new bundle,
advance the anti-rollback floor, enable candidate confirmation/watchdog timers,
change broker binaries, reboot, or invoke attached-target Power/Reset. Those
are later acceptance boundaries.
