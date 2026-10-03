#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""A hostile client of the encoder driver, on the board
(docs/design/h616-ve-encoder-driver-v1.md, "Tests").

usage: python3 ve_enc_hostile.py [OUTPUT_JSON]

Every request out of range, out of order, twice, with memory that is not
the client's, pictures of noise at the finest quantiser, a device closed in
the middle and a client killed in the middle; then random requests, and a
thousand streams of the largest size one after the other. After each group the driver must still encode a key
frame. The kernel's log must have no new line but the test's own notes that
it dropped the page cache. Of the board's contiguous
memory nothing may be left taken: the kernel does not say who holds it and
lends what is free to its page cache, so the page cache is dropped before
each count, and the measure is that after the thousand streams no more
than 16 MiB less is free than before the device was first opened; the
smallest buffer of such a stream, left behind every time, would be 20 MiB.

The driver's module must be loaded. The answer is one JSON document: what
was tried, and every answer that was not the one the interface promises.
"""

import ctypes
import errno
import fcntl
import json
import mmap
import os
import random
import signal
import struct
import subprocess
import sys
import time

DEVICE = "/dev/hardened-blikvm-ve-enc"
IOC_INFO = 0x80205620
IOC_CONFIGURE = 0xC0185621
IOC_ENCODE = 0xC0485622
KEY, AGAIN, WIDE, EARLIER, STAY = 1, 2, 4, 8, 16
UNIT_BYTES = 132
FRAME = struct.Struct("<IIQQQIIIIII3II")
CONFIG = struct.Struct("<IIIIII")
INFO = struct.Struct("<IIIIIIII")
assert FRAME.size == 72 and CONFIG.size == 24 and INFO.size == 32
# The interface's limits (ve_enc_abi.h).
MIN_WIDTH, MIN_HEIGHT, MAX_WIDTH, MAX_HEIGHT = 256, 64, 1920, 1088
# The engine's status when a frame did not fit its stream buffer.
STALL = 2

# Streams opened and closed at the end, and how much less contiguous memory
# may be free after them: less than their smallest buffer left behind every
# time (20 KiB), more than the page cache borrows in the test's two minutes.
STREAMS = 1000
SLACK_KB = 16 * 1024

unexpected = []
tried = 0


def expect(what, wanted, call):
    """Runs `call`; `wanted` is an errno, 0 for success, or a tuple of them."""
    global tried
    tried += 1
    wanted = wanted if isinstance(wanted, tuple) else (wanted,)
    try:
        result = call()
        got = 0
    except OSError as error:
        result = None
        got = error.errno
    if got not in wanted:
        unexpected.append({
            "what": what,
            "wanted": [errno.errorcode.get(code, code) for code in wanted],
            "got": errno.errorcode.get(got, got),
        })
    return result


def cma_free():
    # The kernel lends free contiguous memory to its page cache: what it
    # can give back at once is given back before the count, so that two
    # counts can be compared.
    os.sync()
    try:
        with open("/proc/sys/vm/drop_caches", "w", encoding="ascii") as drop:
            drop.write("1\n")
    except OSError:
        pass
    with open("/proc/meminfo", encoding="ascii") as meminfo:
        for line in meminfo:
            if line.startswith("CmaFree:"):
                return int(line.split()[1])
    return None


def kernel_log():
    return subprocess.run(["dmesg"], capture_output=True, text=True, check=False).stdout.splitlines()


class Client:
    """One open device with buffers of this process to encode with."""

    def __init__(self):
        self.fd = os.open(DEVICE, os.O_RDWR)
        self.picture = None
        self.stream = None
        self.units = ctypes.create_string_buffer(3 * UNIT_BYTES)
        # An SPS-like unit of 9 bytes and a slice header of 30 bits.
        struct.pack_into("<BBH", self.units, 0, 0x67, 0, 72)
        self.units[4:13] = bytes(range(0x40, 0x49))
        struct.pack_into("<BBH", self.units, UNIT_BYTES, 0x65, 0, 30)
        self.units[UNIT_BYTES + 4:UNIT_BYTES + 8] = b"\x88\x80\x41\x1c"

    def close(self):
        if self.fd >= 0:
            os.close(self.fd)
            self.fd = -1

    def configure(self, width, height, flags=0, reserved=0):
        buffer = bytearray(CONFIG.pack(width, height, flags, reserved, 0, 0))
        fcntl.ioctl(self.fd, IOC_CONFIGURE, buffer)
        _, _, _, _, picture_bytes, stream_bytes = CONFIG.unpack(buffer)
        self.picture = ctypes.create_string_buffer(picture_bytes)
        ctypes.memset(self.picture, 0x80, picture_bytes)
        self.stream = ctypes.create_string_buffer(stream_bytes)
        return picture_bytes, stream_bytes

    def frame(self, flags=KEY, qp=30, picture=None, units=None, stream=None, picture_bytes=None,
              unit_count=2, capacity=None, reserved=0):
        """One encode request; anything not given is what a good client gives."""
        fields = [
            flags,
            qp,
            ctypes.addressof(self.picture) if picture is None else picture,
            ctypes.addressof(self.units) if units is None else units,
            ctypes.addressof(self.stream) if stream is None else stream,
            len(self.picture) if picture_bytes is None else picture_bytes,
            unit_count,
            len(self.stream) if capacity is None else capacity,
            0, 0, 0, 0, 0, 0,
            reserved,
        ]
        buffer = bytearray(FRAME.pack(*fields))
        try:
            fcntl.ioctl(self.fd, IOC_ENCODE, buffer)
        finally:
            self.answer = FRAME.unpack(buffer)
        return self.answer[8]

    def works(self, what):
        """A key frame and a frame after it: the driver is still whole."""
        size = expect(f"{what}: a key frame afterwards", 0, self.frame)
        expect(f"{what}: a frame after the key frame", 0, lambda: self.frame(flags=0, unit_count=1,
               units=ctypes.addressof(self.units) + UNIT_BYTES))
        return size


def slice_only_units(client):
    """The arguments of a frame that is no key frame: its slice header alone."""
    return {"unit_count": 1, "units": ctypes.addressof(client.units) + UNIT_BYTES}


LIBC = ctypes.CDLL(None, use_errno=True)
LIBC.mmap.restype = ctypes.c_void_p
LIBC.mmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_long]
LIBC.munmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
MAP_PRIVATE_ANONYMOUS = 0x02 | 0x20
# fcntl.ioctl takes an argument that is a number only as a C int: an
# address of this process does not fit, so such requests go through libc.
LIBC.ioctl.argtypes = [ctypes.c_int, ctypes.c_ulong, ctypes.c_void_p]
LIBC.ioctl.restype = ctypes.c_int


def ioctl_at(fd, request, address):
    """A request whose argument is `address` as it is."""
    if LIBC.ioctl(fd, request, address) < 0:
        code = ctypes.get_errno()
        raise OSError(code, os.strerror(code))
    return 0


def unmapped():
    """An address of this process that has no memory behind it."""
    address = LIBC.mmap(None, 1 << 20, mmap.PROT_READ, MAP_PRIVATE_ANONYMOUS, -1, 0)
    LIBC.munmap(address, 1 << 20)
    return address


def read_only(size):
    """Memory of this process that cannot be written."""
    return LIBC.mmap(None, size, mmap.PROT_READ, MAP_PRIVATE_ANONYMOUS, -1, 0)


def groups(report):
    hole = unmapped()

    # Nothing but one client.
    first = Client()
    answer = bytearray(INFO.size)
    expect("info", 0, lambda: fcntl.ioctl(first.fd, IOC_INFO, answer))
    abi, decoder_ip, encoder_ip, max_width, max_height, min_width, min_height, _ = INFO.unpack(answer)
    report["info"] = {"abi": abi, "decoder_ip": f"{decoder_ip:#x}", "encoder_ip": f"{encoder_ip:#x}"}
    if (abi, max_width, max_height, min_width, min_height) != (1, MAX_WIDTH, MAX_HEIGHT, MIN_WIDTH, MIN_HEIGHT):
        unexpected.append({"what": "the interface's limits", "got": [abi, max_width, max_height, min_width, min_height]})
    expect("a second open", errno.EBUSY, lambda: os.open(DEVICE, os.O_RDWR))
    expect("an unknown request", errno.ENOTTY, lambda: fcntl.ioctl(first.fd, 0xC0485699, bytearray(72)))
    expect("the lab module's request", errno.ENOTTY, lambda: fcntl.ioctl(first.fd, 0x5613))
    expect("info into no memory", errno.EFAULT, lambda: ioctl_at(first.fd, IOC_INFO, hole))
    expect("configure from no memory", errno.EFAULT, lambda: ioctl_at(first.fd, IOC_CONFIGURE, hole))
    expect("encode from no memory", (errno.EFAULT, errno.EINVAL), lambda: ioctl_at(first.fd, IOC_ENCODE, hole))
    expect("info into the kernel", errno.EFAULT, lambda: ioctl_at(first.fd, IOC_INFO, 0xFFFF800008000000))
    expect("a mapping", (errno.ENODEV, errno.EINVAL, errno.EACCES), lambda: mmap.mmap(first.fd, 4096))
    expect("a read", (errno.EINVAL, errno.EBADF, errno.ENOSYS), lambda: os.read(first.fd, 16))
    expect("a write", (errno.EINVAL, errno.EBADF, errno.ENOSYS), lambda: os.write(first.fd, b"x" * 16))

    # Out of order: an encode before any configure.
    first.picture = ctypes.create_string_buffer(64)
    first.stream = ctypes.create_string_buffer(64)
    expect("encode before configure", errno.EINVAL, first.frame)

    # Sizes and flags the interface does not have.
    for width, height, flags, reserved in [
        (0, 0, 0, 0), (254, 64, 0, 0), (256, 62, 0, 0), (257, 64, 0, 0), (256, 65, 0, 0),
        # Narrower than the engine finishes a frame that refers to another.
        (64, 64, 0, 0), (192, 192, 0, 0), (64, 1088, 0, 0), (240, 240, 0, 0),
        (1922, 1080, 0, 0), (1920, 1090, 0, 0), (4096, 2160, 0, 0), (0xFFFFFFFF, 64, 0, 0),
        (256, 0xFFFFFFFF, 0, 0), (0x80000100, 0x80000040, 0, 0), (320, 240, 2, 0),
        (320, 240, 0xFFFFFFFF, 0), (320, 240, 0, 1),
    ]:
        expect(f"configure {width}x{height} flags {flags:#x} reserved {reserved}", errno.EINVAL,
               lambda: first.configure(width, height, flags, reserved))
    expect("encode after refused configures", errno.EINVAL, first.frame)

    picture_bytes, stream_bytes = expect("configure 320x240", 0, lambda: first.configure(320, 240))
    report["picture_bytes"], report["stream_bytes"] = picture_bytes, stream_bytes

    # Frames a stream cannot begin with, and requests out of range.
    expect("a first frame that is no key frame", errno.EINVAL, lambda: first.frame(flags=0))
    expect("a first frame again", errno.EINVAL, lambda: first.frame(flags=KEY | AGAIN))
    for name, arguments in [
        ("QP 52", {"qp": 52}), ("QP 0xffffffff", {"qp": 0xFFFFFFFF}),
        ("unknown flags", {"flags": 0xFFFFFFFF}), ("flag 32", {"flags": KEY | 32}),
        ("a first frame that refers to an earlier pass", {"flags": KEY | EARLIER}),
        ("two references at once", {"flags": KEY | EARLIER | STAY}),
        ("no unit", {"unit_count": 0}), ("four units", {"unit_count": 4}),
        ("0xffffffff units", {"unit_count": 0xFFFFFFFF}),
        ("a picture one byte short", {"picture_bytes": picture_bytes - 1}),
        ("a picture one byte long", {"picture_bytes": picture_bytes + 1}),
        ("a picture of no bytes", {"picture_bytes": 0}),
        ("a picture of 0xffffffff bytes", {"picture_bytes": 0xFFFFFFFF}),
        ("a reserved field", {"reserved": 1}),
    ]:
        expect(name, errno.EINVAL, lambda: first.frame(**arguments))
    long_unit = ctypes.create_string_buffer(3 * UNIT_BYTES)
    struct.pack_into("<BBH", long_unit, 0, 0x67, 0, 8 * 128 + 1)
    expect("a unit of 1025 bits", errno.EINVAL,
           lambda: first.frame(units=ctypes.addressof(long_unit), unit_count=1))
    struct.pack_into("<BBH", long_unit, 0, 0x67, 0, 0xFFFF)
    expect("a unit of 65535 bits", errno.EINVAL,
           lambda: first.frame(units=ctypes.addressof(long_unit), unit_count=1))

    # Memory that is not the client's.
    for name, arguments in [
        ("a picture at no memory", {"picture": hole}), ("a picture at address 0", {"picture": 0}),
        ("a picture in the kernel", {"picture": 0xFFFF800008000000}),
        ("units at no memory", {"units": hole}), ("units at address 0", {"units": 0}),
        ("units in the kernel", {"units": 0xFFFF800008000000}),
    ]:
        expect(name, errno.EFAULT, lambda: first.frame(**arguments))
    first.works("after the refused requests")
    for name, arguments in [
        ("a stream to no memory", {"stream": hole}), ("a stream to address 0", {"stream": 0}),
        ("a stream into the kernel", {"stream": 0xFFFF800008000000}),
    ]:
        expect(name, errno.EFAULT, lambda: first.frame(**arguments))
    locked = read_only(stream_bytes)
    expect("a stream to memory that cannot be written", errno.EFAULT,
           lambda: first.frame(stream=locked))
    size = first.works("after the streams that could not be written")
    expect("a stream of no room", errno.ENOSPC, lambda: first.frame(capacity=0))
    needed = first.answer[8]
    if needed != size:
        unexpected.append({"what": "ENOSPC tells the frame's size", "wanted": size, "got": needed})
    expect("a stream of one byte less", errno.ENOSPC, lambda: first.frame(capacity=needed - 1))
    expect("the same picture again after ENOSPC", 0, lambda: first.frame(flags=KEY | AGAIN))

    # Passes and what they refer to, in and out of order.
    slice_only = {"unit_count": 1, "units": ctypes.addressof(first.units) + UNIT_BYTES}
    first.works("before the passes")
    for name, flags, wanted in [
        ("an earlier pass where there was one pass", EARLIER, errno.EINVAL),
        ("a second pass with the other search", AGAIN | WIDE, 0),
        ("a second pass that names an earlier one", AGAIN | EARLIER, errno.EINVAL),
        ("a second pass that stays", AGAIN | STAY, errno.EINVAL),
        ("a third pass", AGAIN, 0),
        ("a picture that refers to the pass before the last", EARLIER, 0),
        ("an earlier pass twice", EARLIER, errno.EINVAL),
        ("a picture that both stays and names an earlier pass", EARLIER | STAY, errno.EINVAL),
        ("a picture that stays where the stream was", STAY | WIDE, 0),
        ("a picture after it", 0, 0),
        ("a second pass after it", AGAIN, 0),
        ("a key frame that stays where the stream was", KEY | STAY, 0),
    ]:
        arguments = {} if flags & KEY else slice_only
        expect(name, wanted, lambda: first.frame(flags=flags, **arguments))
    # A picture that cannot be read ends the stream: a key frame is needed.
    expect("a picture at no memory, in a stream", errno.EFAULT,
           lambda: first.frame(flags=0, picture=hole, **slice_only))
    for flags in (0, STAY, EARLIER, AGAIN):
        expect(f"flags {flags:#x} after a picture that could not be read", errno.EINVAL,
               lambda: first.frame(flags=flags, **slice_only))
    first.works("after the passes")

    # Twice, and in every order.
    expect("the same frame again", 0, lambda: first.frame(flags=KEY | AGAIN, qp=40))
    expect("again, as a frame that is no key frame, with one frame behind it", (0, errno.EINVAL),
           lambda: first.frame(flags=AGAIN, unit_count=1, units=ctypes.addressof(first.units) + UNIT_BYTES))
    for _ in range(50):
        expect("configure again", 0, lambda: first.configure(320, 240))
        expect("a frame after configuring again that is no key frame", errno.EINVAL,
               lambda: first.frame(flags=0))
    first.works("after configuring fifty times")
    for width, height in [(256, 64), (1920, 1088), (1366, 768), (1920, 1080), (258, 1088), (1920, 64)]:
        expect(f"configure {width}x{height}", 0, lambda: first.configure(width, height))
        first.works(f"{width}x{height}")
    report["largest_frame"] = first.answer[8]

    # Pictures of noise at the finest quantiser: the most a frame can cost,
    # and more than some stream buffers hold. The engine then stops at the
    # buffer's end and says so; the request fails with EIO and that status,
    # and the driver goes on with the next key frame.
    noise = {}
    for width, height in [(1920, 1088), (1366, 768), (1024, 768), (256, 64)]:
        first.configure(width, height)
        size = len(first.picture)
        bytes_of = random.Random(width * height)
        outcome = []
        for flags, units in ((KEY, {}), (0, slice_only_units(first))):
            ctypes.memmove(first.picture, bytes_of.randbytes(size), size)
            try:
                outcome.append(first.frame(flags=flags, qp=0, **units))
            except OSError as error:
                status = first.answer[9]
                outcome.append(f"{errno.errorcode.get(error.errno)} status {status:#x}")
                if error.errno != errno.EIO or status != STALL:
                    # A frame after a failed key frame has nothing to refer to.
                    if not (flags == 0 and error.errno == errno.EINVAL):
                        unexpected.append({"what": f"noise at QP 0, {width}x{height}", "got": outcome[-1]})
        noise[f"{width}x{height}"] = {"stream_bytes": len(first.stream), "frames": outcome}
        ctypes.memset(first.picture, 0x80, size)
        first.works(f"after noise at {width}x{height}")
    report["noise_at_qp_0"] = noise

    # Signals while the engine encodes: every frame is finished all the same.
    signals = 0

    def count(*_):
        nonlocal signals
        signals += 1

    signal.signal(signal.SIGALRM, count)
    first.configure(1920, 1080)
    first.frame()
    signal.setitimer(signal.ITIMER_REAL, 0.004, 0.0037)
    for _ in range(200):
        expect("an encode under signals", 0, lambda: first.frame(flags=KEY))
    signal.setitimer(signal.ITIMER_REAL, 0)
    report["encodes_under_signals"] = 200
    report["signals"] = signals
    first.works("after the signals")

    # Closed in the middle, and killed in the middle.
    first.close()
    for round_ in range(20):
        child = os.fork()
        if child == 0:
            try:
                client = Client()
                client.configure(1920, 1080)
                while True:
                    client.frame()
            finally:
                os._exit(0)
        time.sleep(0.05 + 0.01 * round_)
        os.kill(child, signal.SIGKILL)
        os.waitpid(child, 0)
    report["killed_in_the_middle"] = 20
    second = expect("an open after the killed clients", 0, Client)
    if second is None:
        return
    second.configure(320, 240)
    second.works("after the killed clients")

    # Random requests.
    generator = random.Random(20261002)
    counts = {}
    for _ in range(20000):
        choice = generator.randrange(10)
        try:
            if choice == 0:
                second.configure(generator.choice([64, 256, 320, 1280, 1920, generator.randrange(1 << 32)]) & ~generator.randrange(2),
                                 generator.choice([64, 240, 720, 1088, generator.randrange(1 << 32)]),
                                 generator.choice([0, 0, 1, generator.randrange(1 << 32)]))
            elif choice == 1:
                fcntl.ioctl(second.fd, generator.randrange(1 << 32), bytearray(os.urandom(72)))
            elif choice == 2:
                fcntl.ioctl(second.fd, IOC_ENCODE, bytearray(os.urandom(72)))
            else:
                second.frame(
                    flags=generator.choice([KEY, 0, 0, AGAIN, KEY | AGAIN, WIDE, AGAIN | WIDE, EARLIER, STAY,
                                            generator.randrange(64), generator.randrange(1 << 32)]),
                    qp=generator.choice([generator.randrange(52), generator.randrange(1 << 32)]),
                    unit_count=generator.choice([1, 2, 3, generator.randrange(1 << 32)]),
                    picture_bytes=generator.choice([None, None, None, generator.randrange(1 << 32)]),
                    capacity=generator.choice([None, None, generator.randrange(1 << 22)]),
                    picture=generator.choice([None, None, None, hole, generator.randrange(1 << 48)]),
                    stream=generator.choice([None, None, None, hole, generator.randrange(1 << 48)]),
                    units=generator.choice([None, None, None, hole, generator.randrange(1 << 48)]),
                    reserved=generator.choice([0, 0, 0, generator.randrange(1 << 32)]))
            code = 0
        except OSError as error:
            code = error.errno
        except (TypeError, ValueError):
            code = "client"
        name = errno.errorcode.get(code, str(code)) if code else "ok"
        counts[name] = counts.get(name, 0) + 1
    report["random_requests"] = counts
    allowed = {"ok", "EINVAL", "EFAULT", "ENOSPC", "ENOTTY", "ENOMEM", "client"}
    if set(counts) - allowed:
        unexpected.append({"what": "random requests", "got": sorted(set(counts) - allowed)})
    try:
        second.configure(1920, 1080)
        second.works("after the random requests")
    except OSError as error:
        unexpected.append({"what": "configure after the random requests", "got": errno.errorcode.get(error.errno)})
    second.close()

    # A thousand streams of the largest size, each opened, configured, used
    # for one frame and closed.
    streams = 0
    for _ in range(STREAMS):
        try:
            client = Client()
            client.configure(1920, 1088)
            client.frame()
            client.close()
            streams += 1
        except OSError as error:
            unexpected.append({"what": f"stream {streams} of {STREAMS}", "got": errno.errorcode.get(error.errno)})
            break
    report["streams_one_after_the_other"] = streams


def main():
    report = {"device": DEVICE}
    before_log = kernel_log()
    before_free = cma_free()
    try:
        groups(report)
    except Exception as error:  # the report must come out whatever happens
        unexpected.append({"what": "the test itself", "got": repr(error)})
    time.sleep(0.5)
    after_log = kernel_log()
    after_free = cma_free()
    seen = set(before_log)
    new = [line for line in after_log if line not in seen]
    # The kernel notes every drop of the page cache with the name and the
    # number of the process that asked for it: those lines are this test's.
    own = f"({os.getpid()}): drop_caches: "
    report["kernel_lines"] = [line for line in new if own not in line]
    report["own_kernel_lines"] = len(new) - len(report["kernel_lines"])
    report["cma_free_before_kb"] = before_free
    report["cma_free_after_kb"] = after_free
    report["cma_free_slack_kb"] = SLACK_KB
    kept = before_free is not None and after_free is not None and after_free >= before_free - SLACK_KB
    report["nothing_left_taken"] = kept
    report["requests_tried"] = tried
    report["unexpected"] = unexpected
    report["passed"] = not unexpected and not report["kernel_lines"] and kept
    text = json.dumps(report, indent=1)
    if len(sys.argv) > 1:
        with open(sys.argv[1], "w", encoding="ascii") as out:
            out.write(text + "\n")
    print(text)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
