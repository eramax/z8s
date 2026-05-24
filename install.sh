#!/usr/bin/env bash
set -e

Z8S_HOME="$(cd "$(dirname "$0")" && pwd)"

echo "Building z8s release binary..."
(cd "$Z8S_HOME" && cargo build --release)

echo "Installing z8s binary to /usr/local/bin..."
sudo cp "$Z8S_HOME/target/release/z8s" /usr/local/bin/z8s

echo "Installing daemon script..."
sudo cp "$Z8S_HOME/scripts/z8s-daemon.sh" /usr/local/bin/z8s-daemon
sudo chmod +x /usr/local/bin/z8s-daemon

echo "Creating log directory..."
sudo mkdir -p /var/log/z8s

echo ""
echo "z8s installed. Use 'sudo z8s-daemon {start|stop|restart|status}' to manage."
