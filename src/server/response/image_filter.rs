use super::*;
use crate::config::ImageFilterMode;
use flate2::Compression;
use flate2::write::ZlibEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::webp::WebPEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageFormat};
use std::io::Write;

pub(in crate::server) enum ImageFilterOutcome {
    Complete,
    Error(u16),
}

#[derive(Debug, Clone, Copy)]
enum ImageKind {
    Jpeg,
    Gif,
    Png,
    WebP,
}

impl ImageKind {
    fn content_type(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Png => "image/png",
            Self::WebP => "image/webp",
        }
    }

    fn json_name(self) -> &'static str {
        match self {
            Self::Jpeg => "jpeg",
            Self::Gif => "gif",
            Self::Png => "png",
            Self::WebP => "webp",
        }
    }

    fn format(self) -> ImageFormat {
        match self {
            Self::Jpeg => ImageFormat::Jpeg,
            Self::Gif => ImageFormat::Gif,
            Self::Png => ImageFormat::Png,
            Self::WebP => ImageFormat::WebP,
        }
    }
}

pub(in crate::server) async fn apply_image_filter(
    response: &mut Response,
    context: &EffectiveConfig,
    variables: &Variables,
) -> ImageFilterOutcome {
    let config = &context.image_filter;
    if config.mode == ImageFilterMode::Off || response.status != 200 {
        return ImageFilterOutcome::Complete;
    }

    if response
        .header("Content-Length")
        .and_then(|value| value.trim().parse::<u64>().ok())
        .is_some_and(|length| length > config.buffer_size)
    {
        return ImageFilterOutcome::Error(415);
    }

    let body = std::mem::replace(&mut response.body, ResponseBody::Empty);
    let body = match body.into_memory(config.buffer_size).await {
        Ok(body) => body,
        Err(_) => return ImageFilterOutcome::Error(415),
    };
    let Some(kind) = image_kind(&body) else {
        if config.mode == ImageFilterMode::Size {
            set_size_response(response, None);
            return ImageFilterOutcome::Complete;
        }
        return ImageFilterOutcome::Error(415);
    };

    if config.mode == ImageFilterMode::Test {
        if image::load_from_memory_with_format(&body, kind.format()).is_err() {
            return ImageFilterOutcome::Error(415);
        }
        response.body = ResponseBody::Bytes(body);
        response.set_header("Content-Type", kind.content_type());
        response.remove_header("Accept-Ranges");
        response.disable_ranges();
        response.set_content_length();
        return ImageFilterOutcome::Complete;
    }

    if config.mode == ImageFilterMode::Size {
        set_size_response(
            response,
            image_dimensions(kind, &body).map(|dims| (kind, dims)),
        );
        return ImageFilterOutcome::Complete;
    }

    let Ok(angle) = resolved_angle(config.angle.as_deref(), variables) else {
        return ImageFilterOutcome::Error(415);
    };
    let max_width = match resolved_dimension(config.width.as_deref(), variables) {
        Ok(value) => value,
        Err(()) => return ImageFilterOutcome::Error(415),
    };
    let max_height = match resolved_dimension(config.height.as_deref(), variables) {
        Ok(value) => value,
        Err(()) => return ImageFilterOutcome::Error(415),
    };

    let Ok(mut image) = image::load_from_memory_with_format(&body, kind.format()) else {
        return ImageFilterOutcome::Error(415);
    };

    let (mut width, mut height) = image.dimensions();
    match config.mode {
        ImageFilterMode::Resize | ImageFilterMode::Crop => {
            let target = target_dimensions(config.mode, width, height, max_width, max_height);
            if target != (width, height) {
                image = image.resize_exact(target.0, target.1, FilterType::Triangle);
                width = target.0;
                height = target.1;
            }
        }
        ImageFilterMode::Rotate => {}
        ImageFilterMode::Off | ImageFilterMode::Test | ImageFilterMode::Size => {
            unreachable!("non-transform image filter mode")
        }
    }

    if angle != 0 {
        image = match angle {
            90 => image.rotate90(),
            180 => image.rotate180(),
            270 => image.rotate270(),
            _ => return ImageFilterOutcome::Error(415),
        };
        std::mem::swap(&mut width, &mut height);
        if angle == 180 {
            std::mem::swap(&mut width, &mut height);
        }
    }

    if config.mode == ImageFilterMode::Crop {
        let remove_width = u64::from(width).saturating_sub(max_width);
        let remove_height = u64::from(height).saturating_sub(max_height);
        if remove_width != 0 || remove_height != 0 {
            let crop_width = u64::from(width) - remove_width;
            let crop_height = u64::from(height) - remove_height;
            let (Ok(crop_width), Ok(crop_height), Ok(offset_x), Ok(offset_y)) = (
                u32::try_from(crop_width),
                u32::try_from(crop_height),
                u32::try_from(remove_width / 2),
                u32::try_from(remove_height / 2),
            ) else {
                return ImageFilterOutcome::Error(415);
            };
            image = image.crop_imm(offset_x, offset_y, crop_width, crop_height);
        }
    }

    if !config.transparency {
        image = DynamicImage::ImageRgb8(image.to_rgb8());
    }

    let sharpen = match resolved_percentage(config.sharpen.as_deref(), variables, 0) {
        Ok(value) => value,
        Err(()) => return ImageFilterOutcome::Error(415),
    };
    if sharpen > 0 {
        let sigma = (sharpen as f32 / 20.0).clamp(0.1, 5.0);
        image = DynamicImage::ImageRgba8(image::imageops::unsharpen(&image.to_rgba8(), sigma, 0));
    }

    let jpeg_quality = if matches!(kind, ImageKind::Jpeg) {
        match resolved_quality(config.jpeg_quality.as_deref(), variables, 75) {
            Ok(value) => value,
            Err(()) => return ImageFilterOutcome::Error(415),
        }
    } else {
        75
    };
    if matches!(kind, ImageKind::WebP)
        && resolved_quality(config.webp_quality.as_deref(), variables, 80).is_err()
    {
        return ImageFilterOutcome::Error(415);
    }
    let Ok(body) = encode_image(&image, kind, jpeg_quality, config.interlace) else {
        return ImageFilterOutcome::Error(415);
    };

    response.body = ResponseBody::Bytes(body);
    response.set_header("Content-Type", kind.content_type());
    response.remove_header("Content-Encoding");
    response.remove_header("Content-Range");
    response.remove_header("Accept-Ranges");
    response.disable_ranges();
    response.set_content_length();
    weaken_etag(response);
    ImageFilterOutcome::Complete
}

