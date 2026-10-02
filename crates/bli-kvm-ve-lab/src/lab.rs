//! The first experiments of the lab, written against any engine so that the
//! stand-in can be put through them on the host: reading the blocks the
//! open maps name, checking buffers, and finding which bits of the encoder's
//! registers can be set.

use std::io;

use crate::{
    engine::{Engine, Patience, Step, run},
    regs::{self, Hypothesis},
};

/// The blocks that are read: the top level, the input stage, the encoder,
/// and the JPEG decoder of the sun50i parts.
pub const BLOCKS: [(&str, u32, u32); 4] = [
    ("top", 0x000, 0x100),
    ("input stage", 0xa00, 0xb00),
    ("encoder", 0xb00, 0xc00),
    ("jpeg decoder", 0xe00, 0xf00),
];

/// Every register of a block, read once and in order.
pub fn dump(engine: &mut dyn Engine, from: u32, to: u32) -> Vec<(u32, u32)> {
    (from..to)
        .step_by(4)
        .map(|reg| (reg, engine.read(reg)))
        .collect()
}

/// What a buffer check found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferCheck {
    pub bytes: usize,
    pub address: u32,
    /// What was written through the mapping was read back.
    pub holds: bool,
    /// The buffer was all zero when it was given.
    pub zeroed: bool,
}

/// Allocates buffers of the given sizes, checks that each arrives zeroed and
/// keeps a pattern written through its mapping.
pub fn check_buffers(engine: &mut dyn Engine, sizes: &[usize]) -> io::Result<Vec<BufferCheck>> {
    let mut checks = Vec::new();
    for (number, &bytes) in sizes.iter().enumerate() {
        let buffer = engine.allocate(bytes)?;
        let address = engine.address(buffer);
        let memory = engine.bytes(buffer);
        let zeroed = memory.iter().all(|byte| *byte == 0);
        let pattern = |position: usize| {
            (position as u32).wrapping_mul(2_654_435_761).to_le_bytes()[3] ^ number as u8
        };
        for (position, byte) in memory.iter_mut().enumerate() {
            *byte = pattern(position);
        }
        let holds = memory
            .iter()
            .enumerate()
            .all(|(position, byte)| *byte == pattern(position));
        checks.push(BufferCheck {
            bytes: memory.len(),
            address,
            holds,
            zeroed,
        });
    }
    Ok(checks)
}

/// What a register did when all the bits it may be given were set and then
/// cleared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaskRow {
    pub reg: u32,
    /// The bits that were tried.
    pub tried: u32,
    pub before: u32,
    /// Read after all tried bits were written as ones.
    pub ones: u32,
    /// Read after zero was written.
    pub zeros: u32,
    /// Read after the first value was written back.
    pub after: u32,
}

/// The bits of the input stage's control register that the open map names,
/// without its interrupt enable: the lab never enables an interrupt it does
/// not wait for.
pub const ISP_CTRL_NAMED: u32 = 0xf879_0006;

/// Selects the encoder as `hypothesis` says and tries the bits of every
/// register of `regs::MASK_PROBE`. No trigger is written.
pub fn probe_masks(
    engine: &mut dyn Engine,
    hypothesis: &Hypothesis,
    trace: &mut Vec<Step>,
) -> Result<Vec<MaskRow>, crate::engine::Fault> {
    run(
        engine,
        &regs::select(hypothesis),
        Patience::default(),
        trace,
    )?;
    let mut rows = Vec::new();
    for reg in regs::MASK_PROBE {
        let tried = if reg == regs::ISP_CTRL {
            ISP_CTRL_NAMED
        } else {
            u32::MAX
        };
        let before = engine.read(reg);
        engine.write(reg, tried);
        let ones = engine.read(reg);
        engine.write(reg, 0);
        let zeros = engine.read(reg);
        engine.write(reg, before);
        let after = engine.read(reg);
        rows.push(MaskRow {
            reg,
            tried,
            before,
            ones,
            zeros,
            after,
        });
    }
    run(
        engine,
        &regs::deselect(hypothesis),
        Patience::default(),
        trace,
    )?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::FakeEngine;

    #[test]
    fn blocks_are_read_whole_and_in_order() {
        let mut engine = FakeEngine::new();
        engine.write(regs::AVC_PARA1, 0x1234);
        let block = dump(&mut engine, 0xb00, 0xc00);
        assert_eq!(block.len(), 64);
        assert_eq!(block[0].0, 0xb00);
        assert_eq!(block[2], (regs::AVC_PARA1, 0x1234));
        assert_eq!(block[63].0, 0xbfc);
        // Reading wrote nothing but what the test wrote.
        assert_eq!(engine.writes.len(), 1);
    }

    #[test]
    fn buffers_arrive_zeroed_and_keep_what_is_written() {
        let mut engine = FakeEngine::new();
        let checks = check_buffers(&mut engine, &[4096, 65536]).unwrap();
        assert_eq!(checks.len(), 2);
        assert!(checks.iter().all(|check| check.holds && check.zeroed));
        assert_ne!(checks[0].address, checks[1].address);
        assert_eq!(checks[1].bytes, 65536);
    }

    #[test]
    fn the_mask_probe_writes_no_trigger_and_puts_every_register_back() {
        for hypothesis in [Hypothesis::OLDER, Hypothesis::V3S] {
            let mut engine = FakeEngine::new();
            engine.write(regs::AVC_ME, 0x104);
            let mut trace = Vec::new();
            let rows = probe_masks(&mut engine, &hypothesis, &mut trace).unwrap();
            assert_eq!(rows.len(), regs::MASK_PROBE.len());
            for row in &rows {
                // The stand-in keeps every bit it is given.
                assert_eq!(row.ones, row.tried, "{:#x}", row.reg);
                assert_eq!(row.zeros, 0);
                assert_eq!(row.after, row.before);
            }
            let me = rows.iter().find(|row| row.reg == regs::AVC_ME).unwrap();
            assert_eq!(me.before, 0x104);
            let control = rows.iter().find(|row| row.reg == regs::ISP_CTRL).unwrap();
            assert_eq!(control.tried & 1, 0);
            for (reg, _) in &engine.writes {
                assert!(
                    ![regs::AVC_TRIGGER, regs::AVC_INT_EN, regs::AVC_PUT_DATA].contains(reg),
                    "{reg:#x} was written"
                );
            }
            // The encoder is deselected at the end.
            let mode = engine.read(regs::MODE);
            assert_eq!(mode & 0xf, 7);
            assert_eq!(mode & (regs::MODE_ENCODER | regs::MODE_INPUT_STAGE), 0);
        }
    }
}
