#![no_std]
#![forbid(unsafe_code)]

//! Pure bounded model for the H616 ST7789 240x240 display path.
//!
//! This crate models validated address-window commands, coordinate rotation,
//! RGB565 conversion, and exact-count MSB-first pixel chunks. It owns no bus,
//! GPIO, filesystem, clock, thread, network, or device capability.

pub const DISPLAY_WIDTH: u16 = 240;
pub const DISPLAY_HEIGHT: u16 = 240;
pub const DISPLAY_PIXEL_COUNT: u32 = DISPLAY_WIDTH as u32 * DISPLAY_HEIGHT as u32;
pub const BYTES_PER_PIXEL: usize = 2;
pub const MAX_PIXEL_CHUNK_PIXELS: usize = DISPLAY_WIDTH as usize;

pub const COMMAND_COLUMN_ADDRESS_SET: u8 = 0x2a;
pub const COMMAND_ROW_ADDRESS_SET: u8 = 0x2b;
pub const COMMAND_MEMORY_WRITE: u8 = 0x2c;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointError {
    OutOfBounds { x: u16, y: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DisplayPoint {
    x: u16,
    y: u16,
}

impl DisplayPoint {
    pub const fn new(x: u16, y: u16) -> Result<Self, PointError> {
        if x >= DISPLAY_WIDTH || y >= DISPLAY_HEIGHT {
            Err(PointError::OutOfBounds { x, y })
        } else {
            Ok(Self { x, y })
        }
    }

    pub const fn x(self) -> u16 {
        self.x
    }

    pub const fn y(self) -> u16 {
        self.y
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rotation {
    Degrees0,
    Degrees90,
    Degrees180,
    Degrees270,
}

impl Rotation {
    pub const fn map(self, point: DisplayPoint) -> DisplayPoint {
        let (x, y) = match self {
            Self::Degrees0 => (point.x, point.y),
            Self::Degrees90 => (DISPLAY_WIDTH - 1 - point.y, point.x),
            Self::Degrees180 => (DISPLAY_WIDTH - 1 - point.x, DISPLAY_HEIGHT - 1 - point.y),
            Self::Degrees270 => (point.y, DISPLAY_HEIGHT - 1 - point.x),
        };
        DisplayPoint { x, y }
    }

    pub const fn inverse(self) -> Self {
        match self {
            Self::Degrees0 => Self::Degrees0,
            Self::Degrees90 => Self::Degrees270,
            Self::Degrees180 => Self::Degrees180,
            Self::Degrees270 => Self::Degrees90,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowError {
    Empty,
    OutOfBounds {
        x: u16,
        y: u16,
        width: u16,
        height: u16,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DisplayWindow {
    x_start: u16,
    y_start: u16,
    x_end_inclusive: u16,
    y_end_inclusive: u16,
    pixel_count: u32,
}

impl DisplayWindow {
    pub const FULL: Self = Self {
        x_start: 0,
        y_start: 0,
        x_end_inclusive: DISPLAY_WIDTH - 1,
        y_end_inclusive: DISPLAY_HEIGHT - 1,
        pixel_count: DISPLAY_PIXEL_COUNT,
    };

    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Result<Self, WindowError> {
        if width == 0 || height == 0 {
            return Err(WindowError::Empty);
        }
        let x_end_exclusive = x as u32 + width as u32;
        let y_end_exclusive = y as u32 + height as u32;
        if x >= DISPLAY_WIDTH
            || y >= DISPLAY_HEIGHT
            || x_end_exclusive > DISPLAY_WIDTH as u32
            || y_end_exclusive > DISPLAY_HEIGHT as u32
        {
            return Err(WindowError::OutOfBounds {
                x,
                y,
                width,
                height,
            });
        }
        Ok(Self {
            x_start: x,
            y_start: y,
            x_end_inclusive: (x_end_exclusive - 1) as u16,
            y_end_inclusive: (y_end_exclusive - 1) as u16,
            pixel_count: width as u32 * height as u32,
        })
    }

    pub const fn x_start(self) -> u16 {
        self.x_start
    }

    pub const fn y_start(self) -> u16 {
        self.y_start
    }

    pub const fn x_end_inclusive(self) -> u16 {
        self.x_end_inclusive
    }

    pub const fn y_end_inclusive(self) -> u16 {
        self.y_end_inclusive
    }

    pub const fn width(self) -> u16 {
        self.x_end_inclusive - self.x_start + 1
    }

    pub const fn height(self) -> u16 {
        self.y_end_inclusive - self.y_start + 1
    }

    pub const fn pixel_count(self) -> u32 {
        self.pixel_count
    }

    pub const fn program(self) -> WindowProgram {
        WindowProgram {
            column_command: COMMAND_COLUMN_ADDRESS_SET,
            column_data: address_data(self.x_start, self.x_end_inclusive),
            row_command: COMMAND_ROW_ADDRESS_SET,
            row_data: address_data(self.y_start, self.y_end_inclusive),
            memory_write_command: COMMAND_MEMORY_WRITE,
            pixel_count: self.pixel_count,
        }
    }
}

const fn address_data(start: u16, end_inclusive: u16) -> [u8; 4] {
    let [start_high, start_low] = start.to_be_bytes();
    let [end_high, end_low] = end_inclusive.to_be_bytes();
    [start_high, start_low, end_high, end_low]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowProgram {
    column_command: u8,
    column_data: [u8; 4],
    row_command: u8,
    row_data: [u8; 4],
    memory_write_command: u8,
    pixel_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowPhase {
    Command(u8),
    AddressData([u8; 4]),
}

impl WindowProgram {
    pub const fn column_command(self) -> u8 {
        self.column_command
    }

    pub const fn column_data(self) -> [u8; 4] {
        self.column_data
    }

    pub const fn row_command(self) -> u8 {
        self.row_command
    }

    pub const fn row_data(self) -> [u8; 4] {
        self.row_data
    }

    pub const fn memory_write_command(self) -> u8 {
        self.memory_write_command
    }

    pub const fn pixel_count(self) -> u32 {
        self.pixel_count
    }

    pub const fn phases(self) -> [WindowPhase; 5] {
        [
            WindowPhase::Command(self.column_command),
            WindowPhase::AddressData(self.column_data),
            WindowPhase::Command(self.row_command),
            WindowPhase::AddressData(self.row_data),
            WindowPhase::Command(self.memory_write_command),
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb565(u16);

impl Rgb565 {
    pub const BLACK: Self = Self(0x0000);
    pub const BLUE: Self = Self(0x001f);
    pub const GREEN: Self = Self(0x07e0);
    pub const RED: Self = Self(0xf800);
    pub const WHITE: Self = Self(0xffff);

    pub const fn from_raw(value: u16) -> Self {
        Self(value)
    }

    pub const fn from_rgb888(red: u8, green: u8, blue: u8) -> Self {
        Self(((red as u16 & 0xf8) << 8) | ((green as u16 & 0xfc) << 3) | (blue as u16 >> 3))
    }

    pub const fn raw(self) -> u16 {
        self.0
    }

    pub const fn to_msb_first_bytes(self) -> [u8; BYTES_PER_PIXEL] {
        self.0.to_be_bytes()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelStreamError {
    EmptyChunk,
    ChunkTooLarge { maximum: usize, actual: usize },
    OutputLength { expected: usize, actual: usize },
    TooManyPixels { remaining: u32, supplied: u32 },
    Incomplete { remaining: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelStream {
    remaining_pixels: u32,
}

impl PixelStream {
    pub const fn new(window: DisplayWindow) -> Self {
        Self {
            remaining_pixels: window.pixel_count,
        }
    }

    pub const fn remaining_pixels(&self) -> u32 {
        self.remaining_pixels
    }

    pub const fn is_complete(&self) -> bool {
        self.remaining_pixels == 0
    }

    pub fn encode_chunk(
        &mut self,
        pixels: &[Rgb565],
        output: &mut [u8],
    ) -> Result<usize, PixelStreamError> {
        if pixels.is_empty() {
            return Err(PixelStreamError::EmptyChunk);
        }
        if pixels.len() > MAX_PIXEL_CHUNK_PIXELS {
            return Err(PixelStreamError::ChunkTooLarge {
                maximum: MAX_PIXEL_CHUNK_PIXELS,
                actual: pixels.len(),
            });
        }
        let expected_output = pixels.len() * BYTES_PER_PIXEL;
        if output.len() != expected_output {
            return Err(PixelStreamError::OutputLength {
                expected: expected_output,
                actual: output.len(),
            });
        }
        let supplied = pixels.len() as u32;
        if supplied > self.remaining_pixels {
            return Err(PixelStreamError::TooManyPixels {
                remaining: self.remaining_pixels,
                supplied,
            });
        }
        for (pixel, encoded) in pixels.iter().zip(output.chunks_exact_mut(2)) {
            encoded.copy_from_slice(&pixel.to_msb_first_bytes());
        }
        self.remaining_pixels -= supplied;
        Ok(expected_output)
    }

    pub const fn finish(self) -> Result<(), PixelStreamError> {
        if self.remaining_pixels == 0 {
            Ok(())
        } else {
            Err(PixelStreamError::Incomplete {
                remaining: self.remaining_pixels,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_are_nonempty_bounded_and_encode_inclusive_msb_first_addresses() {
        let full = DisplayWindow::FULL;
        assert_eq!(full.width(), DISPLAY_WIDTH);
        assert_eq!(full.height(), DISPLAY_HEIGHT);
        assert_eq!(full.pixel_count(), DISPLAY_PIXEL_COUNT);
        let full_program = full.program();
        assert_eq!(full_program.column_command(), 0x2a);
        assert_eq!(full_program.column_data(), [0x00, 0x00, 0x00, 0xef]);
        assert_eq!(full_program.row_command(), 0x2b);
        assert_eq!(full_program.row_data(), [0x00, 0x00, 0x00, 0xef]);
        assert_eq!(full_program.memory_write_command(), 0x2c);
        assert_eq!(full_program.pixel_count(), 57_600);
        assert_eq!(
            full_program.phases(),
            [
                WindowPhase::Command(0x2a),
                WindowPhase::AddressData([0x00, 0x00, 0x00, 0xef]),
                WindowPhase::Command(0x2b),
                WindowPhase::AddressData([0x00, 0x00, 0x00, 0xef]),
                WindowPhase::Command(0x2c),
            ]
        );

        let window = DisplayWindow::new(10, 20, 3, 4).unwrap();
        assert_eq!(window.x_end_inclusive(), 12);
        assert_eq!(window.y_end_inclusive(), 23);
        assert_eq!(window.pixel_count(), 12);
        assert_eq!(window.program().column_data(), [0x00, 0x0a, 0x00, 0x0c]);
        assert_eq!(window.program().row_data(), [0x00, 0x14, 0x00, 0x17]);
    }

    #[test]
    fn invalid_windows_fail_without_clamping_or_underflow() {
        assert_eq!(DisplayWindow::new(0, 0, 0, 1), Err(WindowError::Empty));
        assert_eq!(DisplayWindow::new(0, 0, 1, 0), Err(WindowError::Empty));
        for invalid in [
            (240, 0, 1, 1),
            (0, 240, 1, 1),
            (239, 0, 2, 1),
            (0, 239, 1, 2),
            (u16::MAX, u16::MAX, u16::MAX, u16::MAX),
        ] {
            assert!(matches!(
                DisplayWindow::new(invalid.0, invalid.1, invalid.2, invalid.3),
                Err(WindowError::OutOfBounds { .. })
            ));
        }
        for x in 0..DISPLAY_WIDTH {
            for y in 0..DISPLAY_HEIGHT {
                assert!(DisplayWindow::new(x, y, 1, 1).is_ok());
            }
            let width_to_edge = DISPLAY_WIDTH - x;
            assert!(DisplayWindow::new(x, 0, width_to_edge, 1).is_ok());
            assert!(matches!(
                DisplayWindow::new(x, 0, width_to_edge + 1, 1),
                Err(WindowError::OutOfBounds { .. })
            ));
        }
        for y in 0..DISPLAY_HEIGHT {
            let height_to_edge = DISPLAY_HEIGHT - y;
            assert!(DisplayWindow::new(0, y, 1, height_to_edge).is_ok());
            assert!(matches!(
                DisplayWindow::new(0, y, 1, height_to_edge + 1),
                Err(WindowError::OutOfBounds { .. })
            ));
        }
    }

    #[test]
    fn rotations_map_every_point_bijectively() {
        let rotations = [
            Rotation::Degrees0,
            Rotation::Degrees90,
            Rotation::Degrees180,
            Rotation::Degrees270,
        ];
        for x in 0..DISPLAY_WIDTH {
            for y in 0..DISPLAY_HEIGHT {
                let point = DisplayPoint::new(x, y).unwrap();
                for rotation in rotations {
                    let mapped = rotation.map(point);
                    assert!(mapped.x() < DISPLAY_WIDTH);
                    assert!(mapped.y() < DISPLAY_HEIGHT);
                    assert_eq!(rotation.inverse().map(mapped), point);
                }
            }
        }
        assert_eq!(
            Rotation::Degrees270.map(DisplayPoint::new(0, 0).unwrap()),
            DisplayPoint::new(0, 239).unwrap()
        );
        assert_eq!(
            Rotation::Degrees90.map(DisplayPoint::new(0, 0).unwrap()),
            DisplayPoint::new(239, 0).unwrap()
        );
        assert_eq!(
            DisplayPoint::new(240, 0),
            Err(PointError::OutOfBounds { x: 240, y: 0 })
        );
    }

    #[test]
    fn rgb565_conversion_and_wire_bytes_are_deterministic() {
        assert_eq!(Rgb565::from_rgb888(0, 0, 0), Rgb565::BLACK);
        assert_eq!(Rgb565::from_rgb888(255, 0, 0), Rgb565::RED);
        assert_eq!(Rgb565::from_rgb888(0, 255, 0), Rgb565::GREEN);
        assert_eq!(Rgb565::from_rgb888(0, 0, 255), Rgb565::BLUE);
        assert_eq!(Rgb565::from_rgb888(255, 255, 255), Rgb565::WHITE);
        assert_eq!(Rgb565::from_rgb888(248, 252, 248), Rgb565::WHITE);
        assert_eq!(Rgb565::from_raw(0x1234).raw(), 0x1234);
        assert_eq!(Rgb565::from_raw(0x1234).to_msb_first_bytes(), [0x12, 0x34]);
    }

    #[test]
    fn pixel_stream_tracks_exact_count_across_bounded_chunks() {
        let window = DisplayWindow::new(0, 0, 2, 2).unwrap();
        let mut stream = PixelStream::new(window);
        let first = [Rgb565::RED, Rgb565::GREEN];
        let mut encoded = [0_u8; 4];
        assert_eq!(stream.encode_chunk(&first, &mut encoded), Ok(4));
        assert_eq!(encoded, [0xf8, 0x00, 0x07, 0xe0]);
        assert_eq!(stream.remaining_pixels(), 2);

        let second = [Rgb565::BLUE, Rgb565::WHITE];
        assert_eq!(stream.encode_chunk(&second, &mut encoded), Ok(4));
        assert_eq!(encoded, [0x00, 0x1f, 0xff, 0xff]);
        assert!(stream.is_complete());
        assert_eq!(stream.finish(), Ok(()));
    }

    #[test]
    fn pixel_stream_rejections_do_not_change_remaining_count() {
        let mut stream = PixelStream::new(DisplayWindow::new(0, 0, 1, 1).unwrap());
        let mut empty_output = [];
        assert_eq!(
            stream.encode_chunk(&[], &mut empty_output),
            Err(PixelStreamError::EmptyChunk)
        );
        let two = [Rgb565::BLACK; 2];
        let mut four_bytes = [0_u8; 4];
        assert_eq!(
            stream.encode_chunk(&two, &mut four_bytes),
            Err(PixelStreamError::TooManyPixels {
                remaining: 1,
                supplied: 2,
            })
        );
        let one = [Rgb565::BLACK];
        assert_eq!(
            stream.encode_chunk(&one, &mut four_bytes),
            Err(PixelStreamError::OutputLength {
                expected: 2,
                actual: 4,
            })
        );
        assert_eq!(stream.remaining_pixels(), 1);
        assert_eq!(
            stream.finish(),
            Err(PixelStreamError::Incomplete { remaining: 1 })
        );

        let too_large = [Rgb565::BLACK; MAX_PIXEL_CHUNK_PIXELS + 1];
        let mut too_large_output = [0_u8; (MAX_PIXEL_CHUNK_PIXELS + 1) * 2];
        let mut full_stream = PixelStream::new(DisplayWindow::FULL);
        assert_eq!(
            full_stream.encode_chunk(&too_large, &mut too_large_output),
            Err(PixelStreamError::ChunkTooLarge {
                maximum: MAX_PIXEL_CHUNK_PIXELS,
                actual: MAX_PIXEL_CHUNK_PIXELS + 1,
            })
        );
        assert_eq!(full_stream.remaining_pixels(), DISPLAY_PIXEL_COUNT);
    }

    #[test]
    fn full_screen_stream_completes_in_exact_source_sized_rows() {
        let row = [Rgb565::from_raw(0xa55a); MAX_PIXEL_CHUNK_PIXELS];
        let mut output = [0_u8; MAX_PIXEL_CHUNK_PIXELS * BYTES_PER_PIXEL];
        let mut stream = PixelStream::new(DisplayWindow::FULL);
        for _ in 0..DISPLAY_HEIGHT {
            assert_eq!(stream.encode_chunk(&row, &mut output), Ok(output.len()));
            assert_eq!(output[0..4], [0xa5, 0x5a, 0xa5, 0x5a]);
        }
        assert!(stream.is_complete());
        assert_eq!(stream.finish(), Ok(()));
    }
}
