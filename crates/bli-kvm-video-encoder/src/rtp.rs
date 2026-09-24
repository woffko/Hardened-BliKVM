//! RTP packetization of encoded VP8 frames (RFC 3550, RFC 7741).
//!
//! Each frame is split into packets no larger than the configured MTU. The
//! VP8 payload descriptor carries a 15-bit picture ID so receivers can detect
//! loss; the marker bit ends a frame. No RTCP or retransmission is done here.

use thiserror::Error;

const RTP_HEADER_BYTES: usize = 12;
/// X=1 with I=1 and a 15-bit PictureID; S marks the first packet of a frame.
const DESCRIPTOR_BYTES: usize = 4;
const VP8_CLOCK_HZ: u64 = 90_000;
pub const MIN_MTU: usize = 256;
pub const MAX_MTU: usize = 1500;
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RtpError {
    #[error("RTP configuration is invalid")]
    InvalidConfig,
    #[error("encoded frame is empty or too large")]
    InvalidFrame,
}

#[derive(Debug)]
pub struct Vp8Packetizer {
    payload_type: u8,
    ssrc: u32,
    mtu: usize,
    sequence: u16,
    picture_id: u16,
}

impl Vp8Packetizer {
    /// `payload_type` must be dynamic (96..=127); the SSRC and initial
    /// sequence number should be random per session.
    pub fn new(
        payload_type: u8,
        ssrc: u32,
        initial_sequence: u16,
        mtu: usize,
    ) -> Result<Self, RtpError> {
        if !(96..=127).contains(&payload_type) || !(MIN_MTU..=MAX_MTU).contains(&mtu) {
            return Err(RtpError::InvalidConfig);
        }
        Ok(Self {
            payload_type,
            ssrc,
            mtu,
            sequence: initial_sequence,
            picture_id: 0,
        })
    }

    /// RTP timestamp for a capture time in microseconds since stream start.
    pub fn timestamp(elapsed_micros: u64) -> u32 {
        (elapsed_micros.wrapping_mul(VP8_CLOCK_HZ) / 1_000_000) as u32
    }

    /// Packetize one encoded frame. VP8 signals key frames in its own
    /// payload header, so no separate flag is carried here.
    pub fn packetize(&mut self, frame: &[u8], timestamp: u32) -> Result<Vec<Vec<u8>>, RtpError> {
        if frame.is_empty() || frame.len() > MAX_FRAME_BYTES {
            return Err(RtpError::InvalidFrame);
        }
        let chunk = self.mtu - RTP_HEADER_BYTES - DESCRIPTOR_BYTES;
        let picture_id = self.picture_id;
        self.picture_id = (self.picture_id + 1) & 0x7fff;
        let count = frame.len().div_ceil(chunk);
        let mut packets = Vec::with_capacity(count);
        for (index, payload) in frame.chunks(chunk).enumerate() {
            let last = index + 1 == count;
            let mut packet =
                Vec::with_capacity(RTP_HEADER_BYTES + DESCRIPTOR_BYTES + payload.len());
            packet.push(0x80);
            packet.push(self.payload_type | if last { 0x80 } else { 0 });
            packet.extend_from_slice(&self.sequence.to_be_bytes());
            packet.extend_from_slice(&timestamp.to_be_bytes());
            packet.extend_from_slice(&self.ssrc.to_be_bytes());
            // X=1, N=0, S=start of partition 0, PID=0.
            packet.push(0x80 | if index == 0 { 0x10 } else { 0 });
            // I=1: PictureID present.
            packet.push(0x80);
            // M=1: 15-bit PictureID.
            packet.push(0x80 | (picture_id >> 8) as u8);
            packet.push(picture_id as u8);
            packet.extend_from_slice(payload);
            self.sequence = self.sequence.wrapping_add(1);
            packets.push(packet);
        }
        Ok(packets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_is_bounded() {
        assert!(Vp8Packetizer::new(95, 1, 0, 1200).is_err());
        assert!(Vp8Packetizer::new(128, 1, 0, 1200).is_err());
        assert!(Vp8Packetizer::new(96, 1, 0, MIN_MTU - 1).is_err());
        assert!(Vp8Packetizer::new(96, 1, 0, MAX_MTU + 1).is_err());
        let mut packetizer = Vp8Packetizer::new(96, 1, 0, 1200).unwrap();
        assert_eq!(packetizer.packetize(&[], 0), Err(RtpError::InvalidFrame));
        assert_eq!(
            packetizer.packetize(&vec![0; MAX_FRAME_BYTES + 1], 0),
            Err(RtpError::InvalidFrame)
        );
    }

    #[test]
    fn frames_split_at_the_mtu_with_marker_sequence_and_picture_id() {
        let mut packetizer = Vp8Packetizer::new(100, 0xdead_beef, 0xfffe, 300).unwrap();
        let frame: Vec<u8> = (0..700).map(|value| value as u8).collect();
        let packets = packetizer.packetize(&frame, 1234).unwrap();
        let chunk = 300 - RTP_HEADER_BYTES - DESCRIPTOR_BYTES;
        assert_eq!(packets.len(), frame.len().div_ceil(chunk));
        let mut reassembled = Vec::new();
        for (index, packet) in packets.iter().enumerate() {
            assert!(packet.len() <= 300);
            assert_eq!(packet[0], 0x80);
            assert_eq!(packet[1] & 0x7f, 100);
            assert_eq!(packet[1] & 0x80 != 0, index + 1 == packets.len());
            let sequence = u16::from_be_bytes([packet[2], packet[3]]);
            assert_eq!(sequence, 0xfffe_u16.wrapping_add(index as u16));
            assert_eq!(u32::from_be_bytes(packet[4..8].try_into().unwrap()), 1234);
            assert_eq!(
                u32::from_be_bytes(packet[8..12].try_into().unwrap()),
                0xdead_beef
            );
            assert_eq!(packet[12] & 0x10 != 0, index == 0);
            assert_eq!(packet[12] & 0x80, 0x80);
            assert_eq!(packet[13], 0x80);
            assert_eq!(u16::from_be_bytes([packet[14] & 0x7f, packet[15]]), 0);
            reassembled.extend_from_slice(&packet[16..]);
        }
        assert_eq!(reassembled, frame);
        let next = packetizer.packetize(&[1], 1235).unwrap();
        assert_eq!(u16::from_be_bytes([next[0][14] & 0x7f, next[0][15]]), 1);
        assert_eq!(next[0][1] & 0x80, 0x80);
    }

    #[test]
    fn picture_id_wraps_at_fifteen_bits_and_timestamps_use_90_khz() {
        let mut packetizer = Vp8Packetizer::new(96, 1, 0, 1200).unwrap();
        packetizer.picture_id = 0x7fff;
        let packet = &packetizer.packetize(&[1], 0).unwrap()[0];
        assert_eq!(u16::from_be_bytes([packet[14] & 0x7f, packet[15]]), 0x7fff);
        let packet = &packetizer.packetize(&[1], 0).unwrap()[0];
        assert_eq!(u16::from_be_bytes([packet[14] & 0x7f, packet[15]]), 0);
        assert_eq!(Vp8Packetizer::timestamp(1_000_000), 90_000);
        assert_eq!(Vp8Packetizer::timestamp(33_333), 2_999);
    }
}
