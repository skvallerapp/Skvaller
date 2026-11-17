#!/bin/sh
# Cargo's runner on macOS. It signs the app with your Apple Development certificate
# before starting it. The keychain trusts an app by its signature, and the linker's
# own ad-hoc signature changes with every build, so without this macOS asks for your
# password after each rebuild. With a stable certificate, "Always Allow" sticks.
#
# Uses the first Apple Development certificate in your keychain, or the identity in
# SKVALLER_SIGN_IDENTITY. Without one, the app runs unsigned.

bin="$1"
if [ "$(basename "$bin")" = skvaller ]; then
  identity="${SKVALLER_SIGN_IDENTITY:-$(security find-identity -v -p codesigning |
    awk '/"Apple Development/ { print $2; exit }')}"
  if [ -z "$identity" ]; then
    echo "macos-run: no Apple Development certificate, so the app is unsigned" >&2
  elif ! codesign --force --sign "$identity" \
    --identifier io.github.skvallerapp.Skvaller "$bin"; then
    echo "macos-run: signing failed, so the app is unsigned" >&2
  fi
fi
exec "$@"
