# Experimental Python calculator

The Python tool is disabled in default desktop builds. The opt-in build is for
testing only; it has not been validated across all supported operating systems
or for arbitrary long-running agent tasks. It does not establish absolute
security.

To build an experimental bundle from a checkout, first install the UI packages,
then build with both the feature and the resource overlay:

```sh
npm --prefix ui ci
cd crates/cordial-app
../../ui/node_modules/.bin/tauri build --features experimental-python --config tauri.python.conf.json
```

The overlay packages the pinned WASI CPython module and its license notices.
The runtime verifies the module's SHA-256 before compilation. Python programs
are supplied in memory, with no host filesystem or environment, no network,
no process creation, and no persistent script files. Memory, output and guest
instructions are bounded; blocking WASI polling is disabled. These limits and
isolation rely on the WASI runtime and operating system. Cancellation during
module compilation and non-Linux platform behavior require additional testing.

The experimental tool does not replace the read-only `run_command` allowlist.
It is visible to the model only when the runtime file is present. A missing or
integrity-failed module cannot run code. Tool calls and outputs remain in the
normal conversation transcript, which is not encrypted by Cordial.
