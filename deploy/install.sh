#!/usr/bin/env bash
# =============================================================================
# install.sh — One-command install for concord-bots
# =============================================================================
#
# Usage:
#   ./deploy/install.sh                    # clone and install
#   ./deploy/install.sh /path/to/bot.toml  # install with specific config
#
# This script:
#   1. Installs Rust (if needed)
#   2. Builds the bot in release mode
#   3. Copies the binary and service file
#   4. Sets up the systemd service
#   5. Starts the bot

set -euo pipefail

# Configuration
INSTALL_DIR="${INSTALL_DIR:-/opt/concord-bots}"
SERVICE_NAME="concord-bots"
BINARY_NAME="concord-bots"
USER_NAME="concord-bot"

echo "╔══════════════════════════════════════════╗"
echo "║   concord-bots installer                 ║"
echo "╚══════════════════════════════════════════╝"

# Check if running as root (needed for systemd install)
if [ "$EUID" -ne 0 ]; then
  echo "⚠️  Some steps require root (systemd install). Running with sudo..."
  exec sudo "$0" "$@"
fi

# Step 1: Install Rust if needed
if ! command -v cargo &> /dev/null; then
  echo "📦 Installing Rust..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  source "$HOME/.cargo/env"
  # Also source for the current script
  export PATH="$HOME/.cargo/bin:$PATH"
else
  echo "✅ Rust already installed: $(rustc --version)"
fi

# Step 2: Create user if needed
if ! id "$USER_NAME" &> /dev/null; then
  echo "👤 Creating user: $USER_NAME"
  useradd -r -s /bin/false -d "$INSTALL_DIR" "$USER_NAME"
fi

# Step 3: Create install directory
echo "📁 Creating install directory: $INSTALL_DIR"
mkdir -p "$INSTALL_DIR/config"

# Step 4: Copy source (if we're in the repo)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [ -f "$SCRIPT_DIR/Cargo.toml" ]; then
  echo "📦 Building from source in $SCRIPT_DIR..."
  cd "$SCRIPT_DIR"
  cargo build --release

  echo "📋 Installing binary to /usr/local/bin/"
  cp "target/release/$BINARY_NAME" "/usr/local/bin/$BINARY_NAME"
  chmod +x "/usr/local/bin/$BINARY_NAME"
else
  echo "❌ Could not find project source. Run from the repo root."
  exit 1
fi

# Step 5: Copy config
if [ -n "${1:-}" ] && [ -f "$1" ]; then
  echo "📋 Installing config from: $1"
  cp "$1" "$INSTALL_DIR/config/bot.toml"
elif [ -f "$SCRIPT_DIR/config/bot.toml" ]; then
  echo "📋 Installing default config"
  cp "$SCRIPT_DIR/config/bot.toml" "$INSTALL_DIR/config/bot.toml"
elif [ -f "$SCRIPT_DIR/config/bot.toml.example" ]; then
  echo "📋 Installing example config (edit before starting!)"
  cp "$SCRIPT_DIR/config/bot.toml.example" "$INSTALL_DIR/config/bot.toml"
fi

# Step 6: Set ownership
chown -R "$USER_NAME:$USER_NAME" "$INSTALL_DIR"

# Step 7: Install systemd service
echo "🔧 Installing systemd service..."
cat > "/etc/systemd/system/${SERVICE_NAME}.service" << EOF
[Unit]
Description=Concord Bot (Vector/Concord Protocol Bot)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/${BINARY_NAME}
WorkingDirectory=${INSTALL_DIR}
Environment=RUST_LOG=info
Environment=BOT_CONFIG=${INSTALL_DIR}/config/bot.toml
User=${USER_NAME}
Group=${USER_NAME}
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
EOF

# Step 8: Enable and start
echo "🚀 Enabling and starting service..."
systemctl daemon-reload
systemctl enable "$SERVICE_NAME"

echo ""
echo "╔══════════════════════════════════════════╗"
echo "║   ✅ Installation complete!              ║"
echo "╚══════════════════════════════════════════╝"
echo ""
echo "Next steps:"
echo "  1. Edit config: sudo nano $INSTALL_DIR/config/bot.toml"
echo "  2. Start bot:   sudo systemctl start $SERVICE_NAME"
echo "  3. Check logs:  journalctl -u $SERVICE_NAME -f"
echo ""
