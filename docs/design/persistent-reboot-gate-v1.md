# Persistent software reboot gate v1

This gate verifies one software reboot of the BliKVM H616 device. It never
invokes ATX Power/Reset or any equivalent power-control operation against the
attached target.

Before reboot, the host verifies the persistent Hardened service/listener,
account, certificate, and control-denial state. It then installs and syncs a
marker-bound systemd timer with a four-minute deadline. The timer starts again
after boot. If local Hardened HTTPS and its required services are unhealthy at
the deadline, the guard disables only Hardened Nginx and restores the retained
vendor `kvmd-nginx` web frontend. It does not delete the Hardened account,
package, config, or backend services.

The host requests `systemctl reboot`, requires both SSH and HTTPS to disappear,
then waits a bounded time for SSH recovery. Post-boot acceptance requires a
changed boot ID, byte-identical account state, active/enabled Hardened services,
disabled vendor web service, exact 80/443 scope, closed First Setup, TLS 1.2
and 1.3, active KVMD, and an absent control service/socket. Only after those
checks does the host remove the guard. An authenticated direct-LAN browser gate
remains a separate post-reboot check using the encrypted test credential.

If the host loses contact, the timer remains responsible for local health
evaluation and web fallback. Failure of the hardware to boot cannot be repaired
in software and remains outside this gate. No physical power cycle is allowed.
