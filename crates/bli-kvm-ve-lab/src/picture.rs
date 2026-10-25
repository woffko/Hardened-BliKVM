//! Test pictures. Every frame follows from the sequence's name, the size and
//! the frame's number alone, with integer arithmetic only, so the board and
//! the host make the same bytes and no screen of the operator is recorded.

use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sequence {
    /// A page of text-like marks that scrolls by lines.
    Scroll,
    /// A window dragged over a still desktop.
    Drag,
    /// The whole picture moves: the hardest case.
    Motion,
    /// A still desktop with a small pointer that moves.
    Pointer,
    /// A page that moves by the same vector in every frame: what a search
    /// for motion is measured with. Not one of the plan's sequences.
    Moved { page: Page, right: i32, down: i32 },
}

impl Sequence {
    /// The plan's sequences.
    pub const ALL: [Sequence; 4] = [Self::Scroll, Self::Drag, Self::Motion, Self::Pointer];

    /// `scroll` and the like; a moved page is `PAGE.RIGHT.DOWN`, as in
    /// `text.0.-20`.
    pub fn name(self) -> String {
        match self {
            Self::Scroll => "scroll".into(),
            Self::Drag => "drag".into(),
            Self::Motion => "motion".into(),
            Self::Pointer => "pointer".into(),
            Self::Moved { page, right, down } => format!("{}.{right}.{down}", page.name()),
        }
    }
}

impl FromStr for Sequence {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if let Some(sequence) = Self::ALL
            .into_iter()
            .find(|sequence| sequence.name() == value)
        {
            return Ok(sequence);
        }
        let mut parts = value.split('.');
        let page = match parts.next() {
            Some("text") => Some(Page::Text),
            Some("grain") => Some(Page::Grain),
            _ => None,
        };
        let mut pixels = || {
            parts
                .next()
                .and_then(|part| part.parse::<i32>().ok())
                .filter(|pixels| pixels.abs() <= 256)
        };
        match (page, pixels(), pixels(), parts.next()) {
            (Some(page), Some(right), Some(down), None) => Ok(Self::Moved { page, right, down }),
            _ => Err(format!("no sequence is called {value}")),
        }
    }
}

/// An NV12 picture of whole macroblocks: the luma plane, then the plane of
/// interleaved Cb and Cr at half the size. The visible picture is its top
/// left part; the rest repeats the picture's last column and row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub coded_width: usize,
    pub coded_height: usize,
    pub luma: Vec<u8>,
    pub chroma: Vec<u8>,
}

impl Picture {
    /// `width` and `height` are even.
    pub fn new(width: usize, height: usize) -> Self {
        assert!(width >= 16 && height >= 16 && width % 2 == 0 && height % 2 == 0);
        let coded_width = width.div_ceil(16) * 16;
        let coded_height = height.div_ceil(16) * 16;
        Self {
            width,
            height,
            coded_width,
            coded_height,
            luma: vec![0; coded_width * coded_height],
            chroma: vec![0; coded_width * coded_height / 2],
        }
    }

    /// The bytes of both planes.
    pub fn bytes(&self) -> usize {
        self.luma.len() + self.chroma.len()
    }
}

fn mix(value: u32) -> u32 {
    let mut value = value.wrapping_mul(0x9e37_79b1);
    value ^= value >> 15;
    value = value.wrapping_mul(0x85eb_ca77);
    value ^= value >> 13;
    value
}

/// A wave between 0 and `top` that turns at both ends.
fn bounce(position: u32, top: u32) -> u32 {
    if top == 0 {
        return 0;
    }
    let phase = position % (2 * top);
    if phase <= top { phase } else { 2 * top - phase }
}

/// One pixel: luma, Cb, Cr.
type Pixel = (u8, u8, u8);

const PAPER: Pixel = (235, 128, 128);
const INK: u8 = 28;
/// A line of text: 16 rows of glyphs and 4 of space.
const LINE: u32 = 20;
/// A glyph's cell: 5 columns of marks and 3 of space.
const CELL: u32 = 8;

