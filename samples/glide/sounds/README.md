# Sounds

The `.wav` files here are generated, and their generator is committed
beside the game that plays them: `samples/glide/examples/make_sounds.rs`.
Source and bytes change together in one commit — one without the other
is a review reject, exactly as for the shader blobs.

That rule exists because a sound file with no producer is a magic
number. Wanting a louder flap, a fourth effect, or a re-encode after
the reader's accepted set moves should mean editing one formula and
running one command.

## Generation record

Written 2026-08-04 by the committed generator, on the repository's
pinned stable toolchain:

```
> cargo run -p renew-sample-glide --example make_sounds
samples/glide/sounds/flap.wav: 4012 bytes
samples/glide/sounds/score.wav: 7982 bytes
samples/glide/sounds/death.wav: 19888 bytes
```

All three are mono PCM16 at 22 050 Hz. The generator reads no clock and
draws no random numbers, so running it again writes byte-identical
files; the mixer resamples to whatever rate the device negotiates, so
the rate here is chosen for size rather than fidelity.

To regenerate: run the command above and commit the bytes with whatever
change to the generator produced them, updating the sizes recorded here.
