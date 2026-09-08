import socket
import subprocess
import unittest

from tools.status_outage_channel import PhaseChannel


class PhaseChannelTests(unittest.TestCase):
    def test_real_node_inherited_fd_is_separate_from_finite_stdin(self) -> None:
        first, second = socket.socketpair()
        source = """
import assert from 'node:assert/strict';
import net from 'node:net';
import { StatusOutageControl } from './tools/playwright/status_outage_control.mjs';
let input = '';
for await (const chunk of process.stdin) input += chunk.toString('utf8');
assert.equal(input, 'synthetic-finite-input');
const control = new StatusOutageControl(new net.Socket({fd:Number(process.argv[1]),readable:true,writable:true}), 2000);
try { await control.request('stop'); await control.request('restore'); }
finally { control.close(); }
process.stdout.write('passed');
"""
        with first, second:
            first.settimeout(3)
            with subprocess.Popen(["node", "--input-type=module", "-e", source, str(second.fileno())],
                                  stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                  pass_fds=(second.fileno(),)) as process:
                try:
                    assert process.stdin is not None
                    process.stdin.write(b"synthetic-finite-input")
                    process.stdin.close()
                    channel = PhaseChannel(first)
                    for request_id, phase in ((1, "stop"), (2, "restore")):
                        self.assertEqual(channel.receive(), (request_id, phase))
                        channel.reply(request_id, phase, True)
                    self.assertEqual(process.wait(timeout=3), 0)
                    assert process.stdout is not None
                    self.assertEqual(process.stdout.read(100), b"passed")
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.wait(timeout=3)

    def test_exact_order_and_reply(self) -> None:
        first, second = socket.socketpair()
        with first, second:
            first.settimeout(1)
            second.settimeout(1)
            channel = PhaseChannel(first)
            for request_id, phase in ((1, "stop"), (2, "restore"), (3, "restore")):
                second.sendall(('{{"v":1,"id":{},"phase":"{}"}}\n'.format(request_id, phase)).encode())
                self.assertEqual(channel.receive(), (request_id, phase))
                channel.reply(request_id, phase, True)
                self.assertEqual(second.recv(256), ('{{"v":1,"id":{},"phase":"{}","ok":true}}\n'.format(request_id, phase)).encode())

    def test_malformed_duplicate_oversize_eof_and_timeout(self) -> None:
        for raw in (b'{}\n', b'{"v":1,"id":1,"id":1,"phase":"stop"}\n',
                    b'{"v":1,"id":1,"phase":"restore"}\n', b'{"v":1,"id":true,"phase":"stop"}\n',
                    b'x' * 257, b'{}\n{}\n', b'{'):
            first, second = socket.socketpair()
            with first, second:
                first.settimeout(1)
                second.sendall(raw)
                second.shutdown(socket.SHUT_WR)
                with self.assertRaises(ValueError):
                    PhaseChannel(first).receive()
        first, second = socket.socketpair()
        with first, second:
            first.settimeout(0.01)
            with self.assertRaises(TimeoutError):
                PhaseChannel(first).receive()
            second.shutdown(socket.SHUT_WR)
            self.assertIsNone(PhaseChannel(first).receive())


if __name__ == "__main__":
    unittest.main()
