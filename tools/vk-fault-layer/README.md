# vk-fault-layer

A test-only Vulkan layer that makes a well-behaved driver misbehave on
cue. It is not part of the engine and never ships with it: it exists so
the rendering crate's error handling can be tested.

## Why this exists

A graphics backend spends a lot of code on things that go wrong — an
allocation that fails, a device that is lost mid-frame, a swapchain that
reports itself out of date, a queue submission that times out. On a
working driver none of that ever happens, which means none of it is ever
executed by a test. It is the part of a renderer most likely to be wrong
and least likely to be exercised.

There are two usual answers, and this is the cheaper one:

- **A mock driver** replaces the implementation entirely. Everything
  becomes testable and nothing stays real, so the tests drift from what
  the hardware does.
- **A layer** sits between the application and the real driver. By
  default it forwards every call untouched, so behaviour that is not
  being faulted is the driver's own. It is far less code, and — the part
  that actually matters — it does not have to be *correct about Vulkan*,
  only correct about passing calls along.

## What it does

By default: nothing. Every entry point resolves to the next layer or the
driver, so a run with the layer loaded and nothing armed behaves exactly
like a run without it. That property is worth stating because it is what
makes the layer safe to enable across a whole test suite.

Two environment variables arm it, and both are re-read at every
`vkCreateInstance`, so one test process can run many scenarios in
sequence.

**`RENEW_FAULT=<call>=<result>[@<ordinal>]`** fails one call. The
ordinal-th occurrence (1-based, default 1) of the named Vulkan call
returns the given result instead of reaching the driver.

```
RENEW_FAULT=vkCreateDevice=ERROR_OUT_OF_HOST_MEMORY
RENEW_FAULT=vkQueueSubmit=ERROR_DEVICE_LOST@3
```

**`RENEW_QUIRK=<name>[,<name>…]`** is the mirror image, and the more
interesting half. A quirk never invents a failure: it calls down, lets
the driver succeed, and then rewrites the *answer* into one this machine
would never give — no adapters, no swapchain extension, a queue that
cannot present, an "application chooses" surface extent. Those are all
legal Vulkan responses that some other hardware really does return, and
they reach engine paths a single machine can otherwise never test. The
names are listed in the source beside the code that applies them.

Unknown names and malformed values are ignored in silence. That is
deliberate: this code runs inside somebody else's process, loaded by the
Vulkan loader, and a test tool has no business aborting a host
application over a typo in an environment variable.

## Running it

The layer is a `cdylib` plus a JSON manifest telling the loader where to
find it. Build it, write the manifest beside the artifact, then point the
loader at the directory and name the layer:

```
cargo build -p vk-fault-layer
# write VK_LAYER_RENEW_fault.json next to the built library, with
# "library_path" pointing at it
export VK_ADD_LAYER_PATH="$PWD/target/fault-layer:$VK_ADD_LAYER_PATH"
export VK_LOADER_LAYERS_ENABLE=VK_LAYER_RENEW_fault
cargo test --package renew-rhi --test fault
```

`VK_ADD_LAYER_PATH` **adds** a directory rather than replacing the search
path. Overwriting it would silently drop the validation layer, and a run
that reports no validation errors because validation was not loaded is
worse than no run at all.

## What to know before changing it

Everything here is FFI and every function is `unsafe`. The layer
implements the loader's chain protocol by hand: `vkCreateInstance` and
`vkCreateDevice` walk the chain structures the loader passes in, take the
next link's `GetInstanceProcAddr`/`GetDeviceProcAddr`, advance the chain,
and call down. Every other call goes through the table built at that
point, so the steady-state cost is one lookup.

The layer is excluded from the coverage report. It is scaffolding that
runs inside the loader rather than engine code, and holding test tooling
to the engine's coverage bar would mean writing tests for the thing that
exists to make tests possible.
