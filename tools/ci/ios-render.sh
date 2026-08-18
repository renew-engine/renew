#!/usr/bin/env bash
# Draw a frame on an iOS simulator, with a real GPU translation layer
# underneath.
#
# **What this proves, and what it does not.** It draws *offscreen*: the
# sample is run with `--headless`, which renders into an image rather
# than a swapchain, so no `VkSurfaceKHR` is created and
# `ash_window::create_surface` is never called. **Presentation on iOS
# remains unexercised** - the surface extension is enabled and the
# swapchain device extension is enabled, and enabled is not exercised.
# A lane that puts a window on a simulator and presents into it is a
# different and later thing.
#
# What it does prove is that the engine's rendering path executes on
# this platform and produces a correct picture. Reaching that needed no
# iOS code: the workspace has type-checked for this target on every push
# since the first mobile lane, presentation included. What was missing
# was never source; it was a Vulkan implementation to load at runtime.
#
# MoltenVK is that implementation. `ash` opens the runtime by dlopening
# `libvulkan.dylib` on Apple targets, and MoltenVK's dynamic library
# exports the Vulkan entry points directly, so it is installed under
# that name and found through `DYLD_LIBRARY_PATH`. No Vulkan loader is
# involved, which also means no validation layers here - that is the
# desktop lane's job, and it does it on every push.
set -euo pipefail

# **The runtime comes from the tree, and is checked before it is used.**
# Committed rather than fetched so this lane and a developer machine
# link identical bytes and a clean clone builds offline - and verified
# here anyway, because a vendored binary that quietly changed would
# otherwise be the one thing nobody looks at.
#
# `ash` opens the runtime by dlopening exactly this name, so MoltenVK is
# installed under it.
slice="third_party/moltenvk/ios-arm64_x86_64-simulator/libMoltenVK.dylib"
digest="c6027cfbc343e9595cd8b072aab9f587cde4f8f0a52ad6159dd5794769592e1d"

echo "$digest  $slice" | shasum -a 256 -c - || {
    echo "the vendored MoltenVK does not match the digest recorded beside it in \
third_party/moltenvk/README.md, so this lane will not link something nobody chose" >&2
    exit 1
}

runtime="$PWD/target/moltenvk-runtime"
mkdir -p "$runtime"
cp "$slice" "$runtime/libvulkan.dylib"

selection="$(bash tools/ci/ios-simulator.sh)"
device="$(printf '%s' "$selection" | cut -f1)"
echo "simulator: $(printf '%s' "$selection" | cut -f2) on $(printf '%s' "$selection" | cut -f3)"

echo "booting $device"
xcrun simctl boot "$device" || true
xcrun simctl bootstatus "$device" -b

chmod +x tools/ios-sim-runner.sh
export RENEW_IOS_SIM_UDID="$device"
export CARGO_TARGET_AARCH64_APPLE_IOS_SIM_RUNNER="$PWD/tools/ios-sim-runner.sh"

# `simctl` passes variables prefixed this way through to the process it
# spawns, and strips the prefix on the way. It is the only channel a
# simulator process has to this shell's environment.
export SIMCTL_CHILD_DYLD_LIBRARY_PATH="$runtime"

# The sample treats an environment it cannot run in as a skip, not a
# failure - the right default, and a lie on a lane whose whole purpose
# is to run it. Strict mode turns a MoltenVK that will not load into the
# Vulkan error it is, instead of a clean exit and a missing file.
export RENEW_FRAME_STRICT=1
export SIMCTL_CHILD_RENEW_FRAME_STRICT=1

capture="$PWD/target/ios-triangle.png"
rm -f "$capture"

cargo run --package renew-sample-hello-triangle --bin hello_triangle \
    --target aarch64-apple-ios-sim -- --headless --frames 4 --capture "$capture"

# **A frame, or nothing.** The sample exits zero on a run that drew, so
# a missing or empty file means the run reported success without
# producing the one artifact this lane exists for.
if [ ! -s "$capture" ]; then
    echo "the run finished but wrote no image, so nothing here shows that a frame was \
drawn on this platform" >&2
    exit 1
fi

# **The picture is read, not weighed.** A file that exists and begins
# with a PNG signature proves the encoder ran; it does not distinguish a
# drawn triangle from a cleared square - which is the exact failure a
# translation layer produces when the render pass clears correctly and
# the translated shaders draw nothing. The rendering lane next door
# learned this the hard way and its comment says so: a weaker check
# "did exactly that once". The same three assertions are used here.
python3 - "$capture" <<'CHECK'
import struct
import sys
import zlib

path = sys.argv[1]
with open(path, 'rb') as handle:
    data = handle.read()

if data[:8] != b'\x89PNG\r\n\x1a\n':
    raise SystemExit(f'the captured file is not a PNG: first bytes were {data[:8]!r}')

width, height = struct.unpack('>II', data[16:24])
if (width, height) != (64, 64):
    raise SystemExit(f'unexpected size {width}x{height}')

at, idat = 8, b''
while at < len(data):
    length = struct.unpack('>I', data[at:at + 4])[0]
    if data[at + 4:at + 8] == b'IDAT':
        idat += data[at + 8:at + 8 + length]
    at += 12 + length
raw = zlib.decompress(idat)
if len(raw) != height * (1 + width * 4):
    raise SystemExit('the pixel stream is the wrong length')

stride = 1 + width * 4
colours = set()
for row in range(height):
    line = raw[row * stride + 1:(row + 1) * stride]
    for x in range(0, width * 4, 4):
        colours.add(bytes(line[x:x + 4]))

if len(colours) < 4:
    raise SystemExit(f'only {len(colours)} colour(s); the geometry did not draw')

corner = raw[1:5]
centre = raw[(height // 2) * stride + 1 + (width // 2) * 4:][:4]
if centre == corner:
    raise SystemExit('the middle of the picture is the backdrop; nothing drew there')

print(f'{len(data)} bytes, {width}x{height}, {len(colours)} colours')
print('drew a frame on the simulator through MoltenVK')
CHECK