fn set_size_response(response: &mut Response, image: Option<(ImageKind, (u32, u32))>) {
    let body = match image {
        Some((kind, (width, height))) => format!(
            "{{ \"img\" : {{ \"width\": {width}, \"height\": {height}, \"type\": \"{}\" }} }}\r\n",
            kind.json_name()
        ),
        None => "{}\r\n".into(),
    };
    response.status = 200;
    response.headers.clear();
    response.trailers.clear();
    response.body = ResponseBody::Bytes(body.into_bytes());
    response.set_header("Content-Type", "application/json");
    response.disable_ranges();
    response.set_content_length();
}

fn target_dimensions(
    mode: ImageFilterMode,
    mut width: u32,
    mut height: u32,
    max_width: u64,
    max_height: u64,
) -> (u32, u32) {
    if mode == ImageFilterMode::Resize {
        if u64::from(width) > max_width {
            height = scale_dimension(height, max_width, width);
            width = dimension_to_u32(max_width);
        }
        if u64::from(height) > max_height {
            width = scale_dimension(width, max_height, height);
            height = dimension_to_u32(max_height);
        }
        return (width, height);
    }

    let width_ratio = f64::from(width) / f64::from(height);
    let target_ratio = max_width as f64 / max_height as f64;
    if width_ratio < target_ratio {
        if u64::from(width) > max_width {
            height = scale_dimension(height, max_width, width);
            width = dimension_to_u32(max_width);
        }
    } else if u64::from(height) > max_height {
        width = scale_dimension(width, max_height, height);
        height = dimension_to_u32(max_height);
    }
    (width, height)
}

fn scale_dimension(value: u32, target: u64, source: u32) -> u32 {
    let scaled = u64::from(value)
        .saturating_mul(target)
        .checked_div(u64::from(source))
        .unwrap_or(0);
    dimension_to_u32(scaled.max(1))
}

