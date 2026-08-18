#!/usr/bin/env bash
# Run one binary on a connected Android device or emulator.
#
# Cargo's `CARGO_TARGET_<TRIPLE>_RUNNER` names this script, so
# `cargo run --target x86_64-linux-android` executes there instead of
# here — which is what lets the determinism lane produce a device leg
# through exactly the code path that produces a desktop one.
#
# Contract, and every line of it matters to a lane that must not report
# a divergence for an infrastructure failure:
#
#   * a push or exec failure exits non-zero and adb's own message goes
#     to this script's stderr, so the step fails as plumbing rather than
#     as disagreement. Whether a caller shows that message is the
#     caller's business: `renew determinism --emit` currently prints its
#     own summary and not the child's stderr, so on that path the
#     distinguishing detail is in the job log rather than in the error;
#   * the program's stdout is this script's stdout, and nothing else is
#     written there, because the caller parses its last JSON line. Not
#     byte-for-byte verbatim: adb is known to inject carriage returns
#     (the status read below strips them for exactly that reason), and
#     what makes that harmless is the caller trimming each line rather
#     than anything this script does;
#   * the program's exit code is this script's exit code, carried back
#     through a file because `adb shell` returns the shell's status
#     rather than the program's.
set -euo pipefail

# Two facts that cost an afternoon on a Windows host, harmless on the
# Linux runner this lane uses in CI:
#
#   * cargo cannot execute a `.sh` runner directly there, so the runner
#     variable has to name the interpreter — `bash <this script>`;
#   * the MSYS shell rewrites a leading `/data/local/tmp` into a Windows
#     path before `adb` sees it, which pushes the binary somewhere the
#     device does not have and fails on a message about the host's own
#     `C:/Program Files`. `MSYS_NO_PATHCONV=1` is what stops it.
export MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL="*"

binary="$1"
shift

if [ ! -x "$binary" ] && [ ! -f "$binary" ]; then
    echo "android-runner: no such binary: $binary" >&2
    exit 127
fi

# **One directory per invocation, not per binary name.** Under the one
# path a shell user always owns on a device.
#
# The name alone is not unique enough, and the difference is not
# theoretical: two runs of the same binary would share both the pushed
# file and the status file below, and the exit code this script returns
# is whatever that file holds when it is read. Racing runs then report
# each other's exit codes — a program that failed reported as a program
# that passed, which for a lane whose whole purpose is refusing to claim
# unmeasured success is the worst answer available. `PINNED_RUNS` runs
# sequentially today and CI gives this job its own emulator, so the race
# is not reachable; a precondition that holds by accident and is written
# nowhere is not a precondition, so the path carries the process id and
# the hazard is gone rather than merely absent.
# **Every argument is quoted for the device's shell, one at a time.**
# `adb shell` hands its argument to a shell over there, which
# re-parses it. An unquoted expansion would split an argument
# containing a space into several — so a pinned run would execute
# with a different argv than cargo asked for, and a determinism lane
# would read that as a divergence rather than as the plumbing fault
# it is — and would let a metacharacter run a command of its own on
# the device. Single quotes, with the standard escape for an
# embedded quote, are what survive that re-parse intact.
quote_for_device() {
    printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

name="$(basename "$binary")"
run_directory="/data/local/tmp/renew-runner/$$"
remote="$run_directory/$name"

# Quoted with the same helper as the arguments, for the same reason.
# The basename comes from cargo today, and cargo cannot produce one
# containing a quote — rustc refuses such crate names. But this script
# takes a path from whoever runs it, the quoting rule is the same rule,
# and a fix applied to the arguments and not to the path is the same
# defect wearing a different name. `mkdir -m 700`, because the adb
# shell's umask is 000 and the default would let any other uid on the
# device replace the pushed binary between the push and the exec.
quoted_directory="$(quote_for_device "$run_directory")"
quoted_remote="$(quote_for_device "$remote")"

adb shell "mkdir -p -m 700 $quoted_directory" >&2
adb push "$binary" "$remote" >&2
adb shell "chmod 755 $quoted_remote" >&2

# `adb shell` exits with the shell's status, not the program's, so the
# program's own code is written on the device and read back. Without
# this a crashing simulation would look like a successful run that
# happened to print nothing, which the caller would then report as a
# missing digest rather than as a failure.

command_line="$quoted_remote"
for argument in "$@"; do
    command_line="$command_line $(quote_for_device "$argument")"
done

status_file="$remote.status"
quoted_status="$(quote_for_device "$status_file")"

adb shell "cd $quoted_directory && $command_line ; echo \$? > $quoted_status"
status="$(adb shell "cat $quoted_status" | tr -d '\r')"

# The whole directory, so an aborted run leaves nothing behind either.
adb shell "rm -rf $quoted_directory" >&2 || true
exit "${status:-1}"
