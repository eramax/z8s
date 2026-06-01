ls /sys/class/net/ | grep veth
sudo cat /proc/net/fib_trie 2>/dev/null | grep "LOCAL\|10\.42" | head -20

sudo apt-get install -y -qq libclang-dev 


# 1. Kill any stray z8s processes
sudo pkill -9 -f z8s

# 2. Forcefully unmount any leftover container rootfs mounts
mount | grep z8s | awk '{print $3}' | sudo xargs -r umount -f

# 3. Clean the state and databases
sudo rm -rf /var/lib/z8s /tmp/z8s* /etc/z8s

# 4. Start the hub node
sudo target/debug/z8s node start

# 5. Start the spoke node
sudo target/debug/z8s node start --port 7443

# 6. Wait a few seconds for gossip to establish, then run the test
sleep 3
tests/netmux/test_hub_spoke_setup.sh