fn dimension_to_u32(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn resolved_dimension(value: Option<&str>, variables: &Variables) -> Result<u64, ()> {
    let Some(value) = value else {
        return Ok(0);
    };
    let value = eval_variables(value, variables);
    if value == "-" {
        return Ok(u64::MAX);
    }
    value
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or(())
}

fn resolved_angle(value: Option<&str>, variables: &Variables) -> Result<u16, ()> {
    let Some(value) = value else {
        return Ok(0);
    };
    match eval_variables(value, variables).as_str() {
        "90" => Ok(90),
        "180" => Ok(180),
        "270" => Ok(270),
        _ => Err(()),
    }
}

fn resolved_percentage(
    value: Option<&str>,
    variables: &Variables,
    default: u64,
) -> Result<u64, ()> {
    let Some(value) = value else {
        return Ok(default);
    };
    eval_variables(value, variables)
        .parse::<u64>()
        .ok()
        .filter(|value| *value <= 100)
        .ok_or(())
}

fn resolved_quality(value: Option<&str>, variables: &Variables, default: u8) -> Result<u8, ()> {
    let Some(value) = value else {
        return Ok(default);
    };
    let value = eval_variables(value, variables)
        .parse::<u64>()
        .map_err(|_| ())?;
    if !(1..=100).contains(&value) {
        return Err(());
    }
    Ok(value as u8)
}

fn image_kind(data: &[u8]) -> Option<ImageKind> {
    if data.len() < 16 {
        return None;
    }
    if data.starts_with(&[0xff, 0xd8]) {
        return Some(ImageKind::Jpeg);
    }
    if data.starts_with(b"GIF8") && data.get(5) == Some(&b'a') {
        return match data.get(4) {
            Some(b'7' | b'9') => Some(ImageKind::Gif),
            _ => None,
        };
    }
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(ImageKind::Png);
    }
    if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        return Some(ImageKind::WebP);
    }
    None
}

fn image_dimensions(kind: ImageKind, data: &[u8]) -> Option<(u32, u32)> {
    match kind {
        ImageKind::Jpeg => jpeg_dimensions(data),
        ImageKind::Gif => {
            let bytes = data.get(6..10)?;
            Some((
                u32::from(u16::from_le_bytes([bytes[0], bytes[1]])),
                u32::from(u16::from_le_bytes([bytes[2], bytes[3]])),
            ))
        }
        ImageKind::Png => {
            let bytes = data.get(16..24)?;
            Some((
                u32::from_be_bytes(bytes[0..4].try_into().ok()?),
                u32::from_be_bytes(bytes[4..8].try_into().ok()?),
            ))
        }
        ImageKind::WebP => webp_dimensions(data),
    }
    .filter(|(width, height)| *width != 0 && *height != 0)
}

fn jpeg_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let mut offset = 2;
    while offset + 4 <= data.len() {
        while data.get(offset) == Some(&0xff) {
            offset += 1;
        }
        let marker = *data.get(offset)?;
        offset += 1;
        if marker == 0xd9 || marker == 0xda {
            break;
        }
        if marker == 0x01 || (0xd0..=0xd8).contains(&marker) {
            continue;
        }
        let length = usize::from(u16::from_be_bytes([
            *data.get(offset)?,
            *data.get(offset + 1)?,
        ]));
        if length < 2 || offset.checked_add(length)? > data.len() {
            return None;
        }
        if matches!(
            marker,
            0xc0 | 0xc1
                | 0xc2
                | 0xc3
                | 0xc5
                | 0xc6
                | 0xc7
                | 0xc9
                | 0xca
                | 0xcb
                | 0xcd
                | 0xce
                | 0xcf
        ) {
            let height = u16::from_be_bytes([*data.get(offset + 3)?, *data.get(offset + 4)?]);
            let width = u16::from_be_bytes([*data.get(offset + 5)?, *data.get(offset + 6)?]);
            return Some((u32::from(width), u32::from(height)));
        }
        offset += length;
    }
    None
}

fn webp_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 30 || data.get(12..15)? != b"VP8" {
        return None;
    }
    match data.get(15)? {
        b' ' => {
            if data.get(20)? & 1 != 0 || data.get(23..26)? != [0x9d, 0x01, 0x2a] {
                return None;
            }
            let width = u16::from_le_bytes([*data.get(26)?, *data.get(27)?]) & 0x3fff;
            let height = u16::from_le_bytes([*data.get(28)?, *data.get(29)?]) & 0x3fff;
            Some((u32::from(width), u32::from(height)))
        }
        b'L' => {
            if data.get(20)? != &0x2f {
                return None;
            }
            let bits = u32::from_le_bytes([
                *data.get(21)?,
                *data.get(22)?,
                *data.get(23)?,
                *data.get(24)?,
            ]);
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        b'X' => {
            let width = u32::from_le_bytes([*data.get(24)?, *data.get(25)?, *data.get(26)?, 0]) + 1;
            let height =
                u32::from_le_bytes([*data.get(27)?, *data.get(28)?, *data.get(29)?, 0]) + 1;
            Some((width, height))
        }
        _ => None,
    }
}

