use std::fs;
use std::io;
use std::path::Path;
use wm_proto::FrameBuffer;

/// Encodes a `FrameBuffer` into binary PPM (`P6`) format.
/// Premultiplied RGBA8 pixels are composited onto a dark slate backdrop (`#11111b`)
/// so translucent shell surfaces look crisp in standard image viewers.
pub fn encode_ppm(fb: &FrameBuffer) -> Vec<u8> {
    let header = format!("P6\n{} {}\n255\n", fb.width, fb.height);
    let pixel_count = (fb.width as usize) * (fb.height as usize);
    let mut out = Vec::with_capacity(header.len() + pixel_count * 3);
    out.extend_from_slice(header.as_bytes());

    let bg_r = 17u32;
    let bg_g = 17u32;
    let bg_b = 27u32;

    for chunk in fb.pixels.chunks_exact(4).take(pixel_count) {
        let r_p = chunk[0] as u32;
        let g_p = chunk[1] as u32;
        let b_p = chunk[2] as u32;
        let a = chunk[3] as u32;
        let inv_a = 255u32.saturating_sub(a);

        let r = (r_p + (bg_r * inv_a) / 255).min(255) as u8;
        let g = (g_p + (bg_g * inv_a) / 255).min(255) as u8;
        let b = (b_p + (bg_b * inv_a) / 255).min(255) as u8;
        out.push(r);
        out.push(g);
        out.push(b);
    }

    out
}

/// Saves a `FrameBuffer` to disk as a binary `.ppm` (`P6`) image.
pub fn save_ppm(fb: &FrameBuffer, path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let bytes = encode_ppm(fb);
    fs::write(path, bytes)
}

/// Encodes a `FrameBuffer` into a 32-bit uncompressed BGRA `.bmp` byte vector.
pub fn encode_bmp(fb: &FrameBuffer) -> Vec<u8> {
    let width = fb.width;
    let height = fb.height;
    let pixel_bytes_len = (width * height * 4) as usize;
    let file_header_size = 14usize;
    let info_header_size = 40usize;
    let data_offset = file_header_size + info_header_size;
    let file_size = data_offset + pixel_bytes_len;

    let mut out = Vec::with_capacity(file_size);

    // BITMAPFILEHEADER (14 bytes)
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(file_size as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(data_offset as u32).to_le_bytes());

    // BITMAPINFOHEADER (40 bytes) - negative height for top-down scanline order
    let top_down_height = -(height as i32);
    out.extend_from_slice(&(info_header_size as u32).to_le_bytes());
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&top_down_height.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bpp
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&(pixel_bytes_len as u32).to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes()); // 72 DPI
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());

    // Pixel data in BGRA8 order (un-premultiplied for standard BMP viewers)
    let pixel_count = (width as usize) * (height as usize);
    for chunk in fb.pixels.chunks_exact(4).take(pixel_count) {
        let r_p = chunk[0] as u32;
        let g_p = chunk[1] as u32;
        let b_p = chunk[2] as u32;
        let a = chunk[3] as u32;

        let (r, g, b) = if a == 0 {
            (0u8, 0u8, 0u8)
        } else if a == 255 {
            (r_p as u8, g_p as u8, b_p as u8)
        } else {
            (
                ((r_p * 255) / a).min(255) as u8,
                ((g_p * 255) / a).min(255) as u8,
                ((b_p * 255) / a).min(255) as u8,
            )
        };
        out.push(b);
        out.push(g);
        out.push(r);
        out.push(a as u8);
    }

    out
}

/// Saves a `FrameBuffer` to disk as a 32-bit `.bmp` image.
pub fn save_bmp(fb: &FrameBuffer, path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let bytes = encode_bmp(fb);
    fs::write(path, bytes)
}
