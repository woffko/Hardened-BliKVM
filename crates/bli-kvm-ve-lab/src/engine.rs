//! What the lab tool needs of an engine, how a plan is carried out on one,
//! and a stand-in that behaves like the encoder block of the open maps
//! without being hardware.

use std::{
    io,
    thread::sleep,
    time::{Duration, Instant},
};

use crate::{
    bits::{BitSink, Put},
    regs::{self, Op},
    standin,
};

/// The engine's registers and the memory it can reach.
pub trait Engine {
    fn read(&mut self, reg: u32) -> u32;
    fn write(&mut self, reg: u32, value: u32);
    /// Waits for the encoder's interrupt; false when the time runs out.
    fn wait(&mut self, timeout: Duration) -> bool;
    /// A buffer the engine can reach; the result names it below.
    fn allocate(&mut self, bytes: usize) -> io::Result<usize>;
    /// The buffer's address as the engine sees it.
    fn address(&self, buffer: usize) -> u32;
    fn bytes(&mut self, buffer: usize) -> &mut [u8];
}

/// A step that was carried out, and what the engine answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub op: Op,
    /// What was read: the register before an update, the register when a
    /// wait for bits ended, the status after the wait for the encode.
    pub read: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// Bits that were waited for did not come.
    NotReady { reg: u32, mask: u32, read: u32 },
    /// The encode did not end in time.
    TimedOut { status: u32 },
}

impl std::fmt::Display for Fault {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotReady { reg, mask, read } => write!(
                formatter,
                "register {reg:#05x} read {read:#010x} and never had {mask:#x}"
            ),
            Self::TimedOut { status } => {
                write!(formatter, "the encode did not end; status {status:#x}")
            }
        }
    }
}

/// How long a plan waits.
#[derive(Debug, Clone, Copy)]
pub struct Patience {
    pub bits: Duration,
    pub encode: Duration,
}

impl Default for Patience {
    fn default() -> Self {
        Self {
            bits: Duration::from_millis(100),
            encode: Duration::from_secs(1),
        }
    }
}

/// What carrying out a plan gave besides its steps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Outcome {
    /// From the start trigger to the end of the wait.
    pub encode_time: Option<Duration>,
}

/// Carries out `ops` in order and records every step in `trace`.
pub fn run(
    engine: &mut dyn Engine,
    ops: &[Op],
    patience: Patience,
    trace: &mut Vec<Step>,
) -> Result<Outcome, Fault> {
    let mut outcome = Outcome::default();
    let mut started = None;
    for &op in ops {
        match op {
            Op::Write { reg, value } => {
                if reg == regs::AVC_TRIGGER && value & 0xf == regs::TRIGGER_START {
                    started = Some(Instant::now());
                }
                engine.write(reg, value);
                trace.push(Step { op, read: None });
            }
            Op::Update { reg, clear, set } => {
                let before = engine.read(reg);
                engine.write(reg, (before & !clear) | set);
                trace.push(Step {
                    op,
                    read: Some(before),
                });
            }
            Op::Until { reg, mask } => {
                let deadline = Instant::now() + patience.bits;
                let mut spins = 0u32;
                let read = loop {
                    let read = engine.read(reg);
                    if read & mask == mask {
                        break read;
                    }
                    if Instant::now() >= deadline {
                        trace.push(Step {
                            op,
                            read: Some(read),
                        });
                        return Err(Fault::NotReady { reg, mask, read });
                    }
                    spins += 1;
                    if spins > 64 {
                        sleep(Duration::from_micros(50));
                    }
                };
                trace.push(Step {
                    op,
                    read: Some(read),
                });
            }
            Op::Wait => {
                let came = engine.wait(patience.encode);
                outcome.encode_time = started.map(|start| start.elapsed());
                let status = engine.read(regs::AVC_STATUS);
                trace.push(Step {
                    op,
                    read: Some(status),
                });
                if !came && status & regs::STATUS_ENDED == 0 {
                    return Err(Fault::TimedOut { status });
                }
            }
        }
    }
    Ok(outcome)
}

/// The version the stand-in reports: no engine has it.
pub const FAKE_VERSION: u32 = 0xfa4e;
const FAKE_BASE: u32 = 0x4000_0000;
const FAKE_SPACING: u32 = 0x0100_0000;

/// The encoder block of the open maps, in software. It keeps registers,
/// collects what the bit writer is given, and "encodes" with the stand-in's
/// slice data. It accepts both ways of selecting the encoder and refuses to
/// start without one, or with an address that is not in a buffer.
#[derive(Debug, Default)]
pub struct FakeEngine {
    registers: std::collections::BTreeMap<u32, u32>,
    buffers: Vec<Vec<u8>>,
    sink: BitSink,
    /// Whether the bit writer inserts emulation prevention bytes now.
    prevention: bool,
    status: u32,
    interrupt: bool,
    /// Every write in order: the register file of a run.
    pub writes: Vec<(u32, u32)>,
}

