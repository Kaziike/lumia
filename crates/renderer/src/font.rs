use ab_glyph::{point, Font, FontArc, GlyphId, OutlinedGlyph, PxScale, ScaleFont};
use std::fs;
use std::path::{Path, PathBuf};
use tiny_skia::PixmapMut;
use wm_proto::{Color, Rect, TextAlign};

/// Manages system TrueType/OpenType fonts loaded via `ab_glyph` along with a
/// zero-dependency built-in crisp bitmap/vector fallback glyph renderer.
#[derive(Clone)]
pub struct FontManager {
    sans_regular: Option<FontArc>,
    sans_bold: Option<FontArc>,
    monospace: Option<FontArc>,
}

impl Default for FontManager {
    fn default() -> Self {
        Self::load_system_fonts()
    }
}

impl FontManager {
    /// Scans standard Linux font directories for readable sans-serif and monospace fonts.
    /// Always succeeds: if no system fonts are installed, falls back to the built-in glyph renderer.
    pub fn load_system_fonts() -> Self {
        let search_dirs = Self::standard_font_dirs();

        let sans_regular_candidates = [
            "DejaVuSans.ttf",
            "AdwaitaSans-Regular.ttf",
            "NotoSans-Regular.ttf",
            "LiberationSans-Regular.ttf",
            "Ubuntu-R.ttf",
            "Ubuntu-Regular.ttf",
            "Cantarell-Regular.otf",
            "FreeSans.ttf",
            "DroidSans.ttf",
            "FiraSans-Regular.ttf",
            "Inter-Regular.ttf",
            "Roboto-Regular.ttf",
        ];

        let sans_bold_candidates = [
            "DejaVuSans-Bold.ttf",
            "AdwaitaSans-Bold.ttf",
            "NotoSans-Bold.ttf",
            "LiberationSans-Bold.ttf",
            "Ubuntu-B.ttf",
            "Ubuntu-Bold.ttf",
            "Cantarell-Bold.otf",
            "FreeSansBold.ttf",
            "DroidSans-Bold.ttf",
            "FiraSans-Bold.ttf",
            "Inter-Bold.ttf",
            "Roboto-Bold.ttf",
        ];

        let mono_candidates = [
            "JetBrainsMono-Regular.ttf",
            "DejaVuSansMono.ttf",
            "AdwaitaMono-Regular.ttf",
            "LiberationMono-Regular.ttf",
            "NotoSansMono-Regular.ttf",
            "FiraCode-Regular.ttf",
            "UbuntuMono-R.ttf",
            "FreeMono.ttf",
            "Hack-Regular.ttf",
        ];

        let mut sans_regular = Self::find_font_by_names(&search_dirs, &sans_regular_candidates);
        let sans_bold = Self::find_font_by_names(&search_dirs, &sans_bold_candidates);
        let monospace = Self::find_font_by_names(&search_dirs, &mono_candidates);

        if sans_regular.is_none() {
            sans_regular = Self::find_any_font(&search_dirs);
        }

        Self {
            sans_regular,
            sans_bold,
            monospace,
        }
    }

    /// Creates a `FontManager` that exclusively uses the built-in fallback font renderer.
    /// Useful for testing deterministic rendering in minimal environments with zero system fonts.
    pub fn fallback_only() -> Self {
        Self {
            sans_regular: None,
            sans_bold: None,
            monospace: None,
        }
    }

    /// Returns true if at least one system TrueType/OpenType font was loaded.
    pub fn has_system_font(&self) -> bool {
        self.sans_regular.is_some() || self.sans_bold.is_some() || self.monospace.is_some()
    }

