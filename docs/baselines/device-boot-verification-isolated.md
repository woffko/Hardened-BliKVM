# Native mandatory startup-verification gate

Job `d75834dc058f4edbadd891fab39a6167` passed on H616 on 2026-09-05.
The receipt is `evidence/private/device-boot-verification-sectionfix.json`.
The real recovery template is a required ordered dependency of both isolated
server services. With the fixture timer stopped, missing trust configuration
blocked serving; a modified signed web byte also blocked serving while the
unchanged executable remained runnable. Restoring exact inputs allowed normal
startup, candidate confirmation and timeout rollback. Clean ticks preserved
the confirmed server PID. The actual production runtime/state remained
unchanged and all fixture/controller staging was removed.

The first attempt proved missing-trust rejection but stopped when resetting an
inactive dependency-failed service. The runner now resets only the failed
recovery service. Its extra server write-path/runtime-limit directives also
now explicitly reopen `[Service]`, avoiding their accidental placement after
the current template's `[Install]` section. These were fixture corrections,
not a weakening of the recovery dependency or package sandbox.

This proves native dependency behavior with controlled isolated inputs; it
does not prove an actual reboot. The device still runs updater dev4, confirmed
production application sequence two. Next package/deploy dev5 preserving that
operational state, then perform confirmed and interrupted-update reboot gates.
