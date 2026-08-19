#!/usr/bin/env bash
# Background and resume an installed Android sample, then read what the
# engine made of that.
#
# Usage: android-lifecycle-core.sh <serial> <apk> <kind>  (CYCLES=3)
#   kind: "device" or "emulator" — it appears in the verdict, because a
#   count means different things depending on what produced it.
#
# **Why this is its own file.** Two callers need exactly this: the desk
# tool that runs against a phone, and the CI lane that runs against an
# emulator. They differ in how they get a machine and how they get an
# APK, and in nothing else — so the cycling, the counting and the
# verdict live here once. Two copies of a check are two checks that
# drift apart, and the one that drifts quiet is the one still being
# trusted.
set -euo pipefail

serial="${1:?usage: android-lifecycle-core.sh <serial> <apk> <kind>}"
apk="${2:?usage: android-lifecycle-core.sh <serial> <apk> <kind>}"
kind="${3:?usage: android-lifecycle-core.sh <serial> <apk> <kind>}"
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

# **`|| true` on every process read, deliberately.** `pidof` exits 1 when
# it finds nothing, and under `pipefail` that status propagates through
# the pipe and out of the assignment, where `set -e` kills the script
# with no message at all. The two "the app is gone" messages below exist
# to be read; without this they were unreachable code, and the script's
# way of reporting a dead app was to exit silently.
process_id() {
    adb -s "$serial" shell pidof "$package" 2>/dev/null | tr -d '\r' || true
}

echo "launching, then backgrounding and resuming $cycles time(s)"
launch
pid="$(process_id)"
[ -n "$pid" ] || {
    echo "the app did not start, so there is nothing to observe" >&2
    exit 1
}

for _ in $(seq "$cycles"); do
    adb -s "$serial" shell input keyevent KEYCODE_HOME
    sleep 3
    launch
done

after="$(process_id)"

log="$(adb -s "$serial" shell run-as "$package" cat files/input_echo.log 2>/dev/null || true)"
if [ -z "$log" ]; then
    echo "the app wrote no readable log. This usually means the build is not debuggable," >&2
    echo "since \`run-as\` only works for a debuggable package." >&2
    exit 1
fi

# Kept where a later reader can open it: this device is about to be
# destroyed, and an observation nobody can re-read is thin evidence.
printf '%s\n' "$log" > android-lifecycle.log

echo "--- the app's own log ---"
printf '%s\n' "$log"
echo "--- counted ---"

# **Defaulted to zero, because an empty count silently disables the
# guard below it.** `grep -c` prints nothing and exits 2 on a read
# error, `|| true` swallows the status, and `[ "" -lt 3 ]` then errors
# with status 2 — which bash treats as a false condition, so the `if`
# does not fire and the script walks on to its success message.
count() {
    local n
    n="$(printf '%s\n' "$log" | grep -c "$1" || true)"
    printf '%s' "${n:-0}"
}
ready="$(count '^ready:')"
lost="$(count '^surface lost:')"
suspends="$(count '^suspended:')"
resumes="$(count '^resumed:')"
starts="$(count 'android start')"
echo "ready: $ready   surface-lost: $lost   suspended: $suspends   resumed: $resumes"
echo "launch announcements: $starts"
echo "process: $pid at first launch, ${after:-<gone>} at the end"

# **The counts have to show the cycle, not merely add up.** Surface
# epochs alone cannot: `input keyevent KEYCODE_HOME` exits 0 whether or
# not anything went to the background, so a run where nothing was ever
# backgrounded — a dialog swallowing the keyevent, a window that never
# took focus — can still accumulate epochs from an unrelated churn and
# satisfy a count. The claim being made here is causal (backgrounding
# revokes the surface, returning grants a new one), so the suspend and
# the resume are what have to be counted, and the surfaces are read
# against them. The app logs them separately for exactly this reason,
# and the iOS lane has required both since it was written.
if [ "$starts" -ne 1 ]; then
    echo "the log carries $starts launch announcements, so it is not one run's record" >&2
    exit 1
fi
if [ "$suspends" -ne "$cycles" ] || [ "$resumes" -ne "$cycles" ]; then
    echo "expected $cycles suspends and $cycles resumes; the app was not backgrounded and \
resumed as this run assumed, so the surface counts below describe something else" >&2
    exit 1
fi
# **Exact, not a floor.** A missing epoch is a lifecycle the engine
# failed to observe; a surplus one is an app thrashing its surface
# across a transition. Both are findings, and `>=` reports the second as
# success. Every run recorded here — emulator and CI alike — has
# produced exactly one closed epoch per background and one opened per
# foreground, so the model is not a guess.
if [ "$ready" -ne $((cycles + 1)) ] || [ "$lost" -ne "$cycles" ]; then
    echo "expected exactly $((cycles + 1)) ready and $cycles surface-lost for $cycles \
cycle(s); the app was backgrounded $suspends time(s), so the surface count is the \
surprise here, not the lifecycle" >&2
    exit 1
fi
if [ -z "$after" ]; then
    echo "the app is not running at the end of the run, so it did not survive the last \
cycle" >&2
    exit 1
fi
if [ "$pid" != "$after" ]; then
    echo "NOTE: the process id changed ($pid -> $after), so the OS restarted the app rather"
    echo "than backgrounding it. The epochs above are still real, but they span processes."
fi

echo
echo "OBSERVED on this $kind: $ready surface epochs opened and $lost closed across"
echo "$cycles background/resume cycle(s), in $([ "$pid" = "$after" ] && echo "one process" || echo "more than one process")."
