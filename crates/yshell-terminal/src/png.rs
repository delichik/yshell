//! Minimal PNG encoder used by renderer tests to export evidence files.
//!
//! Only compiled for tests: it keeps the renderer dependency-free while still
//! letting the test suite write a real PNG (uncompressed zlib blocks).

/// Encode an RGBA frame as a PNG byte stream.
#[must_use]
pub fn encode_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut png = Vec::new();
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");

    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.push(8); // bit depth
    header.push(6); // color type: RGBA
    header.push(0); // deflate
    header.push(0); // adaptive filtering
    header.push(0); // no interlace
    push_chunk(&mut png, b"IHDR", &header);

    let mut raw = Vec::with_capacity(rgba.len() + height as usize);
    let stride = width as usize * 4;
    for row in 0..height as usize {
        raw.push(0); // filter: none
        let start = row * stride;
        raw.extend_from_slice(&rgba[start..start + stride]);
    }
    push_chunk(&mut png, b"IDAT", &zlib_store(&raw));
    push_chunk(&mut png, b"IEND", &[]);
    png
}

fn push_chunk(png: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut payload = Vec::with_capacity(4 + data.len());
    payload.extend_from_slice(chunk_type);
    payload.extend_from_slice(data);
    png.extend_from_slice(&payload);
    png.extend_from_slice(&crc32(&payload).to_be_bytes());
}

/// zlib stream with uncompressed (stored) deflate blocks.
fn zlib_store(data: &[u8]) -> Vec<u8> {
    let mut stream = vec![0x78, 0x01];
    let mut offset = 0;
    if data.is_empty() {
        stream.extend_from_slice(&[0x01, 0x00, 0x00, 0xff, 0xff]);
    }
    while offset < data.len() {
        let remaining = data.len() - offset;
        let block = remaining.min(0xffff);
        let final_block = offset + block >= data.len();
        stream.push(u8::from(final_block));
        let length = block as u16;
        stream.extend_from_slice(&length.to_le_bytes());
        stream.extend_from_slice(&(!length).to_le_bytes());
        stream.extend_from_slice(&data[offset..offset + block]);
        offset += block;
    }
    stream.extend_from_slice(&adler32(data).to_be_bytes());
    stream
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let mut a = 1u32;
    let mut b = 0u32;
    for byte in bytes {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_decodable_two_pixel_png() {
        let png = encode_rgba(2, 1, &[255, 0, 0, 255, 0, 255, 0, 255]);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert!(png.ends_with(&crc32(b"IEND").to_be_bytes()));
        assert!(png.windows(4).any(|window| window == b"IHDR"));
        assert!(png.windows(4).any(|window| window == b"IDAT"));
    }

    #[test]
    fn checksums_match_known_vectors() {
        // "hello" adler32 = 0x062C0215, CRC32 of "123456789" = 0xCBF43926.
        assert_eq!(adler32(b"hello"), 0x062C_0215);
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }
}
