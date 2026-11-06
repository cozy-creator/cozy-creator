#!/usr/bin/env bash
# net.sh <action> <ip> <port> [rate]: this runner's own path to one machine's WebRTC port, and nothing else's. Every action names that port.
#   reset      kills the established TCP connection, as a dropped network does; says how
#   block      silently drops the port's packets both ways; unblock lifts it
#   shape      holds what the port sends here to <rate>, with 40 ms more RTT and a deep queue; unshape lifts it
set -uo pipefail
action=$1 ip=$2 port=$3 rate=${4:-}
live() { ss -tnH state established dst "$ip" dport = ":$port" | wc -l; }
rules() { sudo iptables "$1" OUTPUT -d "$ip" -p tcp --dport "$port" -j DROP && sudo iptables "$1" INPUT -s "$ip" -p tcp --sport "$port" -j DROP; }
dev=$(ip route get "$ip" | sed -n 's/.* dev \([^ ]*\).*/\1/p' | head -1)
case $action in
  reset)
    before=$(live)
    sudo ss -K -t dst "$ip" dport = ":$port" > /dev/null 2>&1
    if [ "$(live)" -lt "$before" ]; then echo "ss -K"; exit 0; fi
    # A kernel without socket destroy: a reset from the firewall, held only until the browser's next packet met it.
    sudo iptables -I OUTPUT -d "$ip" -p tcp --dport "$port" -j REJECT --reject-with tcp-reset || exit 1
    for _ in 1 2 3 4 5 6 7 8 9 10; do [ "$(live)" -lt "$before" ] && break; sleep 0.2; done
    sudo iptables -D OUTPUT -d "$ip" -p tcp --dport "$port" -j REJECT --reject-with tcp-reset
    echo "iptables tcp-reset" ;;
  block) rules -I ;;
  unblock) rules -D ;;
  shape)
    sudo modprobe ifb numifbs=1 && sudo ip link set ifb0 up &&
      sudo tc qdisc add dev "$dev" handle ffff: ingress &&
      sudo tc filter add dev "$dev" parent ffff: protocol ip u32 match ip src "$ip/32" match ip sport "$port" 0xffff action mirred egress redirect dev ifb0 &&
      sudo tc qdisc add dev ifb0 root handle 1: htb default 10 &&
      sudo tc class add dev ifb0 parent 1: classid 1:10 htb rate "$rate" &&
      sudo tc qdisc add dev ifb0 parent 1:10 handle 10: netem delay 40ms limit 1000 ;;
  unshape)
    sudo tc qdisc del dev "$dev" ingress; sudo tc qdisc del dev ifb0 root ;;
  *) exit 2 ;;
esac
