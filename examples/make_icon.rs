//! Writes `assets/icon.ico` (built into the executable by build.rs) and
//! `assets/icon.png` from the icon drawn in `src/app_icon.rs`.
//!
//! cargo run --example make_icon

#[path = "../src/app_icon.rs"]
mod app_icon;

/// The sizes Windows picks from for title bars, the taskbar and Explorer.
const ICO_SIZES: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];

fn main() -> std::io::Result<()> {
    let images: Vec<(u32, Vec<u8>)> = ICO_SIZES.iter().map(|&s| (s, app_icon::render(s))).collect();
    std::fs::write("assets/icon.ico", ico(&images))?;
    std::fs::write("assets/icon.png", png(256, &app_icon::render(256)))?;
    std::fs::write("target/icon-preview.png", preview())?;
    println!("wrote assets/icon.ico ({} sizes) and assets/icon.png", images.len());
    Ok(())
}

/// The small sizes side by side, enlarged 6× with hard pixels, to check them.
fn preview() -> Vec<u8> {
    const SCALE: u32 = 6;
    let sizes = [16u32, 24, 32, 48];
    let (width, height) = (sizes.iter().map(|s| s * SCALE + 8).sum::<u32>(), 48 * SCALE);
    let mut canvas = vec![0u8; (width * height * 4) as usize];
    let mut left = 0;
    for size in sizes {
        let rgba = app_icon::render(size);
        for y in 0..size * SCALE {
            for x in 0..size * SCALE {
                let src = (((y / SCALE) * size + x / SCALE) * 4) as usize;
                let dst = ((y * width + left + x) * 4) as usize;
                canvas[dst..dst + 4].copy_from_slice(&rgba[src..src + 4]);
            }
        }
        left += size * SCALE + 8;
    }
    png_sized(width, height, &canvas)
}

/// An .ico with one 32-bit bitmap per size.
fn ico(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let bitmaps: Vec<Vec<u8>> = images.iter().map(|(size, rgba)| bitmap(*size, rgba)).collect();
    let mut out = Vec::new();
    out.extend(0u16.to_le_bytes()); // reserved
    out.extend(1u16.to_le_bytes()); // type: icon
    out.extend((images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for ((size, _), bitmap) in images.iter().zip(&bitmaps) {
        let dimension = if *size >= 256 { 0 } else { *size as u8 }; // 0 means 256
        out.extend([dimension, dimension, 0, 0]);
        out.extend(1u16.to_le_bytes()); // colour planes
        out.extend(32u16.to_le_bytes()); // bits per pixel
        out.extend((bitmap.len() as u32).to_le_bytes());
        out.extend(offset.to_le_bytes());
        offset += bitmap.len() as u32;
    }
    for bitmap in bitmaps {
        out.extend(bitmap);
    }
    out
}

/// A bottom-up BGRA bitmap with an (all-opaque) mask, as icons store them.
fn bitmap(size: u32, rgba: &[u8]) -> Vec<u8> {
    let mask_row = (size as usize).div_ceil(32) * 4;
    let mut out = Vec::new();
    out.extend(40u32.to_le_bytes()); // header size
    out.extend((size as i32).to_le_bytes());
    out.extend((size as i32 * 2).to_le_bytes()); // colour and mask, stacked
    out.extend(1u16.to_le_bytes());
    out.extend(32u16.to_le_bytes());
    out.extend([0u8; 24]); // no compression, sizes and palette unused
    for y in (0..size).rev() {
        for x in 0..size {
            let i = ((y * size + x) * 4) as usize;
            out.extend([rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
        }
    }
    out.extend(std::iter::repeat_n(0u8, mask_row * size as usize));
    out
}

/// A square PNG.
fn png(size: u32, rgba: &[u8]) -> Vec<u8> {
    png_sized(size, size, rgba)
}

/// A PNG using uncompressed deflate blocks, which needs no compressor.
fn png_sized(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut raw = Vec::new();
    for row in rgba.chunks(width as usize * 4) {
        raw.push(0); // no filter
        raw.extend(row);
    }
    let mut zlib = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, block) in blocks.iter().enumerate() {
        zlib.push(u8::from(i + 1 == blocks.len()));
        zlib.extend((block.len() as u16).to_le_bytes());
        zlib.extend((!(block.len() as u16)).to_le_bytes());
        zlib.extend(*block);
    }
    zlib.extend(adler32(&raw).to_be_bytes());

    let mut header = Vec::new();
    header.extend(width.to_be_bytes());
    header.extend(height.to_be_bytes());
    header.extend([8, 6, 0, 0, 0]); // 8-bit RGBA
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    for (kind, data) in [(b"IHDR", header), (b"IDAT", zlib), (b"IEND", Vec::new())] {
        out.extend((data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend(kind);
        out.extend(&data);
        let crc = crc32(&out[start..]);
        out.extend(crc.to_be_bytes());
    }
    out
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}