/// A page of marks that look like text to an encoder: glyphs of 5 by 7
/// points, twice as tall as wide, words and gaps, and a tinted line now and
/// then.
fn text(x: u32, y: u32) -> Pixel {
    let line = y / LINE;
    let row = y % LINE;
    let tinted = line % 7 == 3;
    let paper = if tinted { (222, 112, 140) } else { PAPER };
    if !(2..16).contains(&row) {
        return paper;
    }
    let column = x / CELL;
    let dot = x % CELL;
    // A margin, and a ragged right edge.
    let length = 60 + mix(line.wrapping_mul(77)) % 140;
    if column < 3 || column > length || dot >= 5 {
        return paper;
    }
    let glyph = mix(line.wrapping_mul(8191).wrapping_add(column));
    // One cell in six is a space.
    if glyph % 6 == 0 {
        return paper;
    }
    let bit = dot + 5 * ((row - 2) / 2);
    let marks = mix(glyph ^ 0x5bd1);
    if (marks >> (bit % 32)) & 1 == 1 {
        let ink = if glyph % 23 == 0 {
            (60, 190, 100)
        } else {
            (INK, 128, 128)
        };
        return ink;
    }
    paper
}

/// A desktop: a soft gradient, a grid of icons, a bar at the bottom.
fn desktop(x: u32, y: u32, width: u32, height: u32) -> Pixel {
    if y + 40 >= height {
        // The bar, with a few buttons.
        let button = x / 64;
        if x % 64 >= 8 && x % 64 < 56 && y + 34 >= height && y + 6 < height && button < 6 {
            return (150 + (mix(button) % 60) as u8, 120, 132);
        }
        return (44, 132, 124);
    }
    let cell_x = x / 96;
    let cell_y = y / 96;
    if cell_x < 2 && x % 96 >= 24 && x % 96 < 72 && y % 96 >= 16 && y % 96 < 64 {
        let icon = mix(cell_x * 31 + cell_y);
        let inner = ((x % 96 - 24) / 8 + (y % 96 - 16) / 8) % 2;
        return (
            90 + (icon % 120) as u8 + (inner * 20) as u8,
            80 + (icon >> 8) as u8 % 96,
            80 + (icon >> 16) as u8 % 96,
        );
    }
    let shade = 70 + (y * 50 / height.max(1)) + (x * 16 / width.max(1));
    (shade as u8, 140, 118)
}

/// A window at `(left, top)` with a title bar and a page of text.
fn window(x: u32, y: u32, left: u32, top: u32, width: u32, height: u32) -> Option<Pixel> {
    if x < left || y < top || x >= left + width || y >= top + height {
        return None;
    }
    let (inner_x, inner_y) = (x - left, y - top);
    if inner_x < 2 || inner_y < 2 || inner_x + 2 >= width || inner_y + 2 >= height {
        return Some((20, 128, 128));
    }
    if inner_y < 26 {
        let stripe = (inner_x / 4) % 2;
        return Some((96 + stripe as u8 * 6, 168, 108));
    }
    Some(text(inner_x, inner_y - 26))
}

/// The pointer's shape: an arrow of 12 by 19 with a dark edge.
fn pointer(x: u32, y: u32, at_x: u32, at_y: u32) -> Option<Pixel> {
    if x < at_x || y < at_y {
        return None;
    }
    let (dx, dy) = (x - at_x, y - at_y);
    if dy >= 19 || dx >= 12 {
        return None;
    }
    let reach = if dy < 12 {
        dy
    } else {
        17_u32.saturating_sub(dy)
    };
    if dx > reach {
        return None;
    }
    let edge = dx == 0 || dx == reach || dy == 18;
    Some(if edge {
        (16, 128, 128)
    } else {
        (240, 128, 128)
    })
}

