#!/usr/bin/env bash
# Walks the phone app's controls in the emulator once a session is streaming, and takes a
# screenshot of each state for the e2e artifact.
set -euo pipefail

# Prints "x y" for the centre of the first view whose attribute has the value.
center() {
  adb shell uiautomator dump /sdcard/ui.xml > /dev/null
  adb shell cat /sdcard/ui.xml > ui.xml
  python3 - "$1" "$2" <<'EOF'
import re
import sys
import xml.etree.ElementTree as ET

attr, value = sys.argv[1:]
for node in ET.parse("ui.xml").iter("node"):
    if node.get(attr) == value:
        x1, y1, x2, y2 = map(int, re.findall(r"\d+", node.get("bounds")))
        print((x1 + x2) // 2, (y1 + y2) // 2)
        sys.exit(0)
sys.exit(f"no view with {attr}={value!r} on screen")
EOF
}

tap() {
  local xy
  xy=$(center "$1" "$2")
  # shellcheck disable=SC2086
  adb shell input tap $xy
  sleep 1.5
}

shot() {
  adb exec-out screencap -p > "$1.png"
}

back() {
  adb shell input keyevent KEYCODE_BACK
  sleep 1.5
}

tap text "F1–F12"
shot keys-function
tap text "Shortcuts"
shot keys-shortcuts
tap text "+  Add"
shot shortcut-dialog
tap text "Cancel"
tap content-desc "More"
shot more-menu
back
tap content-desc "Keyboard"
sleep 1.5
shot keyboard
back
tap content-desc "Hide keys"
# The host remakes its screen at the new size.
sleep 2
shot keys-hidden
tap content-desc "Show keys"
back
shot main
tap content-desc "Edit"
shot computer-dialog
tap text "Cancel"
