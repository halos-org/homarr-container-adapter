#!/bin/bash
# Custom rename script for arm64 architecture packages
set -euo pipefail

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --version) DEBIAN_VERSION="$2"; shift 2 ;;
        --distro) APT_DISTRO="$2"; shift 2 ;;
        --component) APT_COMPONENT="$2"; shift 2 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

PACKAGE_NAME="homarr-container-adapter"
ARCH="arm64"

OLD_NAME="${PACKAGE_NAME}_${DEBIAN_VERSION}_${ARCH}.deb"
NEW_NAME="${PACKAGE_NAME}_${DEBIAN_VERSION}_${ARCH}+${APT_DISTRO}+${APT_COMPONENT}.deb"

if [ -f "$OLD_NAME" ]; then
    echo "Renaming package: $OLD_NAME -> $NEW_NAME"
    mv "$OLD_NAME" "$NEW_NAME"
    echo "Package renamed successfully"
else
    echo "Error: Expected package not found: $OLD_NAME"
    echo "Available .deb files:"
    ls -la *.deb 2>/dev/null || echo "None found"
    exit 1
fi