fn pixel(sequence: Sequence, x: u32, y: u32, width: u32, height: u32, frame: u32) -> Pixel {
    match sequence {
        Sequence::Scroll => {
            // Two frames of three move the page by a line.
            let offset = LINE * (frame * 2 / 3);
            text(x, y + offset)
        }
        Sequence::Drag => {
            let (window_width, window_height) = (width / 2, height / 2);
            let left = bounce(frame * 7, width - window_width);
            let top = bounce(frame * 5, (height - window_height).saturating_sub(40));
            window(x, y, left, top, window_width, window_height)
                .unwrap_or_else(|| desktop(x, y, width, height))
        }
        Sequence::Motion => {
            // Two textures that slide over each other, and blocks that
            // change with every second frame.
            let slide_x = x + 5 * frame;
            let slide_y = y + 3 * frame;
            let wave = bounce(slide_x * 2, 255) / 3 + bounce(slide_y * 3, 255) / 3;
            let block = mix((x / 8) + (y / 8) * 4099 + (frame / 2) * 7919) % 64;
            let fine = ((slide_x ^ slide_y) & 15) * 2;
            (
                (16 + wave + block + fine).min(235) as u8,
                (64 + bounce(x + 2 * frame, 127)) as u8,
                (64 + bounce(y + frame, 127)) as u8,
            )
        }
        Sequence::Pointer => {
            let at_x = bounce(frame * 9, width.saturating_sub(12));
            let at_y = bounce(frame * 4, height.saturating_sub(19));
            pointer(x, y, at_x, at_y)
                .or_else(|| window(x, y, width / 5, height / 6, width / 2, height / 2))
                .unwrap_or_else(|| desktop(x, y, width, height))
        }
        Sequence::Moved { .. } => unreachable!("a moved page is drawn by render_moved"),
    }
}

/// Draws frame `frame` of `sequence` into `picture`.
pub fn render(sequence: Sequence, frame: u32, picture: &mut Picture) {
    if let Sequence::Moved { page, right, down } = sequence {
        let frame = frame.min(1 << 12) as i32;
        return render_moved(page, right * frame, down * frame, picture);
    }
    let size = (picture.width as u32, picture.height as u32);
    draw(picture, |x, y| pixel(sequence, x, y, size.0, size.1, frame));
}

/// What a search for motion is measured with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// The page of text: what a screen shows, with its line and its cell
    /// repeating, so that a wrong place can look half right.
    Text,
    /// Blocks of four by four pixels of unrelated greys: nothing repeats,
    /// and only the true place matches.
    Grain,
}

impl Page {
    pub fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Grain => "grain",
        }
    }
}

fn grain(x: u32, y: u32) -> Pixel {
    let block = mix((x / 4).wrapping_mul(73_856_093) ^ (y / 4).wrapping_mul(19_349_663));
    (40 + (block % 160) as u8, 128, 128)
}

/// Draws `page` moved by `(right, down)` pixels, one vector at a time. What
/// moves into the text from the left is paper.
pub fn render_moved(page: Page, right: i32, down: i32, picture: &mut Picture) {
    // Far enough into the page for any move.
    const FAR: i64 = 1 << 20;
    draw(picture, |x, y| {
        let from_y = (FAR + y as i64 - down as i64) as u32;
        match page {
            Page::Text if (x as i64) < right as i64 => PAPER,
            Page::Text => text((x as i64 - right as i64) as u32, from_y),
            Page::Grain => grain((FAR + x as i64 - right as i64) as u32, from_y),
        }
    });
}

