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
| `vendor_run.c`, `vendor_ops.c`, `vendor_ion.c`, `companion_stub.c` | The vendor's encoder library as a black box: it is loaded, called through its published interface to encode a few pictures, and unloaded. |
| `ve_trace_report.py` | Reads a trace: calls, buffers, register accesses, the form in which addresses were written, the bytes of the bit writer, the registers' state at every start of an encode. |

## The clean room

- The vendor's library is never in this repository. It is fetched into a
  cache outside it and named on the command line of a run.
- Its interface headers are read at compile time, from the same cache, for
  the types of the interface. Nothing of them is copied here.
- The library is not disassembled, decompiled or read. The trace does not
  look at its instructions either: on AArch64 it puts a breakpoint behind
  the instruction that faulted, keeping and restoring the word the
  breakpoint replaces without looking at it.
- What is kept from a run is the trace: offsets, values, the calls across
  the interface. The register description is written from it in this
  project's own words.
- The library may not open files (`fopen` is answered by the trace program
  and refused) and gets no device: it imports nothing else that reaches
  outside its process.

## Building and running

```sh
# The trace's own test on this machine (also part of scripts/check.sh).
scripts/build-ve-trace.sh host /absolute/output

# For aarch64, with the compiler scripts/build-ve-module.sh keeps; the
# fourth argument is the vendor library's interface directory in the cache.
scripts/build-ve-trace.sh aarch64 WORK_DIR /absolute/output [INTERFACE_DIR]
```

The vendor's library runs on the test board only, by the operator's
decision: in user space, as the unprivileged user `nobody`, in a session's
directory on tmpfs that is removed afterwards. No module is loaded and the
engine is not touched. From the repository root, with the board's root
password on stdin through `project-memory-exec`:

```sh
tools/device/ve_trace_session.sh OUTPUT_DIR TRACE_DIR /path/to/the/library \
    'first width=320 height=240 frames=3' 'moved base=0x53000000'
```

Every session begins with the trace's own test. Each further argument is
one run: a name for its results and settings of `ve-trace-vendor`, which
lists them when it is started without arguments: the picture, the rate
control, and what our side of the interface tells the library about the
engine (its identification, the offset taken off addresses, where the
made-up addresses begin). One setting is changed per run, and
`ve_trace_report.py --compare` shows what that changed in the registers.
