# The register trace

A lab instrument of the hardware H.264 plan
(`docs/baselines/work-plan-20261002-hardware-h264.md`, phase V2R; method and
record in `docs/design/h616-ve-h264-encoder.md`). It goes into no package,
slot or image, and `tools/test_packaging_boundaries.py` holds it out.

It answers one kind of question: what does a program that drives the video
engine write into the engine's registers, in which order, for which input.
The program under the trace gets no engine. Where it expects the register
window it gets memory it may not touch, and behind that memory a stand-in
answers the way the H616's engine was measured to answer.

## Parts

| File | What it is |
| --- | --- |
| `window.c` | The closed window. An access faults; the window is opened, that one instruction runs against the window's memory, the window is closed, and the access is recorded. No instruction is decoded or emulated. |
| `model.c` | The stand-in for the engine: which bits its registers hold, the bit writer's count, an encode that ends when its client waits. It encodes nothing. |
| `harness.c`, `log.c` | The log, the engine's keeper, and an arena of buffers with made-up addresses. |
| `selftest.c`, `selftest_client.c` | The trace's own proof: a client of this project behind the window, whose every access must come out complete and in order. `scripts/check.sh` runs it on the development host. |
| `ve_trace_report.py` | Reads a trace: calls, buffers, register accesses, the form in which addresses were written, the bytes of the bit writer, the registers' state at every start of an encode. |

## Building and running

```sh
# The trace's own test on this machine (also part of scripts/check.sh).
scripts/build-ve-trace.sh host /absolute/output

# For aarch64, with the compiler scripts/build-ve-module.sh keeps.
scripts/build-ve-trace.sh aarch64 WORK_DIR /absolute/output
```

On an aarch64 system, in a directory that holds what was built:

```sh
./ve-trace-selftest ./libselftest-client.so
```
