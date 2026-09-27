//! CPU rasterizer that turns terminal grid snapshots into RGBA frames.
//!
//! The renderer is deliberately independent from Slint: it owns the font, a
//! glyph cache and the pixel buffer, so it can be unit tested by inspecting
//! raw pixels. The app converts the frame into a `slint::Image`.

use std::borrow::Cow;
use std::collections::HashMap;

use fontdue::{Font, FontSettings};

use crate::cell::TerminalCell;
use crate::color::TerminalColor;
use crate::selection::SelectionRange;

const FONT_BYTES: &[u8] = include_bytes!("../assets/DejaVuSansMono.ttf");
/// Default logical font size in pixels.
pub const DEFAULT_FONT_SIZE: f32 = 16.0;
/// Default text color (`#d8dee9`).
pub const DEFAULT_FOREGROUND: TerminalColor = TerminalColor::rgb(0xd8, 0xde, 0xe9);
/// Default surface color (`#05070b`).
pub const DEFAULT_BACKGROUND: TerminalColor = TerminalColor::rgb(0x05, 0x07, 0x0b);
/// Selection highlight (`#264f78`).
pub const SELECTION_BACKGROUND: TerminalColor = TerminalColor::rgb(0x26, 0x4f, 0x78);

/// A rasterized terminal frame ready to be uploaded as a texture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalFrame {
    pub width: u32,
    pub height: u32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub rgba: Vec<u8>,
}

/// What the renderer needs to know about the current viewport.
///
/// `lines` are already sliced to the visible rows, in viewport order. Visible
/// rows borrow the live grid while expanded scrollback rows are owned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSnapshot<'a> {
    pub lines: Vec<Cow<'a, [TerminalCell]>>,
    /// Cursor position in viewport coordinates.
    pub cursor: Option<(u16, u16)>,
    /// Viewport coordinates (row `0` is the top visible row).
    pub selection: Option<SelectionRange>,
}

#[derive(Debug, Clone)]
struct CachedGlyph {
    width: usize,
    height: usize,
    xmin: i32,
    ymin: i32,
    bitmap: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
struct CellRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

/// Rasterizes [`TerminalSnapshot`]s with the bundled DejaVu Sans Mono font.
pub struct TerminalRenderer {
    font: Font,
    font_size: f32,
    cell_width: u32,
    cell_height: u32,
    ascent: f32,
    glyph_cache: HashMap<char, CachedGlyph>,
}

impl Default for TerminalRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalRenderer {
    /// Create a renderer with the default 16px font size.
    #[must_use]
    pub fn new() -> Self {
        Self::with_font_size(DEFAULT_FONT_SIZE)
    }

    /// Create a renderer with a specific pixel font size.
    #[must_use]
    pub fn with_font_size(font_size: f32) -> Self {
        let font = Font::from_bytes(FONT_BYTES, FontSettings::default())
            .expect("bundled DejaVu Sans Mono font must parse");
        let metrics = font
            .horizontal_line_metrics(font_size)
            .expect("bundled font exposes horizontal metrics");
        let advance = font.metrics('0', font_size).advance_width;
        let cell_width = ((advance).round() as u32).max(1);
        let cell_height =
            ((metrics.ascent - metrics.descent + metrics.line_gap).ceil() as u32).max(1);
        Self {
            font,
            font_size,
            cell_width,
            cell_height,
            ascent: metrics.ascent,
            glyph_cache: HashMap::new(),
        }
    }

    /// Width and height of a single character cell in pixels.
    #[must_use]
    pub const fn cell_size(&self) -> (u32, u32) {
        (self.cell_width, self.cell_height)
    }

    #[must_use]
    pub const fn font_size(&self) -> f32 {
        self.font_size
    }

