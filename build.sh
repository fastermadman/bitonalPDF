#!/bin/bash
# Builds bitonalPDF.app (a drag-and-drop droplet) in the current directory.
set -euo pipefail
cd "$(dirname "$0")"
rm -rf bitonalPDF.app
osacompile -o bitonalPDF.app droplet.applescript
cp bitonalpdf.sh bitonalPDF.app/Contents/Resources/
chmod +x bitonalPDF.app/Contents/Resources/bitonalpdf.sh
# Tahoe (macOS 26) shows legacy icons inside a grey plate. assets/compiled/ holds the icon compiled
# from assets/AppIcon.icon by the "compile-icon" GitHub workflow (needs Xcode's actool).
R=bitonalPDF.app/Contents/Resources
rm -f $R/droplet.icns
cp assets/compiled/Assets.car $R/Assets.car
cp assets/compiled/AppIcon.icns $R/AppIcon.icns
plutil -replace CFBundleIconFile -string AppIcon bitonalPDF.app/Contents/Info.plist
plutil -replace CFBundleIconName -string AppIcon bitonalPDF.app/Contents/Info.plist
codesign --force --deep -s - bitonalPDF.app
echo "Built bitonalPDF.app — drag it to ~/Applications"
