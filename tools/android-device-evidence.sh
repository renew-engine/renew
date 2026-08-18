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
# cycle, the counting and the verdict live here, and the person with the
# phone runs one command.
#
# It works against an emulator too, and that is deliberate: a tool whose
# first execution is on the hardware it was written for is a tool nobody
# has tested. The distinction it will not blur is in the report - a
# device run and an emulator run say which they were.
set -euo pipefail

cycles="${CYCLES:-3}"
package="com.renewengine.inputecho"
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

jni="samples/input_echo/android/app/src/main/jniLibs"
cargo ndk -t "$abi" -o "$jni" build -p "$sample" --release

(cd samples/input_echo/android && ./gradlew --quiet assembleDebug)
apk="samples/input_echo/android/app/build/outputs/apk/debug/app-debug.apk"
[ -s "$apk" ] || {
    echo "gradle produced no APK at $apk" >&2
    exit 1
}

# A fresh install every time: the app appends to its log, so a container
# left from an earlier run would have this run counting somebody else's
# lines.
adb -s "$serial" uninstall "$package" >/dev/null 2>&1 || true
adb -s "$serial" install -r "$apk" >/dev/null
echo "installed $(basename "$apk")"

launch() {
    adb -s "$serial" shell monkey -p "$package" -c android.intent.category.LAUNCHER 1 \
        >/dev/null 2>&1
    sleep 4
}

echo "launching, then backgrounding and resuming $cycles time(s)"
launch
pid="$(adb -s "$serial" shell pidof "$package" | tr -d '\r')"
[ -n "$pid" ] || {
    echo "the app did not start, so there is nothing to observe" >&2
    exit 1
}

for _ in $(seq "$cycles"); do
    adb -s "$serial" shell input keyevent KEYCODE_HOME
    sleep 3
    launch
done

after="$(adb -s "$serial" shell pidof "$package" | tr -d '\r')"

log="$(adb -s "$serial" shell run-as "$package" cat files/input_echo.log 2>/dev/null || true)"
if [ -z "$log" ]; then
    echo "the app wrote no readable log. On a device this usually means the build is not"
    echo "debuggable, since \`run-as\` only works for a debuggable package." >&2
    exit 1
fi

echo "--- the app's own log ---"
printf '%s\n' "$log"
echo "--- counted ---"
ready="$(printf '%s\n' "$log" | grep -c '^ready:' || true)"
lost="$(printf '%s\n' "$log" | grep -c '^surface lost:' || true)"
starts="$(printf '%s\n' "$log" | grep -c 'android start' || true)"
echo "ready: $ready   surface-lost: $lost   launch announcements: $starts"
echo "process: $pid at first launch, $after at the end"

# **What the counts have to show before this is evidence of anything.**
# Android revokes the window when it backgrounds an activity, so a real
# cycle leaves one surface-lost per background and one ready per
# foreground. Zero of either means the cycle did not happen - a phone
# that never backgrounded the app, or an app that never came back - and
# reporting that as a passing lifecycle would be reporting the absence of
# a test as the result of one.
if [ "$starts" -ne 1 ]; then
    echo "the log carries $starts launch announcements, so it is not one run's record" >&2
    exit 1
fi
if [ "$ready" -lt $((cycles + 1)) ] || [ "$lost" -lt "$cycles" ]; then
    echo "expected at least $((cycles + 1)) ready and $cycles surface-lost for $cycles \
cycle(s); the app was not backgrounded and resumed as this run assumed" >&2
    exit 1
fi
if [ "$pid" != "$after" ]; then
    echo "NOTE: the process id changed ($pid -> $after), so the OS restarted the app rather"
    echo "than backgrounding it. The epochs above are still real, but they span processes."
fi

echo
echo "OBSERVED on this $kind: $ready surface epochs opened and $lost closed across"
echo "$cycles background/resume cycle(s), in $([ "$pid" = "$after" ] && echo "one process" || echo "more than one process")."
