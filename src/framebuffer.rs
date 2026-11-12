use std::io::{self, Write};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::color::Rgb;

#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
pub struct CellStyle {
    pub foreground: Option<Rgb>,
    pub background: Option<Rgb>,
}

impl CellStyle {
    pub const fn new(foreground: Option<Rgb>, background: Option<Rgb>) -> Self {
        Self {
            foreground,
            background,
        }
    }

    pub const fn foreground(color: Rgb) -> Self {
        Self::new(Some(color), None)
    }

    pub const fn background(color: Rgb) -> Self {
        Self::new(None, Some(color))
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct FrameCell {
    pub symbol: String,
    pub style: CellStyle,
    pub continuation: bool,
}

impl Clone for FrameCell {
    fn clone(&self) -> Self {
        Self {
            symbol: self.symbol.clone(),
            style: self.style,
            continuation: self.continuation,
        }
    }

    fn clone_from(&mut self, source: &Self) {
        self.symbol.clone_from(&source.symbol);
        self.style = source.style;
        self.continuation = source.continuation;
    }
}

impl Default for FrameCell {
    fn default() -> Self {
        Self {
            symbol: " ".to_owned(),
            style: CellStyle::default(),
            continuation: false,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
pub struct PresentStats {
    pub changed_cells: usize,
    pub cursor_moves: usize,
}

#[derive(Debug, Default)]
pub struct FrameBuffer {
    width: u16,
    height: u16,
    previous: Vec<FrameCell>,
    current: Vec<FrameCell>,
    valid: bool,
}

impl FrameBuffer {
    pub fn new(width: u16, height: u16) -> Self {
        let mut buffer = Self::default();
        buffer.resize(width, height);
        buffer
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        if self.width == width && self.height == height {
            return;
        }
        self.width = width;
        self.height = height;
        let length = usize::from(width) * usize::from(height);
        self.previous = vec![FrameCell::default(); length];
        self.current = vec![FrameCell::default(); length];
        self.valid = false;
    }

    pub fn begin_frame(&mut self) {
        for cell in &mut self.current {
            cell.symbol.clear();
            cell.symbol.push(' ');
            cell.style = CellStyle::default();
            cell.continuation = false;
        }
    }

    pub fn invalidate(&mut self) {
        self.valid = false;
    }

    pub fn put(&mut self, y: i32, x: i32, symbol: char) -> bool {
        self.put_styled(y, x, symbol, CellStyle::default())
    }

    pub fn put_styled(&mut self, y: i32, x: i32, symbol: char, style: CellStyle) -> bool {
        let mut encoded = [0; 4];
        self.put_grapheme_styled(y, x, symbol.encode_utf8(&mut encoded), style)
    }

    pub fn put_grapheme_styled(
        &mut self,
        y: i32,
        x: i32,
        grapheme: &str,
        style: CellStyle,
    ) -> bool {
        let Some(index) = self.index(y, x) else {
            return false;
        };
        // Cell text is data, never a terminal escape sequence. This also
        // protects window titles, which do not pass through application text
        // layout, from moving the cursor or injecting OSC commands.
        if grapheme.is_empty() || grapheme.chars().any(char::is_control) {
            return false;
        }
        let symbol_width = grapheme_width(grapheme).max(1);
        if symbol_width > usize::from(self.width) - x as usize {
            return false;
        }
        // An overlay may cover either half of a wide glyph. Erase its full
        // footprint before writing, including the half outside the overlay.
        for offset in 0..symbol_width {
            self.erase_overlapping_glyph(index + offset);
        }
        let cell = &mut self.current[index];
        cell.symbol.clear();
        cell.symbol.push_str(grapheme);
        cell.style = style;
        cell.continuation = false;
        for cell in &mut self.current[index + 1..index + symbol_width] {
            cell.symbol.clear();
            cell.style = style;
            cell.continuation = true;
        }
        true
    }

    fn erase_overlapping_glyph(&mut self, index: usize) {
        let width = usize::from(self.width);
        let row_start = index / width * width;
        let row_end = row_start + width;
        let mut start = index;
        while start > row_start && self.current[start].continuation {
            start -= 1;
        }
        let mut end = start + 1;
        while end < row_end && self.current[end].continuation {
            end += 1;
        }
        if end == start + 1 {
            return;
        }
        for cell in &mut self.current[start..end] {
            cell.symbol.clear();
            cell.symbol.push(' ');
            cell.continuation = false;
        }
    }

    pub fn put_text(&mut self, y: i32, x: i32, text: impl AsRef<str>) {
        self.put_text_styled(y, x, text, CellStyle::default());
    }

    pub fn put_text_styled(&mut self, y: i32, x: i32, text: impl AsRef<str>, style: CellStyle) {
        let mut offset = 0;
        for grapheme in UnicodeSegmentation::graphemes(text.as_ref(), true) {
            if grapheme.chars().any(char::is_control) {
                continue;
            }
            self.put_grapheme_styled(y, x + offset, grapheme, style);
            offset += grapheme_width(grapheme).max(1) as i32;
        }
    }

    pub fn get(&self, y: i32, x: i32) -> Option<&FrameCell> {
        self.index(y, x).map(|index| &self.current[index])
    }

    pub fn present(&mut self, writer: &mut impl Write) -> io::Result<PresentStats> {
        let mut stats = PresentStats::default();
        let width = usize::from(self.width);
        let mut active_style = CellStyle::default();

        for y in 0..usize::from(self.height) {
            let mut x = 0;
            while x < width {
                while x < width && !self.changed(y * width + x) {
                    x += 1;
                }
                if x == width {
                    break;
                }

                let run_start = x;
                while x < width && self.changed(y * width + x) {
                    x += 1;
                }

                write!(writer, "\x1b[{};{}H", y + 1, run_start + 1)?;
                stats.cursor_moves += 1;
                stats.changed_cells += x - run_start;
                for column in run_start..x {
                    let cell = &self.current[y * width + column];
                    if cell.continuation {
                        continue;
                    }
                    if cell.style != active_style {
                        write_style(writer, cell.style, active_style != CellStyle::default())?;
                        active_style = cell.style;
                    }
                    let symbol = &cell.symbol;
                    write!(writer, "{symbol}")?;
                }
            }
        }

        if active_style != CellStyle::default() {
            writer.write_all(b"\x1b[0m")?;
        }

        writer.flush()?;
        self.previous.clone_from(&self.current);
        self.valid = true;
        Ok(stats)
    }

    /// Repaints each changed terminal row from a known cursor origin.
    ///
    /// Terminals occasionally disagree with Unicode tables about the width of
    /// emoji and composed graphemes. Clearing an affected row before drawing it
    /// prevents that disagreement from leaving text fragments after scrolling,
    /// while unchanged rows still produce no output.
    pub fn present_clean_rows(&mut self, writer: &mut impl Write) -> io::Result<PresentStats> {
        let mut stats = PresentStats::default();
        let width = usize::from(self.width);
        let default = FrameCell::default();

        for y in 0..usize::from(self.height) {
            let start = y * width;
            let end = start + width;
            if self.valid
                && self.previous[start..end]
                    .iter()
                    .zip(&self.current[start..end])
                    .all(|(previous, current)| previous == current)
            {
                continue;
            }

            write!(writer, "\x1b[0m\x1b[{};1H\x1b[2K", y + 1)?;
            stats.cursor_moves += 1;
            stats.changed_cells += width;
            let last = self.current[start..end]
                .iter()
                .rposition(|cell| cell != &default);
            let Some(last) = last else {
                continue;
            };
            // A Nerd Font glyph may occupy one terminal cell even though the
            // layout reserves two. Paint continuation backgrounds first, then
            // draw the glyph over them. This keeps the cell immediately to its
            // right in the panel color with either terminal width convention.
            let mut prefilled = false;
            for (column, cell) in self.current[start..start + last + 1].iter().enumerate() {
                if cell.continuation
                    && let Some(background) = cell.style.background
                {
                    write!(writer, "\x1b[0m\x1b[{};{}H", y + 1, column + 1)?;
                    write_style(writer, CellStyle::background(background), false)?;
                    writer.write_all(b" ")?;
                    stats.cursor_moves += 1;
                    prefilled = true;
                }
            }
            if prefilled {
                writer.write_all(b"\x1b[0m")?;
            }
            let mut active_style = CellStyle::default();
            let mut cursor_known = !prefilled;
            for (column, cell) in self.current[start..start + last + 1].iter().enumerate() {
                if cell.continuation {
                    continue;
                }
                if !cursor_known || !cell.symbol.is_ascii() {
                    write!(writer, "\x1b[{};{}H", y + 1, column + 1)?;
                    stats.cursor_moves += 1;
                }
                if cell.style != active_style {
                    write_style(writer, cell.style, active_style != CellStyle::default())?;
                    active_style = cell.style;
                }
                write!(writer, "{}", cell.symbol)?;
                cursor_known = cell.symbol.is_ascii();
            }
            if active_style != CellStyle::default() {
                writer.write_all(b"\x1b[0m")?;
            }
        }

        writer.flush()?;
        self.previous.clone_from(&self.current);
        self.valid = true;
        Ok(stats)
    }

    fn changed(&self, index: usize) -> bool {
        !self.valid || self.previous[index] != self.current[index]
    }

    fn index(&self, y: i32, x: i32) -> Option<usize> {
        if y < 0 || x < 0 || y >= i32::from(self.height) || x >= i32::from(self.width) {
            return None;
        }
        Some(y as usize * usize::from(self.width) + x as usize)
    }
}

pub fn grapheme_width(grapheme: &str) -> usize {
    if grapheme
        .chars()
        .any(|c| matches!(c as u32, 0xe000..=0xf8ff | 0xf0000..=0xffffd | 0x100000..=0x10fffd))
    {
        2
    } else {
        UnicodeWidthStr::width(grapheme)
    }
}

fn write_style(writer: &mut impl Write, style: CellStyle, reset_previous: bool) -> io::Result<()> {
    if reset_previous {
        writer.write_all(b"\x1b[0m")?;
    }
    if let Some(color) = style.foreground {
        write!(writer, "\x1b[38;2;{};{};{}m", color.r, color.g, color.b)?;
    }
    if let Some(color) = style.background {
        write!(writer, "\x1b[48;2;{};{};{}m", color.r, color.g, color.b)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_frame_writes_nothing() {
        let mut frame = FrameBuffer::new(4, 2);
        frame.begin_frame();
        frame.put_text(0, 1, "ab");
        frame.present(&mut Vec::new()).unwrap();

        frame.begin_frame();
        frame.put_text(0, 1, "ab");
        let mut output = Vec::new();
        let stats = frame.present(&mut output).unwrap();

        assert!(output.is_empty());
        assert_eq!(stats, PresentStats::default());
    }

    #[test]
    fn contiguous_changes_share_one_cursor_move() {
        let mut frame = FrameBuffer::new(6, 1);
        frame.begin_frame();
        frame.present(&mut Vec::new()).unwrap();

        frame.begin_frame();
        frame.put_text(0, 2, "abc");
        let mut output = Vec::new();
        let stats = frame.present(&mut output).unwrap();

        assert_eq!(String::from_utf8(output).unwrap(), "\x1b[1;3Habc");
        assert_eq!(stats.changed_cells, 3);
        assert_eq!(stats.cursor_moves, 1);
    }

    #[test]
    fn separated_changes_use_the_minimum_changed_only_runs() {
        let mut frame = FrameBuffer::new(5, 1);
        frame.begin_frame();
        frame.present(&mut Vec::new()).unwrap();

        frame.begin_frame();
        frame.put(0, 0, 'a');
        frame.put(0, 4, 'b');
        let mut output = Vec::new();
        let stats = frame.present(&mut output).unwrap();

        assert_eq!(String::from_utf8(output).unwrap(), "\x1b[1;1Ha\x1b[1;5Hb");
        assert_eq!(stats.changed_cells, 2);
        assert_eq!(stats.cursor_moves, 2);
    }

    #[test]
    fn removed_content_is_erased_with_spaces() {
        let mut frame = FrameBuffer::new(3, 1);
        frame.begin_frame();
        frame.put_text(0, 0, "abc");
        frame.present(&mut Vec::new()).unwrap();

        frame.begin_frame();
        let mut output = Vec::new();
        let stats = frame.present(&mut output).unwrap();

        assert_eq!(String::from_utf8(output).unwrap(), "\x1b[1;1H   ");
        assert_eq!(stats.changed_cells, 3);
        assert_eq!(stats.cursor_moves, 1);
    }

    #[test]
    fn style_changes_are_part_of_the_frame_diff() {
        let mut frame = FrameBuffer::new(1, 1);
        frame.begin_frame();
        frame.put_styled(
            0,
            0,
            'x',
            CellStyle::new(Some(Rgb::new(255, 0, 0)), Some(Rgb::new(0, 0, 255))),
        );
        frame.present(&mut Vec::new()).unwrap();

        frame.begin_frame();
        frame.put_styled(0, 0, 'x', CellStyle::foreground(Rgb::new(0, 255, 0)));
        let mut output = Vec::new();
        let stats = frame.present(&mut output).unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "\x1b[1;1H\x1b[38;2;0;255;0mx\x1b[0m"
        );
        assert_eq!(stats.changed_cells, 1);
        assert_eq!(stats.cursor_moves, 1);
    }

    #[test]
    fn wide_symbols_reserve_a_cell_without_printing_an_extra_space() {
        let mut frame = FrameBuffer::new(5, 1);
        frame.begin_frame();
        frame.put_text(0, 0, "🙂x");
        let mut output = Vec::new();
        frame.present(&mut output).unwrap();

        assert_eq!(String::from_utf8(output).unwrap(), "\x1b[1;1H🙂x  ");
        assert!(frame.get(0, 1).unwrap().continuation);
        assert_eq!(frame.get(0, 2).unwrap().symbol, "x");
    }

    #[test]
    fn styled_nerd_glyph_paints_its_reserved_cell_before_the_glyph() {
        let mut frame = FrameBuffer::new(5, 1);
        let rail = Rgb::new(12, 23, 34);
        frame.begin_frame();
        frame.put_grapheme_styled(
            0,
            1,
            "\u{f07b}",
            CellStyle::new(Some(Rgb::new(255, 255, 255)), Some(rail)),
        );
        let mut output = Vec::new();
        frame.present_clean_rows(&mut output).unwrap();
        let output = String::from_utf8(output).unwrap();
        let fill = output.find("\x1b[1;3H\x1b[48;2;12;23;34m ").unwrap();
        let glyph = output.find("\u{f07b}").unwrap();
        assert!(fill < glyph, "reserved background must be painted first");
        assert!(frame.get(0, 2).unwrap().continuation);
    }

    #[test]
    fn removing_a_wide_symbol_erases_both_cells() {
        let mut frame = FrameBuffer::new(3, 1);
        frame.begin_frame();
        frame.put_text(0, 0, "🙂x");
        frame.present(&mut Vec::new()).unwrap();

        frame.begin_frame();
        frame.put_text(0, 0, "a");
        let mut output = Vec::new();
        frame.present(&mut output).unwrap();

        assert_eq!(String::from_utf8(output).unwrap(), "\x1b[1;1Ha  ");
    }

    #[test]
    fn clean_rows_erase_before_redrawing_unicode() {
        let mut frame = FrameBuffer::new(4, 1);
        frame.begin_frame();
        frame.put_text(0, 0, "🙂x");
        frame.present_clean_rows(&mut Vec::new()).unwrap();

        frame.begin_frame();
        frame.put_text(0, 0, "a");
        let mut output = Vec::new();
        frame.present_clean_rows(&mut output).unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "\x1b[0m\x1b[1;1H\x1b[2Ka"
        );
    }

    #[test]
    fn emoji_sequence_is_kept_in_one_display_cell_group() {
        let mut frame = FrameBuffer::new(4, 1);
        frame.begin_frame();
        frame.put_text(0, 0, "👨‍💻x");
        let mut output = Vec::new();
        frame.present(&mut output).unwrap();

        assert_eq!(String::from_utf8(output).unwrap(), "\x1b[1;1H👨‍💻x ");
        assert!(frame.get(0, 1).unwrap().continuation);
        assert_eq!(frame.get(0, 2).unwrap().symbol, "x");
    }

    #[test]
    fn overlays_erase_the_entire_underlying_wide_glyph() {
        for column in [1, 2] {
            let mut frame = FrameBuffer::new(5, 1);
            let background = CellStyle::background(Rgb::new(12, 34, 56));
            frame.put_grapheme_styled(0, 1, "🙂", background);
            frame.put(0, column, '|');
            let other = if column == 1 { 2 } else { 1 };
            assert_eq!(frame.get(0, column).unwrap().symbol, "|");
            assert_eq!(frame.get(0, other).unwrap().symbol, " ");
            assert_eq!(frame.get(0, other).unwrap().style, background);
            assert!(!frame.get(0, other).unwrap().continuation);
            let mut output = Vec::new();
            frame.present_clean_rows(&mut output).unwrap();
            assert!(!String::from_utf8(output).unwrap().contains('🙂'));
        }
    }

    #[test]
    fn wide_glyph_at_the_screen_edge_cannot_wrap() {
        let mut frame = FrameBuffer::new(3, 1);
        assert!(!frame.put_grapheme_styled(0, 2, "🙂", CellStyle::default()));
        assert_eq!(frame.get(0, 2).unwrap().symbol, " ");
    }

    #[test]
    fn cell_text_cannot_emit_terminal_controls() {
        let mut frame = FrameBuffer::new(32, 1);
        frame.put_text(0, 0, "title\x1b]52;c;YQ==\x07\nend");
        assert!(!frame.put_grapheme_styled(0, 31, "\x1b[2J", CellStyle::default()));
        let mut output = Vec::new();
        frame.present_clean_rows(&mut output).unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(!output.contains("\x1b]52"));
        assert!(!output.contains("\x1b[2J"));
        assert!(!output.contains(['\x07', '\n']));
        assert!(output.contains("title]52;c;YQ==end"));
    }

    #[test]
    fn redraws_reuse_cell_text_storage() {
        let mut frame = FrameBuffer::new(3, 1);
        frame.put_text(0, 0, "👨‍💻x");
        frame.present_clean_rows(&mut Vec::new()).unwrap();
        let current = frame.current[0].symbol.as_ptr();
        let previous = frame.previous[0].symbol.as_ptr();
        frame.begin_frame();
        frame.put_text(0, 0, "🙂y");
        frame.present_clean_rows(&mut Vec::new()).unwrap();
        assert_eq!(frame.current[0].symbol.as_ptr(), current);
        assert_eq!(frame.previous[0].symbol.as_ptr(), previous);
    }
}
