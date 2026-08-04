# WAV fixtures

Two small PCM files that **this repository did not encode**. They are read
by `tests/wav_anchor.rs`, and their provenance is the whole reason they
exist.

WAV is defined outside this tree. A reader and a writer built here from
the same reading of the same specification are inverses even when that
reading is wrong, so first-party artefacts alone cannot catch a field
this repository got backwards — they would all be backwards the same way.
These two files were encoded by somebody else's implementation, from
sample values chosen to pin both ends of the 16-bit range. Nobody here
decided what byte goes where in them.

That is also why they must never be re-encoded with the test writer in
`tests/wav_properties.rs`. Doing so would leave two files that look like
independent evidence and are not, which is worse than having none.

## Encoding record

Encoded 2026-08-04 by the `wave` module of the CPython 3.9 standard
library (verified reproducible on 3.9.13), 16-bit PCM, canonical 44-byte
header, no chunks beyond `fmt ` and `data`:

| file | bytes | channels | rate | samples |
|---|---|---|---|---|
| `mono_8000_stdlib.wav` | 60 | 1 | 8 000 Hz | `0, 1, -1, 256, -256, 32767, -32768, 12345` |
| `stereo_44100_stdlib.wav` | 68 | 2 | 44 100 Hz | `0, 1, 100, -100, 32767, -32768, -1, 2, 16384, -16384, 7, -7` |

The rates are the two that matter: the lowest the reader accepts, and the
one a device will actually negotiate.

Both files are output of a tool rather than copied source, so nothing
here is vendored — the recipe below is the artefact, and the bytes are
what it produced.

```python
import struct, wave

def write(path, channels, rate, values):
    with wave.open(path, 'wb') as out:
        out.setnchannels(channels)
        out.setsampwidth(2)
        out.setframerate(rate)
        out.writeframes(b''.join(struct.pack('<h', v) for v in values))

write('mono_8000_stdlib.wav', 1, 8000,
      [0, 1, -1, 256, -256, 32767, -32768, 12345])
write('stereo_44100_stdlib.wav', 2, 44100,
      [0, 1, 100, -100, 32767, -32768, -1, 2, 16384, -16384, 7, -7])
```

Re-running it writes byte-identical files: the encoder records no
timestamp and the sample values are literals. Changing either file means
changing the values above and the assertions in `tests/wav_anchor.rs`
together, in one commit.