    /// Rasterize one snapshot into an RGBA frame.
    pub fn render(&mut self, snapshot: &TerminalSnapshot) -> TerminalFrame {
        let columns = snapshot
            .lines
            .iter()
            .map(|line| line.len())
            .max()
            .unwrap_or(0);
        let rows = snapshot.lines.len();
        let width = columns as u32 * self.cell_width;
        let height = rows as u32 * self.cell_height;
        let mut rgba = vec![0u8; width as usize * height as usize * 4];
        fill_rect(
            &mut rgba,
            width,
            0,
            0,
            width,
            height,
            rgba_of(DEFAULT_BACKGROUND),
        );

        let selection = snapshot.selection.map(SelectionRange::normalized);
        // Rasterize every needed glyph once up front so the paint pass can use
        // immutable borrows of the cache.
        let mut glyphs = Vec::new();
        for (row, line) in snapshot.lines.iter().enumerate() {
            for (column, cell) in line.iter().enumerate() {
                if cell.wide_continuation {
                    continue;
                }
                if let Some(character) = cell.grapheme.chars().next() {
                    if character != ' ' {
                        glyphs.push((row, column, character));
                    }
                }
            }
        }
        for (_, _, character) in &glyphs {
            self.ensure_glyph(*character);
        }

        for (row, line) in snapshot.lines.iter().enumerate() {
            let row_u16 = u16::try_from(row).unwrap_or(u16::MAX);
            for column in 0..columns {
                let column_u16 = u16::try_from(column).unwrap_or(u16::MAX);
                let cell = line.get(column).cloned().unwrap_or_default();
                let selected = selection
                    .map(|range| range.contains(column_u16, row_u16))
                    .unwrap_or(false);
                let cursor = snapshot.cursor == Some((column_u16, row_u16));
                let (_, background) = cell_colors(&cell, selected, cursor);
                let cell_x = column as u32 * self.cell_width;
                let cell_y = row as u32 * self.cell_height;
                fill_rect(
                    &mut rgba,
                    width,
                    cell_x,
                    cell_y,
                    self.cell_width,
                    self.cell_height,
                    rgba_of(background),
                );
            }
        }

        for (row, column, character) in glyphs {
            let row_u16 = u16::try_from(row).unwrap_or(u16::MAX);
            let column_u16 = u16::try_from(column).unwrap_or(u16::MAX);
            let cell = snapshot.lines[row][column].clone();
            let selected = selection
                .map(|range| range.contains(column_u16, row_u16))
                .unwrap_or(false);
            let cursor = snapshot.cursor == Some((column_u16, row_u16));
            let (foreground, _) = cell_colors(&cell, selected, cursor);
            if let Some(glyph) = self.glyph_cache.get(&character) {
                paint_glyph(
                    &mut rgba,
                    width,
                    CellRect {
                        x: column as u32 * self.cell_width,
                        y: row as u32 * self.cell_height,
                        width: self.cell_width,
                        height: self.cell_height,
                    },
                    self.ascent.round() as i32,
                    glyph,
                    foreground,
                );
            }
        }

        for (row, line) in snapshot.lines.iter().enumerate() {
            let row_u16 = u16::try_from(row).unwrap_or(u16::MAX);
            for (column, cell) in line.iter().enumerate() {
                if !cell.underline || cell.wide_continuation {
                    continue;
                }
                let column_u16 = u16::try_from(column).unwrap_or(u16::MAX);
                let selected = selection
                    .map(|range| range.contains(column_u16, row_u16))
                    .unwrap_or(false);
                let cursor = snapshot.cursor == Some((column_u16, row_u16));
                let (foreground, _) = cell_colors(cell, selected, cursor);
                paint_underline(
                    &mut rgba,
                    width,
                    CellRect {
                        x: column as u32 * self.cell_width,
                        y: row as u32 * self.cell_height,
                        width: self.cell_width,
                        height: self.cell_height,
                    },
                    self.ascent.round() as i32,
                    foreground,
                );
            }
        }

        TerminalFrame {
            width,
            height,
            cell_width: self.cell_width,
            cell_height: self.cell_height,
            rgba,
        }
    }