impl FakeEngine {
    pub fn new() -> Self {
        Self::default()
    }

    fn register(&self, reg: u32) -> u32 {
        self.registers.get(&reg).copied().unwrap_or(0)
    }

    /// The buffer an address lies in, and where in it.
    fn locate(&self, address: u32) -> Option<(usize, usize)> {
        let index = address.checked_sub(FAKE_BASE)? / FAKE_SPACING;
        let offset = (address - FAKE_BASE) % FAKE_SPACING;
        let buffer = self.buffers.get(index as usize)?;
        (offset as usize <= buffer.len()).then_some((index as usize, offset as usize))
    }

    fn selected(&self) -> bool {
        let mode = self.register(regs::MODE);
        mode & 0xf == 0xb || mode & regs::MODE_ENCODER != 0
    }

    /// Copies what the bit writer holds to the stream buffer. False when it
    /// does not fit between the buffer's start and end registers.
    fn mirror(&mut self) -> bool {
        let start = self.register(regs::AVC_STREAM_START);
        let end = self.register(regs::AVC_STREAM_END);
        let Some((index, offset)) = self.locate(start) else {
            return false;
        };
        let written = self.sink.written();
        let room = (end.wrapping_sub(start) as usize).saturating_add(1);
        let buffer = &mut self.buffers[index];
        if written.len() > room || offset + written.len() > buffer.len() {
            return false;
        }
        buffer[offset..offset + written.len()].copy_from_slice(written);
        true
    }

    fn put_bits(&mut self, count: u32) {
        if count == 0 || count > 32 {
            return;
        }
        let data = self.register(regs::AVC_PUT_DATA);
        let mask = if count == 32 {
            u32::MAX
        } else {
            (1 << count) - 1
        };
        let prevent = self.register(regs::AVC_PARA0) & regs::PARA0_NO_PREVENTION == 0;
        // The engine's state follows the bit, not the moment it changed.
        if prevent != self.prevention {
            self.sink.set_prevention(prevent);
            self.prevention = prevent;
        }
        self.sink.put(Put::new(data & mask, count as u8));
        if !self.mirror() {
            self.status |= regs::STATUS_STALL;
        }
    }

    fn start(&mut self) {
        self.interrupt = true;
        let size = self.register(regs::ISP_SIZE);
        let (columns, rows) = ((size >> 16) as usize, (size & 0xffff) as usize);
        let stride = (self.register(regs::ISP_STRIDE) >> 16) as usize * 16;
        let key = self.register(regs::AVC_PARA0) & regs::PARA0_P_SLICE == 0;
        let luma = self.locate(self.register(regs::ISP_INPUT_LUMA));
        let chroma = self.locate(self.register(regs::ISP_INPUT_CHROMA));
        let reconstruction = self.locate(self.register(regs::AVC_REC_LUMA));
        let (Some(luma), Some(chroma), Some(_)) = (luma, chroma, reconstruction) else {
            self.status |= regs::STATUS_STALL;
            return;
        };
        let luma_bytes = stride * rows * 16;
        let chroma_bytes = stride * rows * 8;
        if !self.selected()
            || columns == 0
            || rows == 0
            || stride < columns * 16
            || luma.1 + luma_bytes > self.buffers[luma.0].len()
            || chroma.1 + chroma_bytes > self.buffers[chroma.0].len()
        {
            self.status |= regs::STATUS_STALL;
            return;
        }
        if !self.prevention {
            self.sink.set_prevention(true);
            self.prevention = true;
        }
        let luma_plane = self.buffers[luma.0][luma.1..luma.1 + luma_bytes].to_vec();
        let chroma_plane = self.buffers[chroma.0][chroma.1..chroma.1 + chroma_bytes].to_vec();
        standin::slice_data(
            &mut self.sink,
            columns,
            rows,
            key,
            &luma_plane,
            &chroma_plane,
            stride,
        );
        self.status |= if self.mirror() {
            regs::STATUS_FINISH
        } else {
            regs::STATUS_STALL
        };
    }
}

impl Engine for FakeEngine {
    fn read(&mut self, reg: u32) -> u32 {
        match reg {
            regs::VERSION => FAKE_VERSION << 16,
            regs::RESET => self.register(reg) | regs::RESET_IDLE,
            regs::AVC_STATUS => self.status | regs::STATUS_PUT_READY,
            regs::AVC_STREAM_LENGTH => self.sink.bit_length() as u32,
            _ => self.register(reg),
        }
    }

    fn write(&mut self, reg: u32, value: u32) {
        self.writes.push((reg, value));
        match reg {
            // Written ones clear.
            regs::AVC_STATUS => self.status &= !value,
            regs::AVC_STREAM_OFFSET => {
                // A new frame's stream begins.
                self.sink = BitSink::new();
                self.prevention = false;
                self.registers.insert(reg, value);
            }
            regs::AVC_TRIGGER => match value & 0xf {
                regs::TRIGGER_PUT_BITS => self.put_bits((value >> 8) & 0x3f),
                regs::TRIGGER_START => self.start(),
                _ => {}
            },
            _ => {
                self.registers.insert(reg, value);
            }
        }
    }

