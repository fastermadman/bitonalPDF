#!/bin/bash
# Builds bitonalPDF.app (a drag-and-drop droplet) in the current directory.
set -euo pipefail
cd "$(dirname "$0")"
rm -rf bitonalPDF.app
osacompile -o bitonalPDF.app droplet.applescript
cp bitonalpdf.sh bitonalPDF.app/Contents/Resources/
chmod +x bitonalPDF.app/Contents/Resources/bitonalpdf.sh
cp assets/AppIcon.icns bitonalPDF.app/Contents/Resources/droplet.icns
codesign --force --deep -s - bitonalPDF.app
echo "Built bitonalPDF.app — drag it to ~/Applications"
