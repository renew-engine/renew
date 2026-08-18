#!/usr/bin/env bash
# Assemble an iOS application bundle around a sample's binary.
#
# **An iOS app is a directory.** `Foo.app/` holding an executable and an
# `Info.plist` naming it is what `xcrun simctl install` accepts, and a
# Rust `[[bin]]` built for `aarch64-apple-ios-sim` is that executable:
# winit's iOS backend enters `UIApplicationMain` from the event loop, so
# the binary *is* the app.
#
# That is why this repository commits no Xcode project. A
# `project.pbxproj` is a large, order-sensitive file that no tool here
# can generate or open, and its first validation would be CI. A device
# build would be different - installing on hardware needs signing and
# provisioning, which is where a real project earns its keep - but a
# simulator build needs none of it.
#
# Usage: ios-app-bundle.sh <package> <binary-name> <bundle-id> <out-dir>
set -euo pipefail

package="$1"
binary="$2"
bundle_id="$3"
out_dir="$4"

target="aarch64-apple-ios-sim"
cargo build --package "$package" --bin "$binary" --target "$target"

built="target/$target/debug/$binary"
if [ ! -f "$built" ]; then
    echo "ios-app-bundle: cargo produced no $built" >&2
    exit 1
fi

app="$out_dir/$binary.app"
rm -rf "$app"
mkdir -p "$app"
cp "$built" "$app/$binary"

# The smallest plist the simulator accepts, and every key in it is load
# bearing: without `CFBundleExecutable` there is nothing to launch,
# without `CFBundleIdentifier` there is nothing to launch it *by*, and
# without the platform keys the installer rejects the bundle as built
# for something else. `UILaunchScreen` is there because a bundle without
# one is treated as a compatibility app and letterboxed; whether that
# would change what this sample reports has not been measured here, and
# the key is cheap enough not to find out the hard way.
cat > "$app/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>$binary</string>
    <key>CFBundleIdentifier</key>
    <string>$bundle_id</string>
    <key>CFBundleName</key>
    <string>$binary</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.1</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>MinimumOSVersion</key>
    <string>15.0</string>
    <key>CFBundleSupportedPlatforms</key>
    <array>
        <string>iPhoneSimulator</string>
    </array>
    <key>UIDeviceFamily</key>
    <array>
        <integer>1</integer>
    </array>
    <key>UILaunchScreen</key>
    <dict/>
</dict>
</plist>
PLIST

echo "$app"
