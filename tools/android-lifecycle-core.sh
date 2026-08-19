#!/usr/bin/env bash
# Background and resume an installed Android sample, then read what the
# engine made of that.
#
# Usage: android-lifecycle-core.sh <serial> <apk>   (CYCLES=3 by default)
#
# **Why this is its own file.** Two callers need exactly this: the desk
# tool that runs against a phone, and the CI lane that runs against an
# emulator. They differ in how they get a machine and how they get an
# APK, and in nothing else — so the cycling, the counting and the
# verdict live here once. Two copies of a check are two checks that drift
# apart, and the one that drifts quiet is the one still being trusted.
set -euo pipefail

serial="${1:?usage: android-lifecycle-core.sh <serial> <apk>}"
apk="${2:?usage: android-lifecycle-core.sh <serial> <apk>}"
cycles="${CYCLES:-3}"
package="com.renewengine.inputecho"

[ -s "$apk" ] || {
    echo "no APK at $apk" >&2
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
    echo "the app wrote no readable log. This usually means the build is not debuggable," >&2
    echo "since \`run-as\` only works for a debuggable package." >&2
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
# foreground. Zero of either means the cycle did not happen — a machine
# that never backgrounded the app, or an app that never came back — and
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
echo "$ready surface epochs opened and $lost closed across $cycles background/resume"
echo "cycle(s), in $([ "$pid" = "$after" ] && echo "one process" || echo "more than one process")."
