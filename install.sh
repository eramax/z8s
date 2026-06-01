#!/usr/bin/env bash
set -e

Z8S_HOME="$(cd "$(dirname "$0")" && pwd)"

echo "Building z8s release binary..."
(cd "$Z8S_HOME" && cargo build --release)

echo "Installing z8s binary to /usr/local/bin..."
sudo cp "$Z8S_HOME/target/release/z8s" /usr/local/bin/z8s
sudo chmod +x /usr/local/bin/z8s

echo "Creating log directory..."
sudo mkdir -p /var/log/z8s

# Install AppArmor profile if apparmor_parser is available (Ubuntu 23.10+)
if command -v apparmor_parser &>/dev/null; then
    echo "Installing AppArmor profile..."
    sudo install -m 644 "$Z8S_HOME/etc/apparmor.d/z8s" /etc/apparmor.d/z8s
    sudo apparmor_parser -r /etc/apparmor.d/z8s && \
        echo "AppArmor profile loaded." || \
        echo "Warning: AppArmor profile load failed (check 'sudo aa-status')."
else
    echo "apparmor_parser not found — skipping AppArmor profile installation."
fi

echo ""
echo "z8s installed. Use 'sudo z8s' to start, 'sudo z8s stop' to stop."
