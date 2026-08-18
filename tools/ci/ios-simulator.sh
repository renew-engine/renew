#!/usr/bin/env bash
# Choose an iOS simulator and print it as: udid<TAB>name<TAB>runtime.
#
# **One copy, because there were three.** Each iOS lane needs the same
# device and each grew its own selector; by the third they had already
# drifted - one printed the runtime, one did not, one said something
# different when the chosen device had no udid. A thirty-line parser
# duplicated three ways is the shape §15 names, and the drift is what it
# predicts.
#
# Prints to stdout so a caller can read it with `cut`, and says what went
# wrong in its own words on stderr rather than letting a traceback name a
# Python line to somebody reading a red lane.
set -euo pipefail

xcrun simctl list devices available --json | python3 -c "
import json, sys

try:
    catalogue = json.load(sys.stdin)['devices']
except (ValueError, KeyError) as problem:
    raise SystemExit(f'simctl device list was not the shape expected: {problem}')

# Newest runtime wins. The identifiers sort as text, so a plain sort puts
# iOS-9 above iOS-26 and 26-5 above 26-10; comparing the numbers between
# the dashes is what keeps the newest one newest.
def version(runtime):
    tail = runtime.rsplit('.', 1)[-1]
    parts = [p for p in tail.split('-') if p.isdigit()]
    return [int(p) for p in parts] or [0]

best = None
for runtime, devices in catalogue.items():
    if 'iOS' not in runtime:
        continue
    for device in devices:
        if not device.get('isAvailable'):
            continue
        if 'iPhone' not in device.get('name', ''):
            continue
        if best is None or version(runtime) > version(best[0]):
            best = (runtime, device.get('udid'), device.get('name'))

if best is None:
    raise SystemExit('no available iOS iPhone simulator on this runner')

runtime, udid, name = best
if not udid:
    raise SystemExit(f'the chosen simulator ({name}) has no udid')

print(f'{udid}\t{name}\t{runtime}')
"
