#!/bin/sh
# On the board: who is connected, one line each: the HTTPS connections' peers and the open QUIC
# sessions' peers (V7.1, the end-to-end windows). Reads only.
ss -Htn state established '( sport = :443 )' | awk '{ print "tcp " $4 }' | sort
journalctl -b -o cat --no-pager -u hardened-blikvm-quic.service 2>/dev/null |
  awk '/session from .* opened/ { for (i = 1; i <= NF; i++) if ($i == "from") open[$(i + 1)]++ }
       /session from .* ended/ { for (i = 1; i <= NF; i++) if ($i == "from") open[$(i + 1)]-- }
       END { for (peer in open) if (open[peer] > 0) print "quic " peer }' | sort