    fn ensure_glyph(&mut self, character: char) {
        if self.glyph_cache.contains_key(&character) {
            return;
        }
        let (metrics, bitmap) = self.font.rasterize(character, self.font_size);
        self.glyph_cache.insert(
            character,
            CachedGlyph {
                width: metrics.width,
                height: metrics.height,
                xmin: metrics.xmin,
                ymin: metrics.ymin,
                bitmap,
            },
        );
    }
}

/// Effective (foreground, background) for a cell, honoring default colors,
/// bold lightening, selection blending and cursor inversion.
fn cell_colors(
    cell: &TerminalCell,
    selected: bool,
    cursor: bool,
) -> (TerminalColor, TerminalColor) {
    let mut foreground = if cell.foreground == TerminalColor::WHITE {
        DEFAULT_FOREGROUND
    } else {
        cell.foreground
    };
    if cell.bold {
        foreground = lighten(foreground, 0.25);
    }
    let mut background = if cell.background == TerminalColor::BLACK {
        DEFAULT_BACKGROUND
    } else {
        cell.background
    };
    if selected {
        background = mix(background, SELECTION_BACKGROUND, 0.5);
    }
    if cursor {
        std::mem::swap(&mut foreground, &mut background);
    }
    (foreground, background)
}

fn lighten(color: TerminalColor, amount: f32) -> TerminalColor {
    TerminalColor::rgb(
        lighten_channel(color.red, amount),
        lighten_channel(color.green, amount),
        lighten_channel(color.blue, amount),
    )
}

fn lighten_channel(channel: u8, amount: f32) -> u8 {
    let value = f32::from(channel);
    (value + (255.0 - value) * amount).round().clamp(0.0, 255.0) as u8
}

fn mix(left: TerminalColor, right: TerminalColor, amount: f32) -> TerminalColor {
    let channel = |a: u8, b: u8| {
        (f32::from(a) * (1.0 - amount) + f32::from(b) * amount)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    TerminalColor::rgb(
        channel(left.red, right.red),
        channel(left.green, right.green),
        channel(left.blue, right.blue),
    )
}

const fn rgba_of(color: TerminalColor) -> [u8; 4] {
    [color.red, color.green, color.blue, 255]
}

fn fill_rect(
    rgba: &mut [u8],
    frame_width: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    color: [u8; 4],
) {
    if frame_width == 0 {
        return;
    }
    for row in y..y.saturating_add(height) {
        let start = row as usize * frame_width as usize + x as usize;
        let end = start + width as usize;
        let frame_pixels = rgba.len() / 4;
        let end = end.min(frame_pixels);
        if start >= end {
            continue;
        }
        for pixel in rgba[start * 4..end * 4].as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&color);
        }
    }
}

fn paint_glyph(
    rgba: &mut [u8],
    frame_width: u32,
    rect: CellRect,
    baseline: i32,
    glyph: &CachedGlyph,
    color: TerminalColor,
) {
    let top = baseline - glyph.height as i32 - glyph.ymin;
    for bitmap_y in 0..glyph.height {
        let destination_y = rect.y as i32 + top + bitmap_y as i32;
        if destination_y < 0 {
            continue;
        }
        for bitmap_x in 0..glyph.width {
            let alpha = glyph.bitmap[bitmap_y * glyph.width + bitmap_x];
            if alpha == 0 {
                continue;
            }
            let destination_x = rect.x as i32 + glyph.xmin + bitmap_x as i32;
            if destination_x < 0 {
                continue;
            }
            blend_pixel(
                rgba,
                frame_width,
                destination_x as u32,
                destination_y as u32,
                color,
                alpha,
            );
        }
    }
}

fn paint_underline(
    rgba: &mut [u8],
    frame_width: u32,
    rect: CellRect,
    baseline: i32,
    color: TerminalColor,
) {
    let bottom = rect.y as i32 + rect.height as i32 - 1;
    let y = (rect.y as i32 + baseline + 1).clamp(rect.y as i32, bottom.max(rect.y as i32));
    let pixel = rgba_of(color);
    let frame_pixels = rgba.len() / 4;
    for x in rect.x..rect.x.saturating_add(rect.width) {
        let index = y as usize * frame_width as usize + x as usize;
        if index >= frame_pixels {
            break;
        }
        rgba[index * 4..index * 4 + 4].copy_from_slice(&pixel);
    }
}

fn blend_pixel(rgba: &mut [u8], frame_width: u32, x: u32, y: u32, color: TerminalColor, alpha: u8) {
    let index = y as usize * frame_width as usize + x as usize;
    if index * 4 + 4 > rgba.len() {
        return;
    }
    let coverage = f32::from(alpha) / 255.0;
    let pixel = &mut rgba[index * 4..index * 4 + 4];
    pixel[0] = blend_channel(pixel[0], color.red, coverage);
    pixel[1] = blend_channel(pixel[1], color.green, coverage);
    pixel[2] = blend_channel(pixel[2], color.blue, coverage);
    pixel[3] = 255;
}