/// Fills `picture` from `pixel`. Every pixel follows from its own place
/// alone, so the rows are shared among the processors.
fn draw(picture: &mut Picture, pixel: impl Fn(u32, u32) -> Pixel + Copy + Send) {
    let (width, height) = (picture.width, picture.height);
    let stride = picture.coded_width;
    let workers = std::thread::available_parallelism().map_or(1, |count| count.get().min(8));
    let luma_rows = height.div_ceil(workers).max(1);
    let chroma_rows = (height / 2).div_ceil(workers).max(1);
    std::thread::scope(|scope| {
        let (luma, _) = picture.luma.split_at_mut(height * stride);
        for (part, rows) in luma.chunks_mut(luma_rows * stride).enumerate() {
            scope.spawn(move || {
                for (index, row) in rows.chunks_exact_mut(stride).enumerate() {
                    let y = (part * luma_rows + index) as u32;
                    for (x, value) in row.iter_mut().enumerate().take(width) {
                        *value = pixel(x as u32, y).0;
                    }
                    let last = row[width - 1];
                    row[width..].fill(last);
                }
            });
        }
        // Chroma: the pixel at the top left of every two by two.
        let (chroma, _) = picture.chroma.split_at_mut((height / 2) * stride);
        for (part, rows) in chroma.chunks_mut(chroma_rows * stride).enumerate() {
            scope.spawn(move || {
                for (index, row) in rows.chunks_exact_mut(stride).enumerate() {
                    let y = (part * chroma_rows + index) as u32;
                    for x in 0..width / 2 {
                        let (_, cb, cr) = pixel(2 * x as u32, 2 * y);
                        row[2 * x] = cb;
                        row[2 * x + 1] = cr;
                    }
                    let (cb, cr) = (row[width - 2], row[width - 1]);
                    for pair in row[width..].chunks_exact_mut(2) {
                        pair[0] = cb;
                        pair[1] = cr;
                    }
                }
            });
        }
    });
    for y in height..picture.coded_height {
        let (above, below) = picture.luma.split_at_mut(y * stride);
        below[..stride].copy_from_slice(&above[(height - 1) * stride..height * stride]);
    }
    for y in height / 2..picture.coded_height / 2 {
        let (above, below) = picture.chroma.split_at_mut(y * stride);
        below[..stride].copy_from_slice(&above[(height / 2 - 1) * stride..(height / 2) * stride]);
    }
}

/// Which of `ring` pictures frame `frame` shows when they are played forth
/// and back: 0 up to the last and down again, so that no frame jumps from
/// the last picture to the first.
pub fn ring_index(frame: u32, ring: u32) -> usize {
    let period = 2 * (ring.max(2) - 1);
    let phase = frame % period;
    (if phase < ring { phase } else { period - phase }) as usize
}

/// Which picture of a sequence frame `frame` shows when the sequence moves
/// for `moving` frames and then rests for `resting`, again and again: what
/// a hand on a wheel does.
pub fn burst_index(frame: u32, moving: u32, resting: u32) -> u32 {
    let cycle = (moving + resting).max(1);
    (frame / cycle) * moving + (frame % cycle).min(moving)
}

/// `MOVING:RESTING` in frames, as the `burst=` option of the commands has it.
pub fn parse_burst(value: &str) -> Result<(u32, u32), String> {
    let part = |text: &str| {
        text.parse::<u32>()
            .ok()
            .filter(|part| (1..=10_000).contains(part))
    };
    value
        .split_once(':')
        .and_then(|(moving, resting)| Some((part(moving)?, part(resting)?)))
        .ok_or_else(|| format!("burst=MOVING:RESTING in frames, not {value}"))
}

