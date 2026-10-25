#!/bin/sh
# On the board, in a lab session: modules built by the recipe load, work and
# unload.
set -eu
echo "taint before: $(cat /proc/sys/kernel/tainted)"
insmod ./ve_hello.ko
grep -c '^ve_hello ' /proc/modules
rmmod ve_hello
insmod ./ve_probe_test.ko
ls -l /dev/hardened-blikvm-ve-probe-test
python3 ./ve_probe_check.py > out/probe-test.json
cat out/probe-test.json
rmmod ve_probe_test
test ! -e /dev/hardened-blikvm-ve-probe-test
echo "taint after: $(cat /proc/sys/kernel/tainted)"
echo "module check: passed"
