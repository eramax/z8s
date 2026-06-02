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




# 1. Kill z8s
sudo pkill -9 -f z8s

# 2. Iterate and unmount all stuck rootfs locations
for mount in $(mount | grep /var/lib/z8s | awk '{print $3}'); do sudo umount -f $mount; done
for mount in $(mount | grep /tmp/z8s | awk '{print $3}'); do sudo umount -f $mount; done

# 3. Nuke the databases and data folders now that mounts are clear
sudo rm -rf /var/lib/z8s/z8s.redb /tmp/z8s-node-7443.redb
sudo rm -rf /var/lib/z8s/rootfs /tmp/z8s-node-*/rootfs

# 4. Start fresh and run tests!
sudo target/debug/z8s node start
sudo target/debug/z8s node start --port 7443
sleep 3
./tests/run-tests.sh



# Generate cert (one-time)
openssl req -x509 -newkey rsa:2048 -keyout /tmp/z8s-key.pem -out /tmp/z8s-cert.pem \
  -days 365 -nodes -subj "/CN=localhost" \
  -addext "subjectAltName=IP:127.0.0.1,DNS:localhost"

# Start with TLS
z8s node start --tls-cert /tmp/z8s-cert.pem --tls-key /tmp/z8s-key.pem

# kubectl kubeconfig (already configured)
kubectl get pods

