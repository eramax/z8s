ls /sys/class/net/ | grep veth
sudo cat /proc/net/fib_trie 2>/dev/null | grep "LOCAL\|10\.42" | head -20