#!/usr/bin/env bash
# Present a frame on an iOS simulator's screen, and look at the screen.
#
# **The offscreen lane cannot answer this question.** Running headless
# draws into an image: no `VkSurfaceKHR` is created, no swapchain is
# built, nothing is acquired or presented. Everything in the RHI between
# a window handle and a presented frame is compiled for this target and
# exercised by nothing. This lane is what exercises it.
#
# So the evidence has to come from the screen rather than from a file
# the process wrote. `simctl io screenshot` captures what the simulator
# is displaying, which is the only place a presented frame exists.
set -euo pipefail

bundle_id="com.renewengine.hellotriangle"
binary="hello_triangle"

# The runtime comes from the tree and is checked before it is used, the
# same way the offscreen lane does it. Inside an app bundle it goes to
# `Frameworks/`, which is where dyld looks with an `@rpath` and where a
# real application would ship it.
slice="third_party/moltenvk/ios-arm64_x86_64-simulator/libMoltenVK.dylib"
digest="c6027cfbc343e9595cd8b072aab9f587cde4f8f0a52ad6159dd5794769592e1d"

echo "$digest  $slice" | shasum -a 256 -c - || {
    echo "the vendored MoltenVK does not match the digest recorded beside it in \
third_party/moltenvk/README.md, so this lane will not link something nobody chose" >&2
    exit 1
}

selection="$(bash tools/ci/ios-simulator.sh)"
device="$(printf '%s' "$selection" | cut -f1)"
echo "simulator: $(printf '%s' "$selection" | cut -f2) on $(printf '%s' "$selection" | cut -f3)"

echo "booting $device"
xcrun simctl boot "$device" || true
xcrun simctl bootstatus "$device" -b

app="$(bash tools/ios-app-bundle.sh renew-sample-hello-triangle "$binary" "$bundle_id" \
    target/ios-present)"
echo "bundled $app"

xcrun simctl uninstall "$device" "$bundle_id" >/dev/null 2>&1 || true
xcrun simctl install "$device" "$app"

# `simctl` strips this prefix and passes the rest to the process it
# launches. The library is named what `ash` dlopens.
runtime="$PWD/target/ios-present-runtime"
mkdir -p "$runtime"
cp "$slice" "$runtime/libvulkan.dylib"
export SIMCTL_CHILD_DYLD_LIBRARY_PATH="$runtime"
export SIMCTL_CHILD_RENEW_FRAME_STRICT=1

xcrun simctl launch "$device" "$bundle_id" >/dev/null

# Long enough for bring-up and a few frames. A swapchain on a translation
# layer on a simulated GPU is not instant, and a screenshot taken during
# bring-up would show the launch screen and be read as a failure to
# present.
sleep 12

shot="$PWD/target/ios-present.png"
rm -f "$shot"
xcrun simctl io "$device" screenshot "$shot"

container="$(xcrun simctl get_app_container "$device" "$bundle_id" data)"
log="$container/Documents/hello_triangle.log"
if [ -f "$log" ]; then
    echo "--- the app's own log ---"
    cat "$log"
else
    echo "--- the app wrote no log ---"
fi

# **Did it stay up, and is this its screen?** An app that drew one frame
# and died leaves a screenshot of the home screen - which is full of
# colour and would sail through every test below. Liveness is therefore
# load-bearing here, not a formality.
#
# `launchctl list` prints pid, status and label. A job that has exited
# keeps its label and shows `-` where the pid was, so matching the bundle
# id anywhere on the line says only that iOS knows the app exists. The
# pid column is what says it is running, so that is what is read.
if ! xcrun simctl spawn "$device" launchctl list 2>/dev/null |
    awk -v id="$bundle_id" '$1 ~ /^[0-9]+$/ && index($3, id) { found = 1 } END { exit !found }'
then
    echo "the app has no running process, so the screen is showing something else - this \
is a failure to stay up rather than a failure to present" >&2
    exit 1
fi

if [ ! -s "$shot" ]; then
    echo "no screenshot was captured, so nothing here shows what reached the screen" >&2
    exit 1
fi

# **The screen is read, not weighed.** A screenshot always exists and
# always decodes; what distinguishes a presented frame from a blank
# launch screen is what is in the middle of it. The sample fills its
# window with a triangle over a backdrop, so the centre must differ from
# the corner and the picture must carry more than a couple of colours -
# the same shape the offscreen lanes assert, applied to a screen.
python3 - "$shot" <<'CHECK'
import struct
import sys
import zlib

with open(sys.argv[1], 'rb') as handle:
    data = handle.read()

if data[:8] != b'\x89PNG\r\n\x1a\n':
    raise SystemExit(f'the screenshot is not a PNG: first bytes were {data[:8]!r}')

width, height = struct.unpack('>II', data[16:24])
depth, colour = data[24], data[25]
if depth != 8 or colour not in (2, 6):
    raise SystemExit(f'unexpected screenshot format: depth {depth}, colour type {colour}')
