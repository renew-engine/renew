#!/usr/bin/env bash
# Produce the on-device evidence for the Android app lifecycle: build the
# APK for whatever is plugged in, install it, background and resume it,
# and read what the engine made of that.
#
# **Why a script rather than a paragraph in a document.** This evidence
# is owed by the milestone ritual and can only be produced where hardware
# is, which is a developer's desk and never CI. A recipe that has to be
# retyped from a document is a recipe that gets typed differently each
# time, and evidence that differs between runs is not evidence. So the
# whole thing is one command: this script finds the machine and builds
# the APK, and hands both to `android-lifecycle-core.sh`, which is the
# same cycling and counting the CI emulator lane runs.
#
# It works against an emulator too, and that is deliberate: a tool whose
# first execution is on the hardware it was written for is a tool nobody
# has tested. The distinction it will not blur is in the report - a
# device run and an emulator run say which they were.
set -euo pipefail

sample="renew-sample-input-echo"

command -v adb >/dev/null || {
    echo "adb is not on PATH; nothing here can reach a device" >&2
    exit 1
}

serial="$(adb devices | awk '$2 == "device" { print $1; exit }')"
if [ -z "$serial" ]; then
    echo "no device is connected and authorised. \`adb devices\` shows none - a phone \
needs USB debugging enabled and the host's key accepted on its screen." >&2
    exit 1
fi

case "$serial" in
    emulator-*) kind="emulator" ;;
    *) kind="device" ;;
esac
model="$(adb -s "$serial" shell getprop ro.product.model | tr -d '\r')"
release="$(adb -s "$serial" shell getprop ro.build.version.release | tr -d '\r')"
abi="$(adb -s "$serial" shell getprop ro.product.cpu.abi | tr -d '\r')"
echo "$kind: $model, Android $release, $abi ($serial)"

if [ "$kind" = "emulator" ]; then
    echo "NOTE: this is an emulator. It exercises the script and the app, and it is not"
    echo "the on-device evidence the milestone ritual asks for."
fi

# The ABI the attached machine actually runs, rather than both: building
# the other one would be time spent on a library this run cannot load.
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-${ANDROID_NDK_LATEST_HOME:-}}"
if [ -z "$ANDROID_NDK_HOME" ]; then
    echo "ANDROID_NDK_HOME is unset; cargo-ndk cannot find a toolchain" >&2
    exit 1
fi

# Gradle needs the SDK as much as cargo needs the NDK, and it reports
# its absence as a build failure two minutes in rather than as a missing
# variable at the start.
#
# **Not a hard requirement on the variable, though.** Android Studio
# writes the path into `local.properties`, which this project ignores as
# machine-local configuration precisely so that setup works — and a
# check that demanded the variable would reject the standard install
# while gradle sat there able to find the SDK perfectly well. So this
# refuses only when neither route exists.
export ANDROID_HOME="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
if [ -z "$ANDROID_HOME" ] && [ ! -f samples/input_echo/android/local.properties ]; then
    echo "no SDK: ANDROID_HOME is unset and there is no local.properties for gradle to read it from" >&2
    exit 1
fi

jni="samples/input_echo/android/app/src/main/jniLibs"
cargo ndk -t "$abi" -o "$jni" build -p "$sample" --release

(cd samples/input_echo/android && ./gradlew --quiet assembleDebug)
apk="samples/input_echo/android/app/build/outputs/apk/debug/app-debug.apk"
[ -s "$apk" ] || {
    echo "gradle produced no APK at $apk" >&2
    exit 1
}

# The cycling, the counting and the verdict are the same on a phone as
# on an emulator, so they live in one file that both callers use rather
# than in two that drift.
exec bash "$(dirname "$0")/android-lifecycle-core.sh" "$serial" "$apk" "$kind"
