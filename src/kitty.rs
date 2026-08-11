use std::io::{self, Write};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use flate2::Compression;
use flate2::write::ZlibEncoder;

const PAYLOAD_CHUNK_SIZE: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Placement {
    pub image_id: u32,
    pub columns: u16,
    pub rows: u16,
    pub z_index: i32,
}

pub fn compress_rgba(rgba: &[u8]) -> io::Result<Vec<u8>> {
    let mut compressor = ZlibEncoder::new(Vec::new(), Compression::fast());
    compressor.write_all(rgba)?;
    compressor.finish()
}

pub fn transmit_compressed_rgba(
    output: &mut impl Write,
    compressed_rgba: &[u8],
    width: u32,
    height: u32,
    placement: Placement,
) -> io::Result<()> {
    if width == 0 || height == 0 || placement.columns == 0 || placement.rows == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "formula image dimensions must be non-zero",
        ));
    }

    let encoded = STANDARD.encode(compressed_rgba);
    let chunks = encoded.as_bytes().chunks(PAYLOAD_CHUNK_SIZE);
    let chunk_count = chunks.len();
    for (index, chunk) in chunks.enumerate() {
        let more = u8::from(index + 1 < chunk_count);
        if index == 0 {
            // Constrain only the width. Supplying both cell dimensions makes
            // Kitty stretch the image when independently rounded columns and
            // rows do not exactly match the raster's aspect ratio.
            write!(
                output,
                "\x1b_Ga=T,f=32,s={width},v={height},i={},p=1,o=z,c={},z={},C=1,q=2,m={more};",
                placement.image_id, placement.columns, placement.z_index
            )?;
        } else {
            write!(output, "\x1b_Gq=2,m={more};")?;
        }
        output.write_all(chunk)?;
        output.write_all(b"\x1b\\")?;
    }
    output.flush()
}

pub fn delete_image(output: &mut impl Write, image_id: u32) -> io::Result<()> {
    write!(output, "\x1b_Ga=d,d=I,i={image_id},q=2\x1b\\")?;
    output.flush()
}

pub fn delete_all(output: &mut impl Write) -> io::Result<()> {
    output.write_all(b"\x1b_Ga=d,d=A,q=2\x1b\\")?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::{PAYLOAD_CHUNK_SIZE, Placement, compress_rgba, transmit_compressed_rgba};

    #[test]
    fn rejects_zero_sized_formula_images() {
        let error = transmit_compressed_rgba(
            &mut Vec::new(),
            &[0; 3],
            0,
            1,
            Placement {
                image_id: 1,
                columns: 1,
                rows: 1,
                z_index: 1,
            },
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn chunks_payload_at_the_kitty_limit() {
        let pixels = (0..16_384)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let compressed = compress_rgba(&pixels).unwrap();
        let mut output = Vec::new();
        transmit_compressed_rgba(
            &mut output,
            &compressed,
            64,
            64,
            Placement {
                image_id: 7,
                columns: 8,
                rows: 4,
                z_index: 1,
            },
        )
        .unwrap();

        let output = String::from_utf8(output).unwrap();
        let header = output.split_once(';').unwrap().0;
        assert!(header.contains("c=8"));
        assert!(!header.contains(",r="));
        for command in output.split("\x1b\\").filter(|command| !command.is_empty()) {
            let payload = command.split_once(';').unwrap().1;
            assert!(payload.len() <= PAYLOAD_CHUNK_SIZE);
            assert_eq!(payload.len() % 4, 0);
        }
    }
}
