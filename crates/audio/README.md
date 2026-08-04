# renew-audio

Sound as arithmetic: a fixed-voice mixer that fills an interleaved
buffer, the command ring a game thread pushes into while an audio
thread drains it, and a WAV reader that believes nothing a file's
header claims.

Nothing here touches a device. The mixer fills a `&mut [f32]` and
stops; carrying that buffer to speakers belongs to the platform crate,
behind its own feature. That split is why this crate is testable,
fuzzable, and buildable on a machine with no sound hardware at all.

## Why it is a crate

Mixing and decoding are policy over arithmetic — how many voices, what
happens when they run out, which files are acceptable. Policy is
removable; a device seam is not. An engine build without sound drops
this crate and loses nothing else, which the build matrix proves by
building and testing without it.

## Contract

- **The callback path allocates nothing and cannot panic.** Decoding,
  resampling, and laying samples out for the device all happen in
  `Mixer::load`, before the stream starts. What runs on the audio
  thread copies, adds, and clamps, indexing only tables it owns. A
  gate measures the allocation half over windows it first proves are
  mixing.
- **The audio thread never waits.** Commands cross on a fixed-capacity
  ring. `Mixer::fill` *tries* for the lock; if the game thread holds
  it, the callback mixes what it already has and takes the commands
  next time. A few milliseconds of latency on an effect is inaudible.
  A missed buffer deadline is not.
- **Nothing a file claims is believed.** `wav::parse` checks every
  header field against the bytes actually present — declared sizes
  against the input's length, `block_align` and `byte_rate` against
  their derived values — and refuses by name. A malformed sound is a
  named error, never a wrong-shaped read.
- **No clocks.** A voice advances by the samples it was asked for, and
  voice-stealing order comes from a sequence number the mixer assigns.
  A slow frame changes when a sound is heard, never what is heard.
- **Capacity is refused by name.** Loading past the bank's size is a
  caller sizing bug and fails with a retained assertion saying so,
  rather than silently replacing a sound that some later frame plays.

## What is here

- `wav` — `parse` returns a borrowed view over validated bytes, plus an
  iterator decoding to `f32`. PCM16, mono or stereo, in the sample-rate
  range a game uses; everything else is refused by name.
- `mixer` — `MixerConfig` describes the device's buffers; `mixer()`
  returns the `MixerHandle` a game keeps and the `Mixer` an audio
  callback owns. `load` converts a sound once; `play` asks for one;
  `fill` mixes.

Voice stealing takes the oldest playing voice, which is audible as one
effect cut short rather than as an effect that never arrives.

## Testing

Unit tests pin the mixer's arms (silence, a sound ending mid-buffer,
saturation, stealing, resampling, mono into stereo) and the ring's
(order across many laps of its array, a full ring refusing, recovery
after a producer panics while holding the lock). The reader carries a
hand-asserted byte-layout anchor and third-party-encoded fixtures —
because a writer and a reader making the same mistake are still
inverses — beside property tests and a fuzz target with a committed
corpus. The allocation gate measures fill-and-play windows it first
proves are mixing; a stress test runs a flooding producer against a
draining callback, which is what the scheduled sanitizer runs judge.

## Manifest

Machine-readable fields — maturity, dependencies, core status — live in
`Cargo.toml` under `[package.metadata.renew]`, which is authoritative.
