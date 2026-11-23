#!/bin/sh
# Builds Skvaller.app in target/release/bundle, and with --dmg also
# Skvaller-<version>.dmg, which opens to the app and a link to Applications.
#
# Signs with the identity in SKVALLER_SIGN_IDENTITY, or the first Apple Development
# certificate in your keychain, or else ad hoc. Not notarized: on another Mac,
# Gatekeeper blocks the first launch until you allow it in System Settings → Privacy
# & Security → Open Anyway. Built for this Mac's architecture only.
#
# Usage: scripts/macos-bundle.sh [--dmg]
set -eu

dmg=false
case "${1:-}" in
  --dmg) dmg=true ;;
  "") ;;
  *) echo "usage: $0 [--dmg]" >&2; exit 2 ;;
esac

cd "$(dirname "$0")/.."
version=$(cargo pkgid -p skvaller | sed 's/.*[#@]//')

cargo build --release --locked -p skvaller

out=target/release/bundle
app="$out/Skvaller.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/skvaller "$app/Contents/MacOS/skvaller"
cp crates/skvaller/Skvaller.icns "$app/Contents/Resources/Skvaller.icns"
cp crates/skvaller/Info.plist "$app/Contents/Info.plist"
plutil -replace CFBundleShortVersionString -string "$version" "$app/Contents/Info.plist"
plutil -replace CFBundleVersion -string "$version" "$app/Contents/Info.plist"

identity="${SKVALLER_SIGN_IDENTITY:-$(security find-identity -v -p codesigning |
  awk '/"Apple Development/ { print $2; exit }')}"
codesign --force --sign "${identity:--}" "$app"
echo "Built $app (signed ${identity:-ad hoc})"

if $dmg; then
  stage=$(mktemp -d)
  trap 'rm -rf "$stage"' EXIT
  cp -R "$app" "$stage/"
  ln -s /Applications "$stage/Applications"
  file="$out/Skvaller-$version.dmg"
  rm -f "$file"
  hdiutil create -volname Skvaller -srcfolder "$stage" -format UDZO -quiet "$file"
  echo "Built $file"
fi