fn blend_channel(background: u8, foreground: u8, coverage: f32) -> u8 {
    (f32::from(background) * (1.0 - coverage) + f32::from(foreground) * coverage)
        .round()
        .clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::png;

    fn cell_of(grapheme: &str) -> TerminalCell {
        TerminalCell {
            grapheme: grapheme.to_owned(),
            ..TerminalCell::default()
        }
    }

    fn styled(
        grapheme: &str,
        foreground: TerminalColor,
        background: TerminalColor,
    ) -> TerminalCell {
        let mut cell = cell_of(grapheme);
        cell.foreground = foreground;
        cell.background = background;
        cell
    }

    fn render_lines(
        renderer: &mut TerminalRenderer,
        lines: Vec<Vec<TerminalCell>>,
        cursor: Option<(u16, u16)>,
        selection: Option<SelectionRange>,
    ) -> TerminalFrame {
        let slices = lines
            .iter()
            .map(|line| Cow::Borrowed(line.as_slice()))
            .collect::<Vec<_>>();
        renderer.render(&TerminalSnapshot {
            lines: slices,
            cursor,
            selection,
        })
    }

    fn pixel(frame: &TerminalFrame, x: u32, y: u32) -> [u8; 4] {
        let index = (y * frame.width + x) as usize * 4;
        [
            frame.rgba[index],
            frame.rgba[index + 1],
            frame.rgba[index + 2],
            frame.rgba[index + 3],
        ]
    }

    fn background_rgba() -> [u8; 4] {
        [
            DEFAULT_BACKGROUND.red,
            DEFAULT_BACKGROUND.green,
            DEFAULT_BACKGROUND.blue,
            255,
        ]
    }

    fn is_background(pixel: [u8; 4]) -> bool {
        pixel == background_rgba()
    }

    fn cell_has_content(frame: &TerminalFrame, column: u32, row: u32) -> bool {
        let start_x = column * frame.cell_width;
        let start_y = row * frame.cell_height;
        (start_y..start_y + frame.cell_height).any(|y| {
            (start_x..start_x + frame.cell_width).any(|x| !is_background(pixel(frame, x, y)))
        })
    }

    #[test]
    fn frame_dimensions_follow_grid_and_cell_size() {
        let mut renderer = TerminalRenderer::new();
        let (cell_width, cell_height) = renderer.cell_size();
        assert!(cell_width >= 6, "cell width {cell_width}");
        assert!(cell_height >= 10, "cell height {cell_height}");
        assert!(renderer.font_size() > 0.0);

        let frame = render_lines(&mut renderer, vec![vec![cell_of("A"); 4]; 3], None, None);

        assert_eq!(frame.cell_width, cell_width);
        assert_eq!(frame.cell_height, cell_height);
        assert_eq!(frame.width, cell_width * 4);
        assert_eq!(frame.height, cell_height * 3);
        assert_eq!(frame.rgba.len(), (frame.width * frame.height * 4) as usize);
    }

    #[test]
    fn blank_cells_render_the_default_background() {
        let mut renderer = TerminalRenderer::new();
        let frame = render_lines(&mut renderer, vec![vec![cell_of(" "); 3]; 2], None, None);

        assert_eq!(pixel(&frame, 0, 0), background_rgba());
        assert_eq!(
            pixel(&frame, frame.width - 1, frame.height - 1),
            background_rgba()
        );
    }

    #[test]
    fn text_paints_foreground_pixels() {
        let mut renderer = TerminalRenderer::new();
        let frame = render_lines(&mut renderer, vec![vec![cell_of("A"); 2]], None, None);

        let mut foreground_hits = 0;
        for y in 0..frame.cell_height {
            for x in 0..frame.width {
                if !is_background(pixel(&frame, x, y)) {
                    foreground_hits += 1;
                }
            }
        }
        assert!(
            foreground_hits > 10,
            "only {foreground_hits} foreground pixels"
        );

        let default_foreground = [
            DEFAULT_FOREGROUND.red,
            DEFAULT_FOREGROUND.green,
            DEFAULT_FOREGROUND.blue,
            255,
        ];
        let has_full_coverage = (0..frame.cell_height)
            .any(|y| (0..frame.cell_width).any(|x| pixel(&frame, x, y) == default_foreground));
        assert!(
            has_full_coverage,
            "expected a fully covered foreground pixel"
        );
    }

    #[test]
    fn ansi_red_foreground_is_rendered_red() {
        let mut renderer = TerminalRenderer::new();
        let mut cell = cell_of("A");
        cell.foreground = TerminalColor::RED;
        let frame = render_lines(&mut renderer, vec![vec![cell]], None, None);

        let red = [
            TerminalColor::RED.red,
            TerminalColor::RED.green,
            TerminalColor::RED.blue,
            255,
        ];
        let has_red = (0..frame.cell_height)
            .any(|y| (0..frame.cell_width).any(|x| pixel(&frame, x, y) == red));
        assert!(has_red, "expected ANSI red pixels");
    }

    #[test]
    fn bold_lightens_the_foreground() {
        let mut renderer = TerminalRenderer::new();
        let mut bold = cell_of("H");
        bold.bold = true;
        let frame = render_lines(&mut renderer, vec![vec![cell_of("H"), bold]], None, None);

        let normal = [
            DEFAULT_FOREGROUND.red,
            DEFAULT_FOREGROUND.green,
            DEFAULT_FOREGROUND.blue,
            255,
        ];
        let expected = lighten(DEFAULT_FOREGROUND, 0.25);
        let expected = [expected.red, expected.green, expected.blue, 255];
        let mut checked = 0;
        for y in 0..frame.cell_height {
            for x in 0..frame.cell_width {
                if pixel(&frame, x, y) == normal {
                    assert_eq!(
                        pixel(&frame, x + frame.cell_width, y),
                        expected,
                        "bold pixel should be lightened at ({x}, {y})"
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 0, "expected fully covered normal pixels");
    }

    #[test]
    fn underline_paints_a_line_below_the_baseline() {
        let mut renderer = TerminalRenderer::new();
        let mut cell = cell_of(" ");
        cell.underline = true;
        cell.foreground = TerminalColor::GREEN;
        let frame = render_lines(&mut renderer, vec![vec![cell]], None, None);

        let underline_y = renderer.ascent.round() as u32 + 1;
        assert!(underline_y < frame.cell_height);
        let green = [
            TerminalColor::GREEN.red,
            TerminalColor::GREEN.green,
            TerminalColor::GREEN.blue,
            255,
        ];
        for x in 0..frame.cell_width {
            assert_eq!(pixel(&frame, x, underline_y), green);
        }
    }

    #[test]
    fn cursor_inverts_the_cell() {
        let mut renderer = TerminalRenderer::new();
        let frame = render_lines(
            &mut renderer,
            vec![vec![cell_of(" "), cell_of(" ")]],
            Some((0, 0)),
            None,
        );

        let expected = [
            DEFAULT_FOREGROUND.red,
            DEFAULT_FOREGROUND.green,
            DEFAULT_FOREGROUND.blue,
            255,
        ];
        assert_eq!(pixel(&frame, 0, 0), expected);
        assert_eq!(pixel(&frame, 0, frame.cell_height - 1), expected);
        assert!(is_background(pixel(&frame, frame.cell_width, 0)));
    }

    #[test]
    fn selection_blends_the_cell_background() {
        let mut renderer = TerminalRenderer::new();
        let frame = render_lines(
            &mut renderer,
            vec![vec![cell_of(" "), cell_of(" ")]],
            None,
            Some(SelectionRange {
                start: crate::GridPoint { column: 0, row: 0 },
                end: crate::GridPoint { column: 0, row: 0 },
            }),
        );

        let expected = mix(DEFAULT_BACKGROUND, SELECTION_BACKGROUND, 0.5);
        assert_eq!(
            pixel(&frame, 0, 0),
            [expected.red, expected.green, expected.blue, 255]
        );
        assert!(is_background(pixel(&frame, frame.cell_width, 0)));
    }

    #[test]
    fn wide_continuation_cells_skip_glyph_painting() {
        let mut renderer = TerminalRenderer::new();

        let glyph_line = vec![cell_of(" "), cell_of("B"), cell_of(" ")];
        let glyph_frame = render_lines(&mut renderer, vec![glyph_line], None, None);
        assert!(cell_has_content(&glyph_frame, 1, 0));

        let mut continuation = cell_of("B");
        continuation.wide_continuation = true;
        let continuation_line = vec![cell_of(" "), continuation, cell_of(" ")];
        let continuation_frame = render_lines(&mut renderer, vec![continuation_line], None, None);
        assert!(!cell_has_content(&continuation_frame, 1, 0));
        for y in 0..continuation_frame.cell_height {
            for x in continuation_frame.cell_width..continuation_frame.cell_width * 2 {
                assert_eq!(
                    pixel(&continuation_frame, x, y),
                    background_rgba(),
                    "continuation cell must stay background at ({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn exports_color_check_png_for_evidence() {
        let mut renderer = TerminalRenderer::new();
        let columns = 40usize;
        let rows = 12usize;
        let palette = [
            TerminalColor::RED,
            TerminalColor::GREEN,
            TerminalColor::YELLOW,
            TerminalColor::BLUE,
            TerminalColor::MAGENTA,
            TerminalColor::CYAN,
            TerminalColor::BRIGHT_WHITE,
        ];

        let mut lines = Vec::with_capacity(rows);
        for row in 0..rows {
            let mut line = vec![cell_of(" "); columns];
            for column in 0..columns {
                let color = palette[(row + column) % palette.len()];
                if (row + column) % 3 == 0 {
                    // Color background block: visible regardless of glyph coverage.
                    line[column] = styled(" ", DEFAULT_FOREGROUND, color);
                } else {
                    line[column] = styled("M", color, TerminalColor::BLACK);
                }
            }
            if row == 0 {
                line[0] = styled("A", TerminalColor::RED, TerminalColor::BLACK);
                line[1] = styled("N", TerminalColor::GREEN, TerminalColor::BLACK);
                line[2] = styled("S", TerminalColor::YELLOW, TerminalColor::BLACK);
                line[3] = styled("I", TerminalColor::BLUE, TerminalColor::BLACK);
                let mut bold = styled("B", TerminalColor::MAGENTA, TerminalColor::BLACK);
                bold.bold = true;
                line[4] = bold;
                let mut underlined = styled("U", TerminalColor::CYAN, TerminalColor::BLACK);
                underlined.underline = true;
                line[5] = underlined;
                let mut width = packed_wide_cells();
                line.append(&mut width);
            }
            lines.push(line);
        }

        let selection = SelectionRange {
            start: crate::GridPoint { column: 2, row: 2 },
            end: crate::GridPoint { column: 12, row: 3 },
        };
        let frame = render_lines(&mut renderer, lines, Some((10, 6)), Some(selection));

        let mut colorful = 0usize;
        let mut buckets = [0usize; 6];
        for y in 0..frame.height {
            for x in 0..frame.width {
                let [red, green, blue, _] = pixel(&frame, x, y);
                let distance = (i32::from(red) - 5)
                    .abs()
                    .max((i32::from(green) - 7).abs())
                    .max((i32::from(blue) - 11).abs());
                if distance > 24 {
                    colorful += 1;
                    let mut maxima = [0u8; 3];
                    for (index, channel) in [red, green, blue].into_iter().enumerate() {
                        if channel > 40 {
                            maxima[index] = channel;
                        }
                    }
                    let bucket = match (maxima[0] > 0, maxima[1] > 0, maxima[2] > 0) {
                        (true, false, false) => 0,
                        (false, true, false) => 1,
                        (false, false, true) => 2,
                        (true, true, false) => 3,
                        (false, true, true) => 4,
                        (true, false, true) => 5,
                        _ => 6,
                    };
                    if bucket < 6 {
                        buckets[bucket] += 1;
                    }
                }
            }
        }

        let total = (frame.width * frame.height) as usize;
        let ratio = colorful as f64 / total as f64;
        eprintln!(
            "render-check: {colorful}/{total} content pixels ({:.1}%), buckets={buckets:?}",
            ratio * 100.0
        );
        assert!(ratio > 0.05, "content pixel ratio {ratio:.3} is below 5%");
        let hues = buckets.iter().filter(|count| **count > 50).count();
        assert!(hues >= 3, "expected at least 3 hue families, got {hues}");

        let path = evidence_path();
        std::fs::create_dir_all(path.parent().expect("dist dir")).expect("create dist dir");
        std::fs::write(
            &path,
            png::encode_rgba(frame.width, frame.height, &frame.rgba),
        )
        .expect("write render-check.png");
        let written = std::fs::metadata(&path).expect("render-check.png exists");
        assert!(written.len() > 1024, "png is suspiciously small");
        eprintln!("render-check written: {}", path.display());
    }

    fn packed_wide_cells() -> Vec<TerminalCell> {
        let mut wide = cell_of("\u{597d}");
        wide.foreground = TerminalColor::BRIGHT_RED;
        let mut continuation = cell_of(" ");
        continuation.wide_continuation = true;
        vec![wide, continuation]
    }

    fn evidence_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../dist/linux-x86_64/render-check.png")
    }
}
