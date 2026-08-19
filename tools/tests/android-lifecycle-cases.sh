#!/usr/bin/env bash
# Drive `android-lifecycle-core.sh` through the runs a real emulator will
# not produce on demand, and check that it reports each one correctly.
#
# **Why this exists.** The core's job is to refuse — the whole value of
# the lifecycle lane is that a run which did not happen is reported as a
# failure rather than a pass. That property is invisible on a green
# emulator: every honest run exercises the success path and none of the
# refusals, so the guards can rot into decoration and the lane stays
# green while checking nothing.
#
# It is not hypothetical. The first version of the core counted surface
# epochs and nothing else, and the `no background` case below — a log
# with three surfaces, two closures and not one suspend — passed it,
# printing "2 background/resume cycle(s)" about a run that was never
# backgrounded at all. `adb shell input keyevent KEYCODE_HOME` exits 0
# whether or not anything went to the background, so nothing upstream
# would have caught it either.
#
# No device, no emulator, no toolchain: a stand-in `adb` replays canned
# output, so this runs anywhere in about a minute.
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin"
echo "apk" > "$work/fake.apk"

cat > "$work/bin/adb" <<'FAKE'
#!/usr/bin/env bash
# Answers the four things the core asks a device, from the scenario in
# the environment. Everything else succeeds silently, as adb does.
case "$*" in
    *pidof*)
        seen=0
        [ -f "$FAKE_STATE" ] && seen="$(cat "$FAKE_STATE")"
        echo $((seen + 1)) > "$FAKE_STATE"
        # The first read is the launch, later ones are the end of the
        # run - which is where a crashed app has to be distinguishable.
        if [ "$seen" -eq 0 ]; then
            printf '%s\r\n' "$FAKE_PID_START"
        elif [ -n "$FAKE_PID_END" ]; then
            printf '%s\r\n' "$FAKE_PID_END"
        fi
        exit 0
        ;;
    *run-as*) cat "$FAKE_LOG"; exit 0 ;;
    *) exit 0 ;;
esac
FAKE
chmod +x "$work/bin/adb"
export PATH="$work/bin:$PATH"

# Two clean cycles: one launch, and a suspend/close/resume/open quartet
# per background. This is what the emulator lane actually prints.
cat > "$work/honest.log" <<'LOG'
INFO diagnostics: input_echo: android start
ready: 320x640 at scale 1
focus false
suspended: the app is in the background
surface lost: epoch closed, awaiting the next ready
resumed: the app is in the foreground again
ready: 320x640 at scale 1
focus false
suspended: the app is in the background
surface lost: epoch closed, awaiting the next ready
resumed: the app is in the foreground again
ready: 320x640 at scale 1
LOG

# The surfaces churned, but the app was never backgrounded: the keyevent
# went to a dialog, or to a window that never took focus. The epoch
# counts are identical to the honest run, which is the point.
grep -v '^suspended:\|^resumed:' "$work/honest.log" > "$work/no-background.log"

# The app was backgrounded twice and produced a third epoch anyway -
# a surface thrashed across a transition. A floor would call this a pass.
cp "$work/honest.log" "$work/surplus.log"
cat >> "$work/surplus.log" <<'LOG'
surface lost: epoch closed, awaiting the next ready
ready: 320x640 at scale 1
LOG

# Two launch announcements: a container left over from an earlier run,
# so the log is two runs' lines with one run's story read out of them.
cat "$work/honest.log" "$work/honest.log" > "$work/two-runs.log"

: > "$work/empty.log"

failures=0
check() {
    local name="$1" want="$2" log="$3" pid_end="$4" expect="$5"
    rm -f "$work/state"
    local out rc
    set +e
    out="$(FAKE_STATE="$work/state" FAKE_LOG="$log" FAKE_PID_START=4242 \
        FAKE_PID_END="$pid_end" CYCLES=2 \
        bash "$root/tools/android-lifecycle-core.sh" fake-serial "$work/fake.apk" emulator 2>&1)"
    rc=$?
    set -e

    if { [ "$want" = "pass" ] && [ "$rc" -ne 0 ]; } ||
        { [ "$want" = "fail" ] && [ "$rc" -eq 0 ]; }; then
        echo "FAIL  $name: wanted it to $want, it exited $rc"
        printf '%s\n' "$out" | sed 's/^/        /'
        failures=$((failures + 1))
        return
    fi
    # A refusal for the wrong reason is not the check working; it is a
    # second bug wearing the first one's exit code.
    if ! printf '%s' "$out" | grep -q "$expect"; then
        echo "FAIL  $name: exited $rc as wanted, but did not say why"
        echo "        expected to find: $expect"
        printf '%s\n' "$out" | sed 's/^/        /'
        failures=$((failures + 1))
        return
    fi
    echo "ok    $name"
}

check "a clean two-cycle run is reported"     pass "$work/honest.log"       4242 "OBSERVED on this emulator: 3 surface epochs"
check "surfaces without a background refuse"  fail "$work/no-background.log" 4242 "expected 2 suspends and 2 resumes"
check "a surplus epoch refuses"               fail "$work/surplus.log"      4242 "expected exactly 3 ready"
check "two runs in one log refuse"            fail "$work/two-runs.log"     4242 "launch announcements, so it is not one run"
check "an app gone at the end refuses"        fail "$work/honest.log"       ""   "did not survive the last cycle"
check "an unreadable log refuses"             fail "$work/empty.log"        4242 "wrote no readable log"

if [ "$failures" -ne 0 ]; then
    echo
    echo "$failures case(s) wrong: the lifecycle lane cannot be trusted to refuse" >&2
    exit 1
fi
echo
echo "all six cases behaved: the core reports a real run and refuses five ways of not being one"
