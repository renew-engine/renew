#!/usr/bin/env bash
# The Android emulator lifecycle lane's body.
#
# **A file, not lines in the workflow.** The emulator action runs its
# `script:` input one line per `sh -c`: variables do not survive between
# lines, `set -eu` applies to nothing after itself, and any multi-line
# construct is a syntax error at the first line break. The determinism
# lane lost two CI runs learning that; this one inherits the lesson
# rather than repeating it.
set -euo pipefail

# **Asserted, not printed.** The APK this lane installs carries an
# x86_64 shared object and nothing else, so an emulator of another
# architecture would install it and then fail to load the library — a
# failure that reads as a broken app rather than a mismatched machine.
# Saying which it is here costs one line and saves the next reader an
# afternoon.
device_arch="$(adb shell uname -m | tr -d '\r')"
echo "emulator reports: $device_arch"
if [ "$device_arch" != "x86_64" ]; then
    echo "the emulator is '$device_arch'; this lane built an x86_64 library" >&2
    exit 1
fi

serial="$(adb devices | awk '$2 == "device" { print $1; exit }')"
[ -n "$serial" ] || {
    echo "the emulator action reported ready, but no authorised device is attached" >&2
    exit 1
}

# **Two cycles, not the desk tool's three.** Each is seven seconds of
# sleeping, and the property under test — that backgrounding revokes the
# surface and returning grants a new one — is not more true at three
# than at two. What a third would buy is a longer lane on a runner that
# has already booted a virtual machine to get here.
CYCLES=2 exec bash tools/android-lifecycle-core.sh "$serial" \
    samples/input_echo/android/app/build/outputs/apk/debug/app-debug.apk
