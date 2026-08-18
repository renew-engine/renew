#!/usr/bin/env bash
# Drive an iOS app through background and foreground, and read what the
# engine made of it.
#
# **This is the only lane that can observe what an interruption does to
# a window on iOS.** The platform code revokes the surface on suspend
# only under `cfg!(target_os = "android")`, on the argument that iOS
# keeps it. That was reasoned about long before it was watched: the
# determinism lane cannot watch it, because eleven headless
# command-line simulations never open a window or enter an event loop,
# so no suspend event reaches the handler at all.
#
# So an app is what settles it, and the counting is the finding in either
# direction. On Android the same cycle produced three surface-lost events
# against four ready events, because Android takes the window away. If
# iOS keeps it, the expected shape here is **one ready and no surface
# lost**, with the app resuming into the epoch it already had. If instead
# surface-lost lines appear, the `cfg!` guard is hiding a real difference
# and the platform layer is wrong about this platform.
set -euo pipefail

bundle_id="com.renewengine.inputecho"
binary="input_echo"

device_json="$(xcrun simctl list devices available --json)"
device="$(printf '%s' "$device_json" | python3 -c "
import json, sys

try:
    catalogue = json.load(sys.stdin)['devices']
except (ValueError, KeyError) as problem:
    raise SystemExit(f'simctl device list was not the shape expected: {problem}')

def version(runtime):
    tail = runtime.rsplit('.', 1)[-1]
    parts = [p for p in tail.split('-') if p.isdigit()]
    return [int(p) for p in parts] or [0]

best = None
for runtime, devices in catalogue.items():
    if 'iOS' not in runtime:
        continue
    for device in devices:
        if not device.get('isAvailable') or 'iPhone' not in device.get('name', ''):
            continue
        if best is None or version(runtime) > version(best[0]):
            best = (runtime, device.get('udid'))

if best is None or not best[1]:
    raise SystemExit('no available iOS iPhone simulator on this runner')
print(best[1])
")"

echo "booting $device"
xcrun simctl boot "$device" || true
xcrun simctl bootstatus "$device" -b

app="$(bash tools/ios-app-bundle.sh renew-sample-input-echo "$binary" "$bundle_id" target/ios-app)"
echo "bundled $app"

xcrun simctl uninstall "$device" "$bundle_id" >/dev/null 2>&1 || true
xcrun simctl install "$device" "$app"

# Three cycles, so a single lucky reading cannot be mistaken for a rule.
#
# **Backgrounding means giving the foreground to something else, and the
# something else has to be offline.** The first version opened a URL,
# which hands the foreground to Safari - and on a hosted runner that
# times out after fifty seconds, because the simulator has no route to
# the internet. Launching a bundled app instead needs no network and no
# network is what this lane has. Settings is present in every runtime;
# Safari is the fallback, launched rather than sent to a URL.
background() {
    xcrun simctl launch "$device" com.apple.Preferences >/dev/null 2>&1 ||
        xcrun simctl launch "$device" com.apple.mobilesafari >/dev/null 2>&1 ||
        {
            echo "could not give the foreground to another app, so the sample was never backgrounded and this lane observed nothing about suspend" >&2
            exit 1
        }
    sleep 3
}

launch() {
    xcrun simctl launch "$device" "$bundle_id" >/dev/null
    sleep 3
}

echo "launching, then backgrounding and resuming three times"
launch

container="$(xcrun simctl get_app_container "$device" "$bundle_id" data)"
log="$container/Documents/input_echo.log"
after_launch=0
if [ -f "$log" ]; then
    after_launch="$(wc -l < "$log" | tr -d ' ')"
fi

for _ in 1 2 3; do
    background
    launch
done

# The app's sandbox is a real directory on this host, so the log is read
# rather than pulled.
if [ ! -f "$log" ]; then
    echo "the app wrote no log at $log, so it either never started or could not \
write - either way this lane learned nothing and must not report that it did" >&2
    xcrun simctl spawn "$device" log show --last 2m --predicate "processImagePath contains \
'$binary'" 2>/dev/null | tail -40 >&2 || true
    exit 1
fi

echo "--- the app's own log ---"
cat "$log"
echo "--- counted ---"
ready="$(grep -c '^ready:' "$log" || true)"
lost="$(grep -c '^surface lost:' "$log" || true)"
echo "ready events: $ready"
echo "surface-lost events: $lost"

# **A count of zero readys means the app never opened a window**, which
# is a lane that measured nothing rather than a platform that behaved.
if [ "$ready" -lt 1 ]; then
    echo "the app never reported a window, so nothing was observed about suspend" >&2
    exit 1
fi

# **Zero surface-lost lines is only evidence if a suspend actually
# happened**, and this is the check that decides which it was.
#
# The two readings are indistinguishable from that count alone: "iOS
# kept the surface across a suspend" and "the app was never suspended"
# both produce zero. So the app says when it was suspended and when it
# came back - the platform layer tells it, separately from the surface,
# for exactly this reason - and the lane requires both to have happened
# before it reports anything about surfaces.
suspends="$(grep -c '^suspended:' "$log" || true)"
resumes="$(grep -c '^resumed:' "$log" || true)"
echo "suspends: $suspends"
echo "resumes: $resumes"
now="$(wc -l < "$log" | tr -d ' ')"
echo "log lines after first launch: $after_launch, after three cycles: $now"

# **The log is appended to and never reset, so counts alone could come
# from a previous run.** The uninstall above should have taken the old
# container with it, and its failure is deliberately tolerated - but a
# tolerated failure is exactly the one nobody notices. Two checks close
# it: the app announced itself exactly once, so this is one process's
# log and not two; and the log grew after the first launch, so the
# lines being counted were written by the cycle this run drove.
starts="$(grep -c 'input_echo: ios start' "$log" || true)"
if [ "$starts" -ne 1 ]; then
    echo "the log carries $starts launch announcements, so it is not one run's record - \
a container survived from an earlier run and these counts describe something else" >&2
    exit 1
fi
if [ "$now" -le "$after_launch" ]; then
    echo "the log did not grow across three background-and-resume rounds (it had \
$after_launch lines and has $now), so nothing was counted that this cycle produced" >&2
    exit 1
fi

if [ "$suspends" -lt 1 ] || [ "$resumes" -lt 1 ]; then
    echo "the app was not suspended and resumed (suspends=$suspends, resumes=$resumes), \
so this run cannot show what a suspend does to the surface. That is a fault in how this \
lane backgrounds an app, not a finding about the platform - and reporting it as one is \
the failure this check exists to prevent." >&2
    exit 1
fi

if [ "$lost" -eq 0 ]; then
    echo "OBSERVED: the app was suspended $suspends time(s) and resumed $resumes time(s), \
and no surface was lost - which is what the platform layer assumes when it revokes the \
window on Android alone"
else
    echo "OBSERVED: iOS revoked the surface $lost time(s). The platform layer assumes it \
does not, and that assumption is what this lane exists to check - the guard around the \
revocation needs to cover this platform too" >&2
    exit 1
fi
