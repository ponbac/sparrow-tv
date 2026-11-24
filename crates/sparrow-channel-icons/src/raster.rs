use std::{cell::Cell, io::Cursor, panic::catch_unwind};

use image::{DynamicImage, ImageDecoder, ImageError, ImageFormat, ImageReader, Limits};

use crate::ChannelIconError;

const MAX_SOURCE_EDGE: u32 = 4096;
const MAX_SOURCE_PIXELS: u64 = 16_000_000;
const MAX_DECODER_BYTES: u64 = 128 * 1024 * 1024;
const MAX_OUTPUT_EDGE: u32 = 256;

thread_local! {
    static ICON_DECODE_CAN_UNWIND: Cell<bool> = const { Cell::new(false) };
}

/// Lets a host's fatal panic hook allow the unwind protected by `decode`.
/// Call this on the panicking thread.
pub fn is_contained_icon_decode_panic() -> bool {
    ICON_DECODE_CAN_UNWIND.try_with(Cell::get).unwrap_or(false)
}

struct DecodePanicScope<'a> {
    state: &'a Cell<bool>,
    previous: bool,
}

impl Drop for DecodePanicScope<'_> {
    fn drop(&mut self) {
        self.state.set(self.previous);
    }
}

/// One decoded frame, downscaled for the guide renderer.
pub struct DecodedChannelIcon {
    source_dimensions: (u32, u32),
    bgra: image::RgbaImage,
}

impl DecodedChannelIcon {
    pub fn source_dimensions(&self) -> (u32, u32) {
        self.source_dimensions
    }

    /// Returns width, height and tightly packed, straight-alpha BGRA pixels.
    pub fn into_bgra(self) -> (u32, u32, Vec<u8>) {
        let (width, height) = self.bgra.dimensions();
        (width, height, self.bgra.into_raw())
    }
}

pub(crate) fn decode(bytes: &[u8]) -> Result<DecodedChannelIcon, ChannelIconError> {
    ICON_DECODE_CAN_UNWIND.with(|state| {
        let _scope = DecodePanicScope {
            state,
            previous: state.replace(true),
        };
        catch_unwind(|| decode_raster(bytes)).unwrap_or(Err(ChannelIconError::InvalidImage))
    })
}

fn decode_raster(bytes: &[u8]) -> Result<DecodedChannelIcon, ChannelIconError> {
    let format = image::guess_format(bytes).map_err(|_| ChannelIconError::UnsupportedFormat)?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP
    ) {
        return Err(ChannelIconError::UnsupportedFormat);
    }
    if format == ImageFormat::WebP {
        check_simple_webp(bytes)?;
    }

    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_EDGE);
    limits.max_image_height = Some(MAX_SOURCE_EDGE);
    limits.max_alloc = Some(MAX_DECODER_BYTES);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits);
    let decoder = reader.into_decoder().map_err(image_error)?;
    let (width, height) = decoder.dimensions();
    check_dimensions(width, height)?;
    if format == ImageFormat::Gif {
        check_gif_frame(bytes)?;
    }

    // The single-image decoder reads the first frame of an animated format.
    let mut image = DynamicImage::from_decoder(decoder).map_err(image_error)?;
    if width.max(height) > MAX_OUTPUT_EDGE {
        image = image.resize(
            MAX_OUTPUT_EDGE,
            MAX_OUTPUT_EDGE,
            image::imageops::FilterType::Triangle,
        );
    }
    let mut bgra = image.into_rgba8();
    for pixel in bgra.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Ok(DecodedChannelIcon {
        source_dimensions: (width, height),
        bgra,
    })
}

fn check_dimensions(width: u32, height: u32) -> Result<(), ChannelIconError> {
    if width == 0 || height == 0 {
        return Err(ChannelIconError::InvalidImage);
    }
    if width.max(height) > MAX_SOURCE_EDGE
        || u64::from(width) * u64::from(height) > MAX_SOURCE_PIXELS
    {
        return Err(ChannelIconError::ImageTooLarge);
    }
    Ok(())
}

fn check_gif_frame(bytes: &[u8]) -> Result<(), ChannelIconError> {
    // A partial frame can need a separate decode buffer from the logical screen.
    let mut decoder = gif::DecodeOptions::new()
        .read_info(Cursor::new(bytes))
        .map_err(|_| ChannelIconError::InvalidImage)?;
    let frame = decoder
        .next_frame_info()
        .map_err(|_| ChannelIconError::InvalidImage)?
        .ok_or(ChannelIconError::InvalidImage)?;
    check_dimensions(u32::from(frame.width), u32::from(frame.height))
}

fn check_simple_webp(bytes: &[u8]) -> Result<(), ChannelIconError> {
    let (container, payload) = bytes
        .split_at_checked(20)
        .ok_or(ChannelIconError::InvalidImage)?;
    if &container[..4] != b"RIFF" || &container[8..12] != b"WEBP" {
        return Err(ChannelIconError::InvalidImage);
    }
    // image-webp builds lossless Huffman tables outside the configured
    // allocation limit, including for compressed alpha on a lossy frame.
    // Admit only the simple, opaque lossy container before opening the decoder.
    if &container[12..16] != b"VP8 " {
        return Err(ChannelIconError::UnsupportedFormat);
    }
    let riff_size = u32::from_le_bytes([container[4], container[5], container[6], container[7]]);
    let frame_size =
        u32::from_le_bytes([container[16], container[17], container[18], container[19]]);
    let padding = frame_size % 2;
    if bytes.len() as u64 != u64::from(riff_size) + 8
        || payload.len() as u64 != u64::from(frame_size) + u64::from(padding)
        || (padding == 1 && payload.last() != Some(&0))
    {
        return Err(ChannelIconError::InvalidImage);
    }
    let frame = &payload[..frame_size as usize];
    let header = frame.get(..10).ok_or(ChannelIconError::InvalidImage)?;
    if header[0] & 1 != 0 || header[3..6] != [0x9d, 0x01, 0x2a] {
        return Err(ChannelIconError::InvalidImage);
    }
    let width = u32::from(u16::from_le_bytes([header[6], header[7]]) & 0x3fff);
    let height = u32::from(u16::from_le_bytes([header[8], header[9]]) & 0x3fff);
    check_dimensions(width, height)?;
    let first_partition = u32::from_le_bytes([header[0], header[1], header[2], 0]) >> 5;
    if first_partition == 0 || first_partition as usize > frame.len() - header.len() {
        return Err(ChannelIconError::InvalidImage);
    }
    Ok(())
}

fn image_error(error: ImageError) -> ChannelIconError {
    if matches!(error, ImageError::Limits(_)) {
        ChannelIconError::ImageTooLarge
    } else {
        ChannelIconError::InvalidImage
    }
}