fn encode_image(
    image: &DynamicImage,
    kind: ImageKind,
    jpeg_quality: u8,
    interlace: bool,
) -> Result<Vec<u8>, ()> {
    let mut output = Vec::new();
    match kind {
        ImageKind::Jpeg => {
            let mut encoder = JpegEncoder::new_with_quality(&mut output, jpeg_quality);
            encoder.encode_image(&image.to_rgb8()).map_err(|_| ())?;
        }
        ImageKind::Png => {
            encode_png(image, interlace, &mut output)?;
        }
        ImageKind::WebP => {
            image
                .write_with_encoder(WebPEncoder::new_lossless(&mut output))
                .map_err(|_| ())?;
        }
        ImageKind::Gif => encode_gif(image, interlace, &mut output)?,
    }
    Ok(output)
}

fn encode_png(image: &DynamicImage, interlace: bool, output: &mut Vec<u8>) -> Result<(), ()> {
    if !interlace {
        return image
            .write_with_encoder(PngEncoder::new(output))
            .map_err(|_| ());
    }

    let (width, height) = image.dimensions();
    let (color_type, channels, pixels) = if image.has_alpha() {
        let rgba = image.to_rgba8();
        (6, 4, rgba.into_raw())
    } else {
        let rgb = image.to_rgb8();
        (2, 3, rgb.into_raw())
    };

    let mut raw = Vec::new();
    for (x_start, y_start, x_step, y_step) in [
        (0, 0, 8, 8),
        (4, 0, 8, 8),
        (0, 4, 4, 8),
        (2, 0, 4, 4),
        (0, 2, 2, 4),
        (1, 0, 2, 2),
        (0, 1, 1, 2),
    ] {
        if x_start >= width {
            continue;
        }
        let mut y = y_start;
        while y < height {
            raw.push(0);
            let mut x = x_start;
            while x < width {
                let offset = (usize::try_from(y).map_err(|_| ())?
                    * usize::try_from(width).map_err(|_| ())?
                    + usize::try_from(x).map_err(|_| ())?)
                    * channels;
                raw.extend_from_slice(&pixels[offset..offset + channels]);
                x += x_step;
            }
            y += y_step;
        }
    }

    let mut zlib = ZlibEncoder::new(Vec::new(), Compression::default());
    zlib.write_all(&raw).map_err(|_| ())?;
    let compressed = zlib.finish().map_err(|_| ())?;

    output.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, color_type, 0, 0, 1]);
    write_png_chunk(output, b"IHDR", &ihdr)?;
    write_png_chunk(output, b"IDAT", &compressed)?;
    write_png_chunk(output, b"IEND", &[])
}

fn write_png_chunk(output: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) -> Result<(), ()> {
    output.extend_from_slice(&u32::try_from(data.len()).map_err(|_| ())?.to_be_bytes());
    output.extend_from_slice(kind);
    output.extend_from_slice(data);

    let mut crc = crc32fast::Hasher::new();
    crc.update(kind);
    crc.update(data);
    output.extend_from_slice(&crc.finalize().to_be_bytes());
    Ok(())
}

fn encode_gif(image: &DynamicImage, interlace: bool, output: &mut Vec<u8>) -> Result<(), ()> {
    let rgba = image.to_rgba8();
    let width = u16::try_from(rgba.width()).map_err(|_| ())?;
    let height = u16::try_from(rgba.height()).map_err(|_| ())?;
    let mut pixels = rgba.into_raw();
    let mut frame = gif::Frame::from_rgba_speed(width, height, &mut pixels, 10);
    frame.interlaced = interlace;
    let mut encoder = gif::Encoder::new(output, width, height, &[]).map_err(|_| ())?;
    encoder.write_frame(&frame).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    #[test]
    fn interlaced_png_roundtrips_odd_and_small_dimensions() {
        for (width, height) in [(1, 1), (2, 3), (7, 9), (8, 8)] {
            let source = RgbImage::from_fn(width, height, |x, y| {
                Rgb([
                    (x.wrapping_mul(37)) as u8,
                    (y.wrapping_mul(53)) as u8,
                    (x.wrapping_add(y).wrapping_mul(29)) as u8,
                ])
            });
            let mut encoded = Vec::new();
            encode_png(&DynamicImage::ImageRgb8(source.clone()), true, &mut encoded).unwrap();
            assert_eq!(encoded.get(28), Some(&1));

            let decoded = image::load_from_memory_with_format(&encoded, ImageFormat::Png)
                .unwrap_or_else(|error| panic!("{width}x{height}: {error}"));
            assert_eq!(decoded.dimensions(), (width, height));
            assert_eq!(decoded.to_rgb8(), source);
        }
    }
}