channels = 3 if colour == 2 else 4

at, idat = 8, b''
while at < len(data):
    length = struct.unpack('>I', data[at:at + 4])[0]
    if data[at + 4:at + 8] == b'IDAT':
        idat += data[at + 8:at + 8 + length]
    at += 12 + length
raw = zlib.decompress(idat)

# Screenshots are filtered per scanline, so the rows are undone rather
# than read flat - unlike the sample's own captures, which it writes
# unfiltered.
stride = width * channels
out = bytearray()
previous = bytearray(stride)
at = 0
for _ in range(height):
    filter_type = raw[at]
    line = bytearray(raw[at + 1:at + 1 + stride])
    at += 1 + stride
    for i in range(stride):
        left = line[i - channels] if i >= channels else 0
        up = previous[i]
        upper_left = previous[i - channels] if i >= channels else 0
        if filter_type == 1:
            line[i] = (line[i] + left) & 0xFF
        elif filter_type == 2:
            line[i] = (line[i] + up) & 0xFF
        elif filter_type == 3:
            line[i] = (line[i] + (left + up) // 2) & 0xFF
        elif filter_type == 4:
            p = left + up - upper_left
            pa, pb, pc = abs(p - left), abs(p - up), abs(p - upper_left)
            nearest = left if (pa <= pb and pa <= pc) else (up if pb <= pc else upper_left)
            line[i] = (line[i] + nearest) & 0xFF
        elif filter_type != 0:
            raise SystemExit(f'unknown PNG filter {filter_type}')
    out += line
    previous = line

def pixel(x, y):
    start = y * stride + x * channels
    return bytes(out[start:start + 3])

# **Find the app's window, then look strictly inside it.**
#
# The window does not fill the screen: the sample asks for a landscape
# window on a portrait phone, so the app occupies a band at the top and
# the rest of the display is black. Sampling the *screen's* centre reads
# empty display below the app, which is how the first version of this
# check passed while comparing nothing against nothing.
black = b'\x00\x00\x00'
rows = [y for y in range(0, height, 4)
        if any(pixel(x, y) != black for x in range(0, width, 4))]
if not rows:
    raise SystemExit('the whole screen is black; the app presented nothing')

top, bottom = rows[0], rows[-1]
columns = [x for x in range(0, width, 4)
           if any(pixel(x, y) != black for y in range(top, bottom + 1, 4))]
left, right = columns[0], columns[-1]
band_w, band_h = right - left, bottom - top
print(f'{width}x{height} screen; the app occupies '
      f'{band_w}x{band_h} at ({left},{top})')

if band_w < width // 2 or band_h < height // 8:
    raise SystemExit(f'the non-black region is {band_w}x{band_h}, too small to be the window')

# **The band includes the operating system's own chrome, and the chrome
# is enough to pass a coverage test on its own.** A status bar carries a
# clock, a dynamic island, signal and battery glyphs - dozens of colours
# and several percent of the band's pixels - all of it drawn by iOS and
# none of it by this engine. Measured against a screenshot whose window
# content was flattened to its clear colour, chrome alone reached 4.6%
# against a 5% floor: a hair, and on the wrong side of the argument.
#
# So the region is inset before anything is counted. What remains is
# window interior, where a cleared frame really is one flat colour.
inset_y = max(1, band_h // 5)
inset_x = max(1, band_w // 20)
top += inset_y
bottom -= inset_y
left += inset_x
right -= inset_x
if bottom <= top or right <= left:
    raise SystemExit('the window is too small to inspect once its chrome is excluded')
print(f'measuring the interior {right - left}x{bottom - top} at ({left},{top})')

sampled = [pixel(x, y)
           for y in range(top, bottom + 1, 5)
           for x in range(left, right + 1, 5)]
tally = {}
for colour in sampled:
    tally[colour] = tally.get(colour, 0) + 1
backdrop, most = max(tally.items(), key=lambda item: item[1])
drawn = len(sampled) - most
share = 100.0 * drawn / len(sampled)

print(f'{len(tally)} colours in the interior; backdrop {backdrop.hex()} covers '
      f'{100.0 * most / len(sampled):.1f}%, geometry {share:.1f}%')

# **Coverage, not position.** A frame that drew nothing is one flat
# colour end to end. Where the geometry lands is deliberately not
# asserted: the swapchain is built at a different extent than the window
# reports, so the picture sits against an edge rather than centred, and a
# centre-versus-corner check would call a working renderer broken. That
# discrepancy is recorded as debt rather than explained here.
if len(tally) < 3:
    raise SystemExit(f'the interior shows {len(tally)} colour(s); nothing was drawn in it')
if share < 5.0:
    raise SystemExit(
        f'only {share:.1f}% of the window interior differs from its backdrop; that is a '
        f'cleared frame rather than a drawn one'
    )
print('a frame reached the screen')
CHECK
