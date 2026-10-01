//! Random and damaged input for every decoder of the transport: none may
//! panic, take memory for a length it has not checked, or accept a value it
//! would refuse to encode.

use bli_kvm_quic_protocol::{
    frame::{self, FrameHeader, HEADER_BYTES, Kind, MAX_DATAGRAM_BYTES, MAX_FRAME_BYTES},
    link::{self, GatewayToServer, MAX_LINK_MESSAGE_BYTES, ServerToGateway},
    page::{self, GatewayControl, InputRequest, MAX_MESSAGE_BYTES, MessageReader, PageControl},
};

const CASES: usize = 100_000;

/// A small deterministic generator: the cases are the same on every run.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }

    fn bytes(&mut self, length: usize) -> Vec<u8> {
        (0..length).map(|_| self.next() as u8).collect()
    }
}

/// Flips, cuts or extends a valid encoding.
fn damage(random: &mut Random, mut bytes: Vec<u8>) -> Vec<u8> {
    match random.below(4) {
        0 => {
            let at = random.below(bytes.len());
            bytes[at] ^= 1 << random.below(8);
        }
        1 => {
            let at = random.below(bytes.len());
            bytes[at] = random.next() as u8;
        }
        2 => bytes.truncate(random.below(bytes.len())),
        _ => {
            let extra = random.below(8) + 1;
            let tail = random.bytes(extra);
            bytes.extend_from_slice(&tail);
        }
    }
    bytes
}

#[test]
fn frame_headers_survive_random_and_damaged_bytes() {
    let mut random = Random(0x9e37_79b9_7f4a_7c15);
    let mut accepted = 0usize;
    for case in 0..CASES {
        let bytes = if case % 2 == 0 {
            let length = random.below(HEADER_BYTES + 8);
            random.bytes(length)
        } else {
            let frame = (random.next() as u32).max(2);
            let payload = vec![0u8; random.below(5000) + 1];
            let datagrams = frame::fragment(
                (random.next() as u32).max(1),
                frame,
                random.below(frame as usize - 1) as u32 + 1,
                random.next(),
                &payload,
                200 + random.below(1200),
            )
            .unwrap();
            let pick = random.below(datagrams.len());
            damage(&mut random, datagrams[pick].clone())
        };
        if let Ok(header) = FrameHeader::decode(&bytes) {
            accepted += 1;
            // Whatever is accepted is a header this side would send itself.
            assert_eq!(&header.encode().unwrap()[..], &bytes[..HEADER_BYTES]);
            assert!(header.size <= MAX_FRAME_BYTES);
            assert!(header.fragment_index < header.fragment_count);
            if header.kind == Kind::Delta {
                for capacity in [1usize, 168, MAX_DATAGRAM_BYTES - HEADER_BYTES] {
                    if let Some(expected) = frame::expected_fragment_bytes(&header, capacity) {
                        assert!((1..=capacity).contains(&expected));
                    }
                }
            }
        }
    }
    assert!(
        accepted > CASES / 20,
        "the damaged cases must include valid headers"
    );
}

#[test]
fn page_messages_survive_random_and_damaged_bytes() {
    let mut random = Random(0x2545_f491_4f6c_dd1d);
    let valid: Vec<Vec<u8>> = vec![
        page::encode(&PageControl::Hello { protocol: 1, token: "0f".repeat(32) }).unwrap(),
        page::encode(&PageControl::Ping { id: 9, page_time: 1234.5 }).unwrap(),
        page::encode(&PageControl::Keyframe { stream: 1, last_frame: 2, dropped: 3 }).unwrap(),
        page::encode(&PageControl::Video { subscribe: true }).unwrap(),
        page::encode(&InputRequest {
            seq: 3,
            request: serde_json::json!({"action": "report", "report": {"mode": "keyboard", "data": [0, 0, 4, 0, 0, 0, 0, 0]}}),
        })
        .unwrap(),
    ];
    let mut parsed = 0usize;
    for case in 0..CASES {
        let bytes = if case % 3 == 0 {
            let length = random.below(64);
            random.bytes(length)
        } else {
            let pick = random.below(valid.len());
            damage(&mut random, valid[pick].clone())
        };
        let mut reader = MessageReader::new();
        reader.push(&bytes);
        // The reader keeps what it was given and nothing more.
        assert!(reader.pending() <= bytes.len());
        for _ in 0..4 {
            match reader.take::<PageControl>() {
                Ok(Some(message)) => {
                    parsed += 1;
                    assert!(page::encode(&message).unwrap().len() <= 2 + MAX_MESSAGE_BYTES);
                }
                Ok(None) | Err(_) => break,
            }
        }
        let mut reader = MessageReader::new();
        reader.push(&bytes);
        let _ = reader.take::<GatewayControl>();
        let mut reader = MessageReader::new();
        reader.push(&bytes);
        let _ = reader.take::<InputRequest>();
    }
    assert!(parsed > 0);
}

#[test]
fn link_messages_survive_random_and_damaged_bytes() {
    let mut random = Random(0xd6e8_feb8_6659_fd93);
    let valid: Vec<Vec<u8>> = vec![
        link::encode(&GatewayToServer::Redeem {
            id: 1,
            token: "0f".repeat(32),
            peer: "192.0.2.1:5000".into(),
        })
        .unwrap(),
        link::encode(&GatewayToServer::Closed {
            session: "abc".into(),
        })
        .unwrap(),
        link::encode(&ServerToGateway::Redeemed {
            id: 1,
            session: Some("abc".into()),
            refused: None,
        })
        .unwrap(),
        link::encode(&ServerToGateway::Hello { protocol: 1 }).unwrap(),
    ];
    for case in 0..CASES {
        let bytes = if case % 3 == 0 {
            let length = random.below(64);
            random.bytes(length)
        } else {
            let pick = random.below(valid.len());
            damage(&mut random, valid[pick].clone())
        };
        if bytes.len() < 4 {
            continue;
        }
        if let Ok(length) = link::body_length([bytes[0], bytes[1], bytes[2], bytes[3]]) {
            assert!((1..=MAX_LINK_MESSAGE_BYTES).contains(&length));
            if let Some(body) = bytes.get(4..4 + length) {
                let _ = link::decode::<GatewayToServer>(body);
                let _ = link::decode::<ServerToGateway>(body);
            }
        }
    }
}