    fn wait(&mut self, _timeout: Duration) -> bool {
        std::mem::take(&mut self.interrupt)
    }

    fn allocate(&mut self, bytes: usize) -> io::Result<usize> {
        if bytes == 0 || bytes > FAKE_SPACING as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a buffer of 1 byte to 16 MiB",
            ));
        }
        self.buffers.push(vec![0; bytes]);
        Ok(self.buffers.len() - 1)
    }

    fn address(&self, buffer: usize) -> u32 {
        FAKE_BASE + buffer as u32 * FAKE_SPACING
    }

    fn bytes(&mut self, buffer: usize) -> &mut [u8] {
        &mut self.buffers[buffer]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_is_carried_out_in_order_and_traced() {
        let mut engine = FakeEngine::new();
        let mut trace = Vec::new();
        let ops = [
            Op::Write {
                reg: regs::AVC_PARA1,
                value: 5,
            },
            Op::Update {
                reg: regs::AVC_PARA1,
                clear: 1,
                set: 0x10,
            },
            Op::Until {
                reg: regs::RESET,
                mask: regs::RESET_IDLE,
            },
        ];
        let outcome = run(&mut engine, &ops, Patience::default(), &mut trace).unwrap();
        assert_eq!(outcome.encode_time, None);
        assert_eq!(engine.read(regs::AVC_PARA1), 0x14);
        assert_eq!(
            engine.writes,
            [(regs::AVC_PARA1, 5), (regs::AVC_PARA1, 0x14)]
        );
        assert_eq!(trace[1].read, Some(5));
        assert_eq!(trace[2].read, Some(regs::RESET_IDLE));
    }

    #[test]
    fn bits_that_do_not_come_and_an_encode_that_does_not_end_are_faults() {
        let mut engine = FakeEngine::new();
        let mut trace = Vec::new();
        let patience = Patience {
            bits: Duration::from_millis(5),
            encode: Duration::from_millis(5),
        };
        let never = [Op::Until {
            reg: regs::AVC_PARA2,
            mask: 1,
        }];
        assert_eq!(
            run(&mut engine, &never, patience, &mut trace),
            Err(Fault::NotReady {
                reg: regs::AVC_PARA2,
                mask: 1,
                read: 0
            })
        );
        // Nothing was triggered, so nothing ends.
        assert_eq!(
            run(&mut engine, &[Op::Wait], patience, &mut trace),
            Err(Fault::TimedOut { status: 1 << 9 })
        );
    }

    #[test]
    fn the_stand_in_refuses_to_start_without_an_encoder_or_a_buffer() {
        let mut engine = FakeEngine::new();
        engine.write(regs::AVC_TRIGGER, regs::TRIGGER_START);
        assert_eq!(
            engine.read(regs::AVC_STATUS) & regs::STATUS_ENDED,
            regs::STATUS_STALL
        );
        assert!(engine.wait(Duration::ZERO));
        assert!(!engine.wait(Duration::ZERO));
        engine.write(regs::AVC_STATUS, regs::STATUS_ENDED);
        assert_eq!(engine.read(regs::AVC_STATUS) & regs::STATUS_ENDED, 0);
    }

    #[test]
    fn the_bit_writer_fills_the_stream_buffer_within_its_bounds() {
        let mut engine = FakeEngine::new();
        let buffer = engine.allocate(8).unwrap();
        let address = engine.address(buffer);
        engine.write(regs::AVC_STREAM_OFFSET, 0);
        engine.write(regs::AVC_STREAM_START, address);
        engine.write(regs::AVC_STREAM_END, address + 3);
        engine.write(regs::AVC_PARA0, regs::PARA0_NO_PREVENTION);
        for (value, bits) in [(0, 16), (1, 16)] {
            engine.write(regs::AVC_PUT_DATA, value);
            engine.write(regs::AVC_TRIGGER, 1 | bits << 8);
        }
        assert_eq!(engine.read(regs::AVC_STREAM_LENGTH), 32);
        assert_eq!(engine.bytes(buffer)[..4], [0, 0, 0, 1]);
        assert_eq!(engine.read(regs::AVC_STATUS) & regs::STATUS_ENDED, 0);
        // A fifth byte is beyond the end register.
        engine.write(regs::AVC_PUT_DATA, 0x67);
        engine.write(regs::AVC_TRIGGER, 1 | 8 << 8);
        assert_eq!(
            engine.read(regs::AVC_STATUS) & regs::STATUS_ENDED,
            regs::STATUS_STALL
        );
        assert_eq!(engine.bytes(buffer)[4], 0);
    }
}
