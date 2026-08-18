#!/usr/bin/env bash
# Compile and lint this workspace's iOS-gated code on a machine that has
# no Apple SDK.
#
# **Why this exists, and why it copies rather than edits.** Code behind
# `cfg(target_os = "ios")` is compiled by exactly one CI leg and by
# nothing local, so a type error or a lint in it costs a five-minute
# round trip to find. Widening the gate for one run turns that into five
# seconds - and doing it by editing the tree in place went wrong three
# times in one afternoon: a blanket replace clobbered two unrelated
# `#[cfg(test)]` attributes, a second pass clobbered a new test module's,
# and a "surgical" version produced attributes that would not parse.
#
# Every one was caught, and none of that is an argument for being more
# careful. It is an argument for the repository being unreachable: this
# script exports the tracked tree into a scratch directory with
# `git archive`, rewrites the gates *there*, and runs cargo *there*. A
# mangled attribute cannot reach a file anybody will commit, because the
# only files it can touch are copies that get deleted.
#
# What this does NOT do is replace the CI leg. It compiles iOS-gated code
# for the *host*, which is a different target: it catches type errors,
# unresolved names and lints, and cannot catch anything that depends on
# the platform being iOS. `mobile-check` on a macOS runner is what does
# that, and it stays the gate.
#
# Usage: ios-local-check.sh [check|clippy|test]   (default: clippy)
set -euo pipefail

what="${1:-clippy}"
case "$what" in
    check | clippy | test) ;;
    *)
        echo "usage: ios-local-check.sh [check|clippy|test]" >&2
        exit 2
        ;;
esac

root="$(git rev-parse --show-toplevel)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

# **The working tree, not `HEAD`.** The first version exported the last
# commit, which is exactly the wrong thing: this tool exists to check
# edits *before* they are committed, and it would have compiled the code
# as it was before you touched it while reporting success. A planted type
# error went unnoticed, which is how that was found.
#
# Tracked files only, so the build directory and untracked scratch stay
# out, but their *current* contents.
git -C "$root" ls-files -z |
    tar -cf - -C "$root" --null --files-from - |
    tar -x -C "$scratch"
echo "copied the working tree to $scratch"

# **The rewrite, and its refusal to guess.** Only whole-line `cfg`
# attributes that name an iOS target are widened, and only where the
# line is exactly one of the forms this workspace uses - which are
# printed, so a form that is not in the list is visible rather than
# quietly skipped. `#[cfg(test)]` is never touched: that was the bug.
# Unlike the CI scripts, which run on runners that always have
# `python3`, this one runs wherever a developer is - and a Windows
# toolchain commonly provides `python` and no `python3` at all. Asking
# for the wrong one fails after the export, which reads as the export
# having gone wrong.
python="$(command -v python3 || command -v python || true)"
if [ -z "$python" ]; then
    echo "no python interpreter on PATH, so the gates cannot be rewritten" >&2
    exit 1
fi

"$python" - "$scratch" <<'REWRITE'
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
# **`test` would be the obvious switch and it is the wrong one.**
# `cfg(test)` is set only for the crate being compiled as a test, not
# for its dependencies - so widening with it exports `ios_main` from a
# library under test while configuring it out of the same library when a
# binary links it, and the binary fails to find a function that is right
# there. A named cfg passed through RUSTFLAGS applies to every crate in
# the graph uniformly, which is the property this needs.
# **Both halves, or the code stops compiling for a new reason.** A gate
# widened without its negation narrowed leaves two `main` functions
# defined at once - the iOS one now switched on, and the fallback that
# was never switched off. The negations are listed explicitly for the
# same reason the positives are: a form nobody wrote down is a form this
# script must not guess at.
FORMS = {
    '#[cfg(target_os = "ios")]':
        '#[cfg(any(ios_local_check, target_os = "ios"))]',
    '#[cfg(not(target_os = "ios"))]':
        '#[cfg(all(not(ios_local_check), not(target_os = "ios")))]',
    '#[cfg(all(target_os = "ios", feature = "window"))]':
        '#[cfg(any(ios_local_check, all(target_os = "ios", feature = "window")))]',
    '#[cfg(not(all(target_os = "ios", feature = "window")))]':
        '#[cfg(all(not(ios_local_check), not(all(target_os = "ios", feature = "window"))))]',
}

touched = 0
for path in sorted(root.rglob('*.rs')):
    text = path.read_text(encoding='utf-8')
    lines = text.split('\n')
    changed = False
    for index, line in enumerate(lines):
        stripped = line.strip()
        if stripped in FORMS:
            lines[index] = line.replace(stripped, FORMS[stripped])
            changed = True
    if changed:
        path.write_text('\n'.join(lines), encoding='utf-8')
        touched += 1
        print(f'  widened gates in {path.relative_to(root)}')

if touched == 0:
    raise SystemExit(
        'no iOS gates were found in any of the forms this script knows. Either the '
        'workspace has none, or it spells them a way this list does not cover - and '
        'silently checking nothing is the failure this message exists to prevent.'
    )
print(f'{touched} file(s) rewritten in the copy')
REWRITE

echo "running cargo $what against the copy"
cd "$scratch"

# The switch the rewritten gates name, plus its declaration, so the
# compiler does not also warn that it has never heard of it.
export RUSTFLAGS="--cfg ios_local_check --check-cfg=cfg(ios_local_check) ${RUSTFLAGS:-}"
case "$what" in
    check) cargo check --workspace --all-targets ;;
    clippy) cargo clippy --workspace --all-targets ;;
    test) cargo test --workspace ;;
esac

echo
echo "The repository was never modified: this ran entirely in $scratch,"
echo "which is now deleted. It compiled iOS-gated code for THIS host, so"
echo "it proves the code type-checks and lints - not that it behaves on"
echo "iOS. The macOS leg of mobile-check is what proves that."