/// `1920x1080` and the like.
pub fn parse_size(value: &str) -> Result<(usize, usize), String> {
    let (width, height) = value
        .split_once('x')
        .ok_or_else(|| format!("a size is WIDTHxHEIGHT, not {value}"))?;
    let width: usize = width.parse().map_err(|_| format!("no width in {value}"))?;
    let height: usize = height
        .parse()
        .map_err(|_| format!("no height in {value}"))?;
    if !(16..=4096).contains(&width) || !(16..=4096).contains(&height) {
        return Err(format!("{value} is not a size between 16 and 4096"));
    }
    if width % 2 != 0 || height % 2 != 0 {
        return Err(format!("{value} is not an even size"));
    }
    Ok((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FNV-1a over both planes: a picture's fingerprint.
    fn fingerprint(picture: &Picture) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for &byte in picture.luma.iter().chain(&picture.chroma) {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    #[test]
    fn a_burst_moves_and_rests() {
        let shown: Vec<u32> = (0..12).map(|frame| burst_index(frame, 2, 3)).collect();
        assert_eq!(shown, [0, 1, 2, 2, 2, 2, 3, 4, 4, 4, 4, 5]);
        assert_eq!(parse_burst("30:60"), Ok((30, 60)));
        assert!(parse_burst("30").is_err());
        assert!(parse_burst("0:5").is_err());
    }

    #[test]
    fn a_ring_plays_forth_and_back() {
        let shown: Vec<usize> = (0..10).map(|frame| ring_index(frame, 4)).collect();
        assert_eq!(shown, [0, 1, 2, 3, 2, 1, 0, 1, 2, 3]);
        // Neighbours are neighbours everywhere.
        for frame in 0..500 {
            let step = ring_index(frame, 30).abs_diff(ring_index(frame + 1, 30));
            assert_eq!(step, 1);
        }
        assert!((0..500).all(|frame| ring_index(frame, 30) < 30));
    }

    #[test]
    fn a_moved_page_is_a_sequence_by_its_name() {
        let sequence: Sequence = "text.0.-20".parse().unwrap();
        assert_eq!(
            sequence,
            Sequence::Moved {
                page: Page::Text,
                right: 0,
                down: -20
            }
        );
        assert_eq!(sequence.name(), "text.0.-20");
        assert_eq!(
            "grain.16.0".parse::<Sequence>().unwrap().name(),
            "grain.16.0"
        );
        for wrong in [
            "text",
            "text.0",
            "text.0.1.2",
            "paper.0.1",
            "text.x.1",
            "text.0.999",
        ] {
            assert!(wrong.parse::<Sequence>().is_err(), "{wrong}");
        }
        for name in ["scroll", "drag", "motion", "pointer"] {
            assert_eq!(name.parse::<Sequence>().unwrap().name(), name);
        }
        // Frame 3 is the page moved three times.
        let (mut by_name, mut by_hand) = (Picture::new(320, 240), Picture::new(320, 240));
        render(sequence, 3, &mut by_name);
        render_moved(Page::Text, 0, -60, &mut by_hand);
        assert_eq!(by_name, by_hand);
    }

    #[test]
    fn a_moved_page_is_the_page_moved() {
        for page in [Page::Text, Page::Grain] {
            a_moved_page(page);
        }
        // No block of the grain is found again nearby.
        let mut grain = Picture::new(320, 240);
        render_moved(Page::Grain, 0, 0, &mut grain);
        let block = |x: usize, y: usize| -> Vec<u8> {
            (0..16)
                .flat_map(|row| grain.luma[(y + row) * 320 + x..][..16].to_vec())
                .collect()
        };
        let here = block(160, 112);
        for y in (48..176).step_by(4) {
            for x in (96..224).step_by(4) {
                assert!((x, y) == (160, 112) || block(x, y) != here);
            }
        }
    }

    fn a_moved_page(page: Page) {
        let mut still = Picture::new(320, 240);
        render_moved(page, 0, 0, &mut still);
        // The page is not blank.
        let dark = still.luma.iter().filter(|&&luma| luma < 100).count();
        assert!(dark > 5000, "{page:?}: {dark}");
        for (right, down) in [(0, 20), (0, -20), (7, 0), (-7, 0), (24, -36), (-48, 16)] {
            let mut moved = Picture::new(320, 240);
            render_moved(page, right, down, &mut moved);
            let (mut same, mut differ) = (0, 0);
            for y in 0..240i32 {
                for x in 0..320i32 {
                    let (from_x, from_y) = (x - right, y - down);
                    if !(0..320).contains(&from_x) || !(0..240).contains(&from_y) {
                        continue;
                    }
                    let here = moved.luma[y as usize * moved.coded_width + x as usize];
                    let there = still.luma[from_y as usize * still.coded_width + from_x as usize];
                    if here == there {
                        same += 1
                    } else {
                        differ += 1
                    }
                }
            }
            assert_eq!(differ, 0, "{right},{down}");
            assert!(same > 320 * 240 / 2);
        }
    }

    #[test]
    fn a_frame_is_the_same_whenever_it_is_drawn() {
        for sequence in Sequence::ALL {
            let mut first = Picture::new(320, 240);
            let mut second = Picture::new(320, 240);
            render(sequence, 17, &mut first);
            // Another frame in between leaves nothing behind.
            render(sequence, 3, &mut second);
            render(sequence, 17, &mut second);
            assert_eq!(first, second, "{}", sequence.name());
        }
    }

    #[test]
    fn the_pictures_are_pinned() {
        // The board and the host must draw the same bytes, today and after
        // any change here: a change of these numbers is a change of every
        // earlier measurement's input.
        let expected: [(Sequence, u64); 4] = [
            (Sequence::Scroll, 0x4acc_232b_6460_a615),
            (Sequence::Drag, 0x5728_d872_ddca_b506),
            (Sequence::Motion, 0x070e_f736_b0c4_6407),
            (Sequence::Pointer, 0x869c_9497_5107_3bfd),
        ];
        let mut found = Vec::new();
        for (sequence, _) in expected {
            let mut picture = Picture::new(320, 240);
            render(sequence, 5, &mut picture);
            found.push((sequence, fingerprint(&picture)));
        }
        assert_eq!(found, expected, "{found:#x?}");
    }

    #[test]
    fn sequences_move_as_their_names_say() {
        let changed = |sequence: Sequence, from: u32, to: u32| {
            let mut first = Picture::new(1280, 720);
            let mut second = Picture::new(1280, 720);
            render(sequence, from, &mut first);
            render(sequence, to, &mut second);
            first
                .luma
                .iter()
                .zip(&second.luma)
                .filter(|(left, right)| left != right)
                .count()
        };
        let pixels = 1280 * 720;
        // The page moves in two frames of three.
        assert_eq!(changed(Sequence::Scroll, 0, 1), 0);
        assert!(changed(Sequence::Scroll, 1, 2) > pixels / 20);
        // The window is a quarter of the picture; what changes is less.
        let drag = changed(Sequence::Drag, 10, 11);
        assert!(drag > 1000 && drag < pixels / 3, "{drag}");
        // Everything moves.
        assert!(changed(Sequence::Motion, 10, 11) > pixels / 2);
        // A pointer is a few hundred pixels.
        let pointer = changed(Sequence::Pointer, 10, 11);
        assert!(pointer > 50 && pointer < 2000, "{pointer}");
    }

    #[test]
    fn the_room_beyond_the_picture_repeats_its_edge() {
        let mut picture = Picture::new(1366, 768);
        assert_eq!((picture.coded_width, picture.coded_height), (1376, 768));
        render(Sequence::Motion, 2, &mut picture);
        for y in [0, 100, 767] {
            let row = &picture.luma[y * 1376..(y + 1) * 1376];
            assert!(row[1366..].iter().all(|value| *value == row[1365]));
        }
        let mut tall = Picture::new(1920, 1080);
        assert_eq!(tall.coded_height, 1088);
        render(Sequence::Drag, 2, &mut tall);
        let last = tall.luma[1079 * 1920..1080 * 1920].to_vec();
        for y in 1080..1088 {
            assert_eq!(&tall.luma[y * 1920..(y + 1) * 1920], &last[..]);
        }
        let last = tall.chroma[539 * 1920..540 * 1920].to_vec();
        for y in 540..544 {
            assert_eq!(&tall.chroma[y * 1920..(y + 1) * 1920], &last[..]);
        }
    }

    #[test]
    fn sizes_are_parsed_and_refused() {
        assert_eq!(parse_size("1920x1080"), Ok((1920, 1080)));
        assert_eq!(parse_size("1366x768"), Ok((1366, 768)));
        for bad in ["1920", "x", "0x0", "1921x1080", "8x8", "5000x100"] {
            assert!(parse_size(bad).is_err(), "{bad}");
        }
        assert_eq!("drag".parse::<Sequence>(), Ok(Sequence::Drag));
        assert!("other".parse::<Sequence>().is_err());
    }
}
