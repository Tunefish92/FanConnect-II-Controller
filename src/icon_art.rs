//! The app icon, drawn in code: a white fan rotor on a rounded square with the accent-blue
//! gradient. Shared by build.rs (the .ico embedded in the Windows executables) and the GUI
//! (window icon, sidebar and About page), so there is no image file to keep in sync.
//! Self-contained: no dependencies, because build.rs includes this file directly.

use std::f32::consts::TAU;

const BLADES: usize = 5;
/// Samples per pixel along each axis, for anti-aliasing.
const SUPERSAMPLE: usize = 4;

/// Colour of one point in normalised coordinates (-1..1, y down), as linear RGBA 0..1.
fn shade(x: f32, y: f32) -> [f32; 4] {
    // Rounded square background.
    let corner = 0.42;
    let (qx, qy) = ((x.abs() - (1.0 - corner)).max(0.0), (y.abs() - (1.0 - corner)).max(0.0));
    if qx * qx + qy * qy > corner * corner {
        return [0.0; 4];
    }
    // Diagonal gradient from #4C8DFF (top left) to #2F6FEB (bottom right).
    let t = ((x + y) * 0.25 + 0.5).clamp(0.0, 1.0);
    let top = [0x4C as f32, 0x8D as f32, 0xFF as f32];
    let bottom = [0x2F as f32, 0x6F as f32, 0xEB as f32];
    let mut rgb = [0.0; 3];
    for i in 0..3 {
        rgb[i] = (top[i] + (bottom[i] - top[i]) * t) / 255.0;
    }

    let r = (x * x + y * y).sqrt();
    let white = if r < 0.17 {
        // Hub, with a small dark centre.
        if r < 0.07 { 0.0 } else { 1.0 }
    } else if r < 0.72 {
        // Swept blades: the angle is twisted with the radius, and each blade narrows outwards.
        let theta = y.atan2(x) + 1.9 * r;
        let sector = TAU / BLADES as f32;
        let offset = (theta.rem_euclid(sector) - sector / 2.0).abs();
        let half_width = sector * (0.36 - 0.12 * r);
        if offset < half_width { 1.0 } else { 0.0 }
    } else if (0.80..0.87).contains(&r) {
        // Faint guard ring.
        0.35
    } else {
        0.0
    };
    for c in &mut rgb {
        *c += (1.0 - *c) * white;
    }
    [rgb[0], rgb[1], rgb[2], 1.0]
}

/// The icon at `size` × `size` pixels as RGBA bytes (not premultiplied), row by row from the top.
pub fn render(size: u32) -> Vec<u8> {
    let size = size as usize;
    let mut rgba = Vec::with_capacity(size * size * 4);
    let samples = (SUPERSAMPLE * SUPERSAMPLE) as f32;
    for py in 0..size {
        for px in 0..size {
            let mut acc = [0.0f32; 4];
            for sy in 0..SUPERSAMPLE {
                for sx in 0..SUPERSAMPLE {
                    let x = ((px * SUPERSAMPLE + sx) as f32 + 0.5) / (size * SUPERSAMPLE) as f32 * 2.0 - 1.0;
                    let y = ((py * SUPERSAMPLE + sy) as f32 + 0.5) / (size * SUPERSAMPLE) as f32 * 2.0 - 1.0;
                    let c = shade(x, y);
                    // Accumulate premultiplied so edges blend correctly.
                    for i in 0..3 {
                        acc[i] += c[i] * c[3];
                    }
                    acc[3] += c[3];
                }
            }
            let alpha = acc[3] / samples;
            for &channel in &acc[..3] {
                let value = if alpha > 0.0 { channel / samples / alpha } else { 0.0 };
                rgba.push((value * 255.0).round().clamp(0.0, 255.0) as u8);
            }
            rgba.push((alpha * 255.0).round() as u8);
        }
    }
    rgba
}

/// A Windows .ico with the icon in the given sizes, as 32-bit BMP entries.
#[allow(dead_code)]
pub fn ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes
        .iter()
        .map(|&size| {
            let rgba = render(size);
            let s = size as usize;
            let mut bmp = Vec::new();
            // BITMAPINFOHEADER; the height counts the colour and mask images together.
            bmp.extend_from_slice(&40u32.to_le_bytes());
            bmp.extend_from_slice(&(size as i32).to_le_bytes());
            bmp.extend_from_slice(&(2 * size as i32).to_le_bytes());
            bmp.extend_from_slice(&1u16.to_le_bytes());
            bmp.extend_from_slice(&32u16.to_le_bytes());
            bmp.extend_from_slice(&[0u8; 24]);
            // Colour image, bottom row first, BGRA.
            for row in (0..s).rev() {
                for px in &rgba[row * s * 4..(row + 1) * s * 4].chunks(4).collect::<Vec<_>>() {
                    bmp.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                }
            }
            // AND mask (unused with 32-bit alpha): 1 bit per pixel, rows padded to 4 bytes.
            let mask_row = s.div_ceil(32) * 4;
            bmp.extend(std::iter::repeat_n(0u8, mask_row * s));
            bmp
        })
        .collect();

    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len();
    for (&size, image) in sizes.iter().zip(&images) {
        let dim = if size >= 256 { 0 } else { size as u8 };
        ico.extend_from_slice(&[dim, dim, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(image.len() as u32).to_le_bytes());
        ico.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += image.len();
    }
    for image in images {
        ico.extend_from_slice(&image);
    }
    ico
}

/// The icon as a PNG file (used for the Linux app menu). The image data is stored uncompressed
/// (deflate "stored" blocks), which keeps this encoder dependency-free.
#[allow(dead_code)]
pub fn png(size: u32) -> Vec<u8> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in bytes {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }

    let rgba = render(size);
    let row_len = size as usize * 4;
    // Each row starts with filter type 0 (none).
    let mut raw = Vec::with_capacity((row_len + 1) * size as usize);
    for row in rgba.chunks(row_len) {
        raw.push(0);
        raw.extend_from_slice(row);
    }

    // zlib stream with stored blocks of at most 65535 bytes, then the Adler-32 checksum.
    let mut zlib = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, block) in blocks.iter().enumerate() {
        zlib.push(u8::from(i == blocks.len() - 1));
        let len = block.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut png = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];
    let mut header = Vec::new();
    header.extend_from_slice(&size.to_be_bytes());
    header.extend_from_slice(&size.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]); // 8 bits per channel, RGBA, no interlace
    chunk(&mut png, b"IHDR", &header);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    png
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_has_signature_header_and_end() {
        let png = png(32);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n']);
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 32);
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
        // Written for an external decoder check (e.g. System.Drawing on Windows).
        std::fs::write(std::env::temp_dir().join("gpu-fanctl-icon-test.png"), png).unwrap();
    }

    #[test]
    fn ico_lists_every_size() {
        let ico = ico(&[16, 256]);
        assert_eq!(&ico[..4], &[0, 0, 1, 0]);
        assert_eq!(u16::from_le_bytes([ico[4], ico[5]]), 2);
    }
}
