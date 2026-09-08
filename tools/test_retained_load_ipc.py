import json
import socket
import struct
import threading
import time
import unittest
from unittest.mock import patch

from tools import retained_load_ipc as ipc


VIDEO_HEADERS = b"HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=frame\r\n\r\n"


def response() -> bytes:
    return json.dumps({"schemaVersion": 1, "requestId": 1,
                       "response": {"type": "status", "status": {"generation": 22}}}).encode()


class IpcTests(unittest.TestCase):
    def test_status_request_is_fixed_and_requires_write_eof(self) -> None:
        client, server = socket.socketpair()
        errors: list[BaseException] = []

        def serve() -> None:
            try:
                with server:
                    deadline = time.monotonic() + 2
                    length = struct.unpack(">I", ipc.exact(server, 4, deadline))[0]
                    request = ipc.exact(server, length, deadline)
                    self.assertEqual(json.loads(request), {"schemaVersion": 1, "requestId": 1,
                                                          "request": {"type": "status"}})
                    self.assertEqual(ipc.receive(server, 1, deadline), b"")
                    data = response()
                    # Fragment every byte, including the length prefix.
                    for byte in struct.pack(">I", len(data)) + data:
                        server.sendall(bytes([byte]))
            except BaseException as error:
                errors.append(error)

        worker = threading.Thread(target=serve)
        worker.start()
        try:
            with patch.object(type(client), "connect") as connect, \
                    patch.object(ipc.socket, "socket", return_value=client):
                self.assertEqual(ipc.read_status(), {"generation": 22})
                connect.assert_called_once_with(ipc.STATUS_SOCKET)
        finally:
            client.close()
            worker.join(3)
        self.assertFalse(worker.is_alive())
        self.assertEqual(errors, [])

    def test_status_frame_failures_close_client(self) -> None:
        data = response()
        for wire in (b"", b"\x00\x00", struct.pack(">I", 0),
                     struct.pack(">I", ipc.MAX_STATUS + 1),
                     struct.pack(">I", len(data) + 1) + data,
                     struct.pack(">I", len(data)) + data + b"x"):
            with self.subTest(wire_length=len(wire)):
                client, server = socket.socketpair()
                try:
                    server.sendall(wire)
                    server.shutdown(socket.SHUT_WR)
                    with patch.object(type(client), "connect"), \
                            patch.object(ipc.socket, "socket", return_value=client):
                        with self.assertRaises(ValueError):
                            ipc.read_status()
                    self.assertEqual(client.fileno(), -1)
                finally:
                    client.close()
                    server.close()

    def test_status_envelope_rejects_error_mismatch_duplicates_and_nonfinite(self) -> None:
        self.assertEqual(ipc.decode_status(response(), 1), {"generation": 22})
        invalid = [
            b'[]', b'{}', response().replace(b'"requestId": 1', b'"requestId": 2'),
            response().replace(b'"schemaVersion": 1', b'"schemaVersion": true'),
            response().replace(b'"requestId": 1', b'"requestId": true'),
            response().replace(b'"generation": 22', b'"generation": NaN'),
            response().replace(b'"generation": 22', b'"generation": 22, "generation": 23'),
            b'{"schemaVersion":1,"requestId":1,"response":{"type":"error","code":"busy"}}',
            response().replace(b'"type": "status"', b'"type": "status", "extra": 1'),
        ]
        for data in invalid:
            with self.subTest(data=data), self.assertRaises(ValueError):
                ipc.decode_status(data, 1)

    def test_video_keeps_write_side_open_and_does_not_consume_body_as_headers(self) -> None:
        client, server = socket.socketpair()
        try:
            server.sendall(VIDEO_HEADERS + b"discarded-frame-data")
            with patch.object(type(client), "connect") as connect, \
                    patch.object(ipc.socket, "socket", return_value=client):
                stream = ipc.open_video()
                connect.assert_called_once_with(ipc.VIDEO_SOCKET)
            self.assertIs(stream, client)
            server.settimeout(0.02)
            self.assertEqual(server.recv(len(ipc.VIDEO_REQUEST)), ipc.VIDEO_REQUEST)
            with self.assertRaises(TimeoutError):
                server.recv(1)  # EOF here would cancel the real proxy stream.
            self.assertEqual(ipc.drain_video_chunk(stream, time.monotonic() + 1), 20)
            server.shutdown(socket.SHUT_WR)
            with self.assertRaises(ValueError):
                ipc.drain_video_chunk(stream, time.monotonic() + 1)
        finally:
            client.close()
            server.close()

    def test_video_bad_headers_and_eof_close_client(self) -> None:
        invalid = [b"", b"HTTP/1.1 200 OK\r\n", b"x" * (ipc.MAX_HEADERS + 1),
                   VIDEO_HEADERS.replace(b"200 OK", b"403 Forbidden"),
                   VIDEO_HEADERS.replace(b"multipart/x-mixed-replace", b"text/html"),
                   VIDEO_HEADERS.replace(b"\r\n\r\n", b"\r\nTransfer-Encoding: chunked\r\n\r\n"),
                   VIDEO_HEADERS.replace(b"\r\n\r\n", b"\r\nContent-Length: 1\r\n\r\n"),
                   VIDEO_HEADERS.replace(b"\r\n\r\n", b"\r\nContent-Type: text/html\r\n\r\n")]
        for data in invalid:
            with self.subTest(length=len(data)):
                client, server = socket.socketpair()
                try:
                    server.sendall(data)
                    server.shutdown(socket.SHUT_WR)
                    with patch.object(type(client), "connect"), \
                            patch.object(ipc.socket, "socket", return_value=client):
                        with self.assertRaises(ValueError):
                            ipc.open_video()
                    self.assertEqual(client.fileno(), -1)
                finally:
                    client.close()
                    server.close()

    def test_deadline_is_absolute_across_fragmented_reads(self) -> None:
        client, server = socket.socketpair()
        try:
            server.sendall(b"a")
            with patch.object(ipc.time, "monotonic", side_effect=[1, 3]):
                with self.assertRaises(TimeoutError):
                    ipc.exact(client, 2, 2)
            with self.assertRaises(TimeoutError):
                ipc.drain_video_chunk(client, time.monotonic() + 0.02)
        finally:
            client.close()
            server.close()

    def test_silent_peer_times_out_and_closes_both_request_clients(self) -> None:
        for operation in (ipc.open_video, ipc.read_status):
            with self.subTest(operation=operation.__name__):
                client, server = socket.socketpair()
                try:
                    with patch.object(type(client), "connect"), \
                            patch.object(ipc.socket, "socket", return_value=client), \
                            patch.object(ipc, "REQUEST_TIMEOUT", 0.02):
                        with self.assertRaises(TimeoutError):
                            operation()
                    self.assertEqual(client.fileno(), -1)
                finally:
                    client.close()
                    server.close()


if __name__ == "__main__":
    unittest.main()
