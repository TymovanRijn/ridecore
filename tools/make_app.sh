#!/bin/zsh
# Builds RideCore.app (default: on the Desktop). Opening it brings the game
# up to date with the code in this folder (a quick no-op when nothing
# changed) and starts it. Usage: tools/make_app.sh [destination folder]
set -euo pipefail

ROOT=${0:A:h:h}
DEST=${1:-$HOME/Desktop}
APP="$DEST/RideCore.app"
TMP=$(mktemp -d)

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

# Icon: one drawing, every size macOS wants.
python3 -I "$ROOT/tools/make_icon.py" "$TMP/icon.png"
mkdir "$TMP/RideCore.iconset"
for size in 16 32 128 256 512; do
  sips -z $size $size "$TMP/icon.png" --out "$TMP/RideCore.iconset/icon_${size}x${size}.png" >/dev/null
  sips -z $((size * 2)) $((size * 2)) "$TMP/icon.png" --out "$TMP/RideCore.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$TMP/RideCore.iconset" -o "$APP/Contents/Resources/RideCore.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>RideCore</string>
  <key>CFBundleDisplayName</key><string>RideCore</string>
  <key>CFBundleIdentifier</key><string>nl.ridecore.race</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>CFBundleShortVersionString</key><string>0.1</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>RideCore</string>
  <key>CFBundleIconFile</key><string>RideCore</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSBluetoothAlwaysUsageDescription</key>
  <string>RideCore verbindt met je smarttrainer, hartslagmeter en Zwift Click via Bluetooth.</string>
</dict>
</plist>
PLIST

cat > "$APP/Contents/MacOS/RideCore" <<LAUNCH
#!/bin/zsh
# Brings RideCore up to date with its code, then starts it.
ROOT="$ROOT"
LOG="\$HOME/Library/Logs/RideCore.log"
export PATH="\$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:\$PATH"
cd "\$ROOT" || exit 1
BIN="\$ROOT/target/release/race"
if command -v cargo >/dev/null; then
  # Only rebuilding takes a while; tell the rider instead of looking stuck.
  if [[ ! -x "\$BIN" ]] || [[ -n "\$(find src assets Cargo.toml build.rs -newer "\$BIN" -print -quit 2>/dev/null)" ]]; then
    osascript -e 'display notification "Nieuwe versie wordt klaargezet, even geduld…" with title "RideCore"' >/dev/null 2>&1
  fi
  cargo build --release --bin race >"\$LOG" 2>&1 || \
    osascript -e 'display notification "Bijwerken mislukt, de vorige versie start." with title "RideCore"' >/dev/null 2>&1
fi
exec "\$BIN" >>"\$LOG" 2>&1
LAUNCH
chmod +x "$APP/Contents/MacOS/RideCore"
rm -rf "$TMP"
# Make Finder pick up the new icon.
touch "$APP"
echo "Gemaakt: $APP"