    fn standard_font_dirs() -> Vec<PathBuf> {
        let mut dirs = vec![
            PathBuf::from("/usr/share/fonts/TTF"),
            PathBuf::from("/usr/share/fonts/noto"),
            PathBuf::from("/usr/share/fonts/liberation"),
            PathBuf::from("/usr/share/fonts/dejavu"),
            PathBuf::from("/usr/share/fonts/Adwaita"),
            PathBuf::from("/usr/share/fonts/truetype"),
            PathBuf::from("/usr/share/fonts/opentype"),
            PathBuf::from("/usr/share/fonts"),
            PathBuf::from("/usr/local/share/fonts"),
        ];
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(PathBuf::from(&home).join(".local/share/fonts"));
            dirs.push(PathBuf::from(&home).join(".fonts"));
        }
        dirs
    }

    fn find_font_by_names(dirs: &[PathBuf], names: &[&str]) -> Option<FontArc> {
        // First check direct paths in each directory
        for name in names {
            for dir in dirs {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    if let Some(font) = Self::try_load_font_file(&candidate) {
                        return Some(font);
                    }
                }
            }
        }
        // Next recursively scan up to depth 3 for matching file names
        for name in names {
            for dir in dirs {
                if let Some(font) = Self::walk_for_filename(dir, Some(name), 0, 3) {
                    return Some(font);
                }
            }
        }
        None
    }

    fn find_any_font(dirs: &[PathBuf]) -> Option<FontArc> {
        for dir in dirs {
            if let Some(font) = Self::walk_for_filename(dir, None, 0, 3) {
                return Some(font);
            }
        }
        None
    }

    fn walk_for_filename(
        dir: &Path,
        target_name: Option<&str>,
        depth: usize,
        max_depth: usize,
    ) -> Option<FontArc> {
        if depth > max_depth || !dir.is_dir() {
            return None;
        }
        let entries = fs::read_dir(dir).ok()?;
        let mut subdirs = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if let Some(wanted) = target_name {
                    if file_name.eq_ignore_ascii_case(wanted) {
                        if let Some(font) = Self::try_load_font_file(&path) {
                            return Some(font);
                        }
                    }
                } else {
                    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                    if ext.eq_ignore_ascii_case("ttf") || ext.eq_ignore_ascii_case("otf") {
                        if let Some(font) = Self::try_load_font_file(&path) {
                            return Some(font);
                        }
                    }
                }
            } else if path.is_dir() {
                subdirs.push(path);
            }
        }

        for subdir in subdirs {
            if let Some(font) = Self::walk_for_filename(&subdir, target_name, depth + 1, max_depth)
            {
                return Some(font);
            }
        }
        None
    }

    fn try_load_font_file(path: &Path) -> Option<FontArc> {
        let bytes = fs::read(path).ok()?;
        FontArc::try_from_vec(bytes).ok()
    }

    fn select_font(&self, family: &str, weight: u16) -> Option<&FontArc> {
        let fam_lower = family.to_ascii_lowercase();
        let is_mono = fam_lower.contains("mono")
            || fam_lower.contains("code")
            || fam_lower.contains("console")
            || fam_lower.contains("courier")
            || fam_lower.contains("jetbrains");

        if is_mono {
            if let Some(mono) = &self.monospace {
                return Some(mono);
            }
        }
        if weight >= 600 {
            if let Some(bold) = &self.sans_bold {
                return Some(bold);
            }
        }
        self.sans_regular
            .as_ref()
            .or(self.sans_bold.as_ref())
            .or(self.monospace.as_ref())
    }

    /// Measures the single-line `(width, height)` in pixels of `text`.
    pub fn measure_text(
        &self,
        text: &str,
        font_family: &str,
        font_size: f32,
        font_weight: u16,
    ) -> (f32, f32) {
        let size = font_size.max(8.0);
        if text.is_empty() {
            return (0.0, size);
        }

        if let Some(font) = self.select_font(font_family, font_weight) {
            let scale = PxScale::from(size);
            let scaled = font.as_scaled(scale);
            let mut width = 0.0f32;
            let mut prev_glyph: Option<GlyphId> = None;

            for ch in text.chars() {
                if ch == '\n' || ch == '\r' {
                    continue;
                }
                let glyph_id = scaled.glyph_id(ch);
                if glyph_id.0 == 0 && !ch.is_whitespace() {
                    // Missing glyph in font; use fallback advance
                    width += size * 0.6;
                    prev_glyph = None;
                } else {
                    if let Some(prev) = prev_glyph {
                        width += scaled.kern(prev, glyph_id);
                    }
                    width += scaled.h_advance(glyph_id);
                    prev_glyph = Some(glyph_id);
                }
            }
            let height = (scaled.ascent() - scaled.descent()).max(size);
            (width, height)
        } else {
            // Fallback bitmap font metrics
            let char_count = text.chars().filter(|c| *c != '\n' && *c != '\r').count();
            let advance = (size * 0.6).max(5.0);
            (char_count as f32 * advance, size)
        }
    }

    /// Draws `text` inside `bounds` (`content_box`) with horizontal alignment (`TextAlign`),
    /// vertical centering, optional ellipsis/clipping, and premultiplied alpha blending.
    pub fn draw_text(
        &self,
        pixmap: &mut PixmapMut<'_>,
        text: &str,
        bounds: Rect,
        font_family: &str,
        font_size: f32,
        font_weight: u16,
        color: Color,
        text_align: TextAlign,
        opacity: f32,
        clip_rect: Option<Rect>,
    ) {
        let trimmed = text.trim_matches(|c| c == '\n' || c == '\r');
        if trimmed.is_empty() || bounds.width <= 0.0 || bounds.height <= 0.0 {
            return;
        }

        let effective_alpha = ((color.a as f32 / 255.0) * opacity.clamp(0.0, 1.0)).clamp(0.0, 1.0);
        if effective_alpha <= 0.001 {
            return;
        }

        let size = font_size.max(8.0);
        let (measured_w, _measured_h) =
            self.measure_text(trimmed, font_family, size, font_weight);

        let start_x = match text_align {
            TextAlign::Left => bounds.x,
            TextAlign::Center => bounds.x + ((bounds.width - measured_w) * 0.5).max(0.0),
            TextAlign::Right => bounds.x + (bounds.width - measured_w).max(0.0),
        };

        let max_x = bounds.right() + 2.0;
        let bold_extra = font_weight >= 600 && self.sans_bold.is_none();

        if let Some(font) = self.select_font(font_family, font_weight) {
            let scale = PxScale::from(size);
            let scaled = font.as_scaled(scale);
            let ascent = scaled.ascent();
            let descent = scaled.descent();
            let text_h = ascent - descent;
            let baseline_y = bounds.y + (bounds.height - text_h) * 0.5 + ascent;

            let mut cursor_x = start_x;
            let mut prev_glyph: Option<GlyphId> = None;

            for ch in trimmed.chars() {
                if ch == '\n' || ch == '\r' {
                    continue;
                }
                let glyph_id = scaled.glyph_id(ch);
                if let Some(prev) = prev_glyph {
                    cursor_x += scaled.kern(prev, glyph_id);
                }
                let advance = if glyph_id.0 == 0 && !ch.is_whitespace() {
                    size * 0.62
                } else {
                    scaled.h_advance(glyph_id)
                };

                if cursor_x + advance > max_x && cursor_x > start_x {
                    break;
                }

                if !ch.is_whitespace() {
                    let glyph = glyph_id.with_scale_and_position(scale, point(cursor_x, baseline_y));
                    if glyph_id.0 != 0 {
                        if let Some(outlined) = font.outline_glyph(glyph) {
                            Self::blit_outlined_glyph(
                                pixmap,
                                &outlined,
                                color,
                                effective_alpha,
                                clip_rect,
                                bold_extra,
                            );
                        } else {
                            Self::draw_fallback_char(
                                pixmap,
                                ch,
                                cursor_x,
                                bounds.y + (bounds.height - size) * 0.5,
                                size,
                                color,
                                effective_alpha,
                                clip_rect,
                                font_weight >= 600,
                            );
                        }
                    } else {
                        Self::draw_fallback_char(
                            pixmap,
                            ch,
                            cursor_x,
                            bounds.y + (bounds.height - size) * 0.5,
                            size,
                            color,
                            effective_alpha,
                            clip_rect,
                            font_weight >= 600,
                        );
                    }
                }

                cursor_x += advance;
                prev_glyph = Some(glyph_id);
            }
        } else {
            // Pure fallback bitmap renderer
            let advance = (size * 0.6).max(5.0);
            let top_y = bounds.y + (bounds.height - size) * 0.5;
            let mut cursor_x = start_x;

            for ch in trimmed.chars() {
                if ch == '\n' || ch == '\r' {
                    continue;
                }
                if cursor_x + advance > max_x && cursor_x > start_x {
                    break;
                }
                if !ch.is_whitespace() {
                    Self::draw_fallback_char(
                        pixmap,
                        ch,
                        cursor_x,
                        top_y,
                        size,
                        color,
                        effective_alpha,
                        clip_rect,
                        font_weight >= 600,
                    );
                }
                cursor_x += advance;
            }
        }
    }

    fn blit_outlined_glyph(
        pixmap: &mut PixmapMut<'_>,
        outlined: &OutlinedGlyph,
        color: Color,
        effective_alpha: f32,
        clip_rect: Option<Rect>,
        faux_bold: bool,
    ) {
        let px_bounds = outlined.px_bounds();
        let width = pixmap.width() as i32;
        let height = pixmap.height() as i32;
        let data = pixmap.data_mut();

        outlined.draw(|gx, gy, coverage| {
            let cov = (coverage * effective_alpha).clamp(0.0, 1.0);
            if cov <= 0.002 {
                return;
            }
            let x = px_bounds.min.x as i32 + gx as i32;
            let y = px_bounds.min.y as i32 + gy as i32;

            Self::blend_pixel(data, width, height, x, y, color, cov, clip_rect);
            if faux_bold {
                Self::blend_pixel(data, width, height, x + 1, y, color, cov * 0.65, clip_rect);
            }
        });
    }

    #[inline]
    fn blend_pixel(
        data: &mut [u8],
        width: i32,
        height: i32,
        x: i32,
        y: i32,
        color: Color,
        alpha: f32,
        clip_rect: Option<Rect>,
    ) {
        if x < 0 || y < 0 || x >= width || y >= height {
            return;
        }
        if let Some(clip) = clip_rect {
            let fx = x as f32;
            let fy = y as f32;
            if fx < clip.x || fx >= clip.right() || fy < clip.y || fy >= clip.bottom() {
                return;
            }
        }

        let idx = ((y * width + x) * 4) as usize;
        if idx + 3 >= data.len() {
            return;
        }

        // Source-over blending into premultiplied RGBA8 buffer
        let src_a = alpha.clamp(0.0, 1.0);
        let src_r = (color.r as f32) * src_a;
        let src_g = (color.g as f32) * src_a;
        let src_b = (color.b as f32) * src_a;
        let inv_a = 1.0 - src_a;

        let dst_r = data[idx] as f32;
        let dst_g = data[idx + 1] as f32;
        let dst_b = data[idx + 2] as f32;
        let dst_a = data[idx + 3] as f32;

        data[idx] = (src_r + dst_r * inv_a).round().clamp(0.0, 255.0) as u8;
        data[idx + 1] = (src_g + dst_g * inv_a).round().clamp(0.0, 255.0) as u8;
        data[idx + 2] = (src_b + dst_b * inv_a).round().clamp(0.0, 255.0) as u8;
        data[idx + 3] = (src_a * 255.0 + dst_a * inv_a).round().clamp(0.0, 255.0) as u8;
    }

    /// Built-in 5x7 / 7x9 scaled bitmap glyph renderer so text ALWAYS renders crisply
    /// even when zero system fonts exist or when rendering special symbols.
    #[allow(clippy::too_many_arguments)]
    fn draw_fallback_char(
        pixmap: &mut PixmapMut<'_>,
        ch: char,
        x: f32,
        y: f32,
        font_size: f32,
        color: Color,
        effective_alpha: f32,
        clip_rect: Option<Rect>,
        bold: bool,
    ) {
        let glyph_rows = fallback_glyph_5x7(ch);
        let gw = 5.0f32;
        let gh = 7.0f32;

        let target_h = (font_size * 0.72).max(7.0);
        let target_w = (font_size * 0.50).max(5.0);
        let offset_y = y + (font_size - target_h) * 0.5;
        let offset_x = x + (font_size * 0.05);

        let px_w = pixmap.width() as i32;
        let px_h = pixmap.height() as i32;
        let data = pixmap.data_mut();

        let steps_y = (target_h.ceil() as i32).max(7);
        let steps_x = (target_w.ceil() as i32).max(5);

        for sy in 0..steps_y {
            let gy = ((sy as f32 / steps_y as f32) * gh).floor() as usize;
            if gy >= 7 {
                continue;
            }
            let row_bits = glyph_rows[gy];
            for sx in 0..steps_x {
                let gx = ((sx as f32 / steps_x as f32) * gw).floor() as usize;
                if gx >= 5 {
                    continue;
                }
                let bit = (row_bits >> (4 - gx)) & 1;
                if bit != 0 {
                    let dst_x = offset_x.round() as i32 + sx;
                    let dst_y = offset_y.round() as i32 + sy;
                    Self::blend_pixel(
                        data,
                        px_w,
                        px_h,
                        dst_x,
                        dst_y,
                        color,
                        effective_alpha,
                        clip_rect,
                    );
                    if bold {
                        Self::blend_pixel(
                            data,
                            px_w,
                            px_h,
                            dst_x + 1,
                            dst_y,
                            color,
                            effective_alpha * 0.75,
                            clip_rect,
                        );
                    }
                }
            }
        }
    }
}

