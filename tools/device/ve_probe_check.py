# On the board: the test module's device answers its request and maps its page.
import fcntl, json, mmap, os, struct

fd = os.open("/dev/hardened-blikvm-ve-probe-test", os.O_RDWR)
answer = bytearray(4)
fcntl.ioctl(fd, 0x5601, answer)
refused = False
try:
    fcntl.ioctl(fd, 0x5602, bytearray(4))
except OSError:
    refused = True
page = mmap.mmap(fd, 4096, mmap.MAP_SHARED, mmap.PROT_READ | mmap.PROT_WRITE)
text = bytes(page[:18])
page[100:104] = b"lab!"
again = bytes(page[100:104])
page.close()
os.close(fd)
result = {
    "ioctl": struct.unpack("<I", answer)[0],
    "otherRequestRefused": refused,
    "mapped": text.decode("ascii", "replace"),
    "writtenThroughMapping": again.decode("ascii", "replace"),
}
assert result["ioctl"] == 0x48363136 and refused
assert text == b"hardened-blikvm-ve" and again == b"lab!"
print(json.dumps(result))
