#!/bin/bash
# Builds bitonalPDF.app (a drag-and-drop droplet) in the current directory.
set -euo pipefail
cd "$(dirname "$0")"
rm -rf bitonalPDF.app
osacompile -o bitonalPDF.app droplet.applescript
cp bitonalpdf.sh bitonalPDF.app/Contents/Resources/
chmod +x bitonalPDF.app/Contents/Resources/bitonalpdf.sh
cp assets/AppIcon.icns bitonalPDF.app/Contents/Resources/droplet.icns
# osacompile ships a compiled icon catalog that macOS prefers over the .icns; drop it
rm -f bitonalPDF.app/Contents/Resources/Assets.car
plutil -remove CFBundleIconName bitonalPDF.app/Contents/Info.plist
codesign --force --deep -s - bitonalPDF.app
echo "Built bitonalPDF.app — drag it to ~/Applications"