/// Compact 5x7 bitmap font covering printable ASCII and common UI symbols.
/// Each byte has bits 4..=0 representing columns 0..5.
fn fallback_glyph_5x7(ch: char) -> [u8; 7] {
    match ch {
        'A' | 'a' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' | 'b' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' | 'c' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        'D' | 'd' => [0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100],
        'E' | 'e' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' | 'f' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' | 'g' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110],
        'H' | 'h' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' | 'i' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        'J' | 'j' => [0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100],
        'K' | 'k' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' | 'l' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' | 'm' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'N' | 'n' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        'O' | 'o' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' | 'p' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'Q' | 'q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        'R' | 'r' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' | 's' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'T' | 't' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' | 'u' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' | 'v' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'W' | 'w' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
        'X' | 'x' | '✕' | '×' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Y' | 'y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        'Z' | 'z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00110, 0b01000, 0b10000, 0b11111],
        '3' => [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110],
        '6' => [0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110],
        ':' => [0b00000, 0b00100, 0b00100, 0b00000, 0b00100, 0b00100, 0b00000],
        ';' => [0b00000, 0b00100, 0b00000, 0b00100, 0b00100, 0b01000, 0b00000],
        '.' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00100, 0b00100],
        ',' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00100, 0b00100, 0b01000],
        '-' | '—' | '–' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        '_' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b11111],
        '+' => [0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000],
        '/' => [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
        '%' => [0b11001, 0b11010, 0b00010, 0b00100, 0b01000, 0b01011, 0b10011],
        '(' | '[' | '{' => [0b00110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b00110],
        ')' | ']' | '}' => [0b01100, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01100],
        '<' | '◀' => [0b00010, 0b00100, 0b01000, 0b10000, 0b01000, 0b00100, 0b00010],
        '>' | '▶' => [0b01000, 0b00100, 0b00010, 0b00001, 0b00010, 0b00100, 0b01000],
        '!' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100],
        '?' => [0b01110, 0b10001, 0b00001, 0b00110, 0b00100, 0b00000, 0b00100],
        '|' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        '□' | '■' => [0b11111, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11111],
        '•' | '●' | '○' => [0b00000, 0b01110, 0b11111, 0b11111, 0b11111, 0b01110, 0b00000],
        '…' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b10101],
        _ => [0b01110, 0b10001, 0b10101, 0b10101, 0b10101, 0b10001, 0b01110],
    }
}
