use crate::media::{AppError, AppResult, Position, Size, Tools, Watermark};
use ab_glyph::{point, Font, FontArc, PxScale, ScaleFont};
use image::{
    imageops::FilterType, ImageDecoder, ImageEncoder, ImageFormat, ImageReader, Rgba, RgbaImage,
};
use std::{fs, path::Path};

pub struct Overlay {
    pub png: Vec<u8>,
    pub size: Size,
    pub x: u32,
    pub y: u32,
}

fn invalid(message: &str) -> AppError {
    AppError::new("InvalidWatermark", message)
}

fn image_error(error: image::ImageError) -> AppError {
    match error {
        image::ImageError::IoError(error) => AppError::io(error),
        error => invalid("无法读取或生成水印图片。").diagnostic(error.to_string()),
    }
}

fn color(rgb: &str) -> AppResult<[u8; 3]> {
    let hex = rgb
        .strip_prefix('#')
        .filter(|hex| hex.len() == 6 && hex.is_ascii())
        .ok_or_else(|| invalid("请选择有效的文字颜色。"))?;
    let value = u32::from_str_radix(hex, 16).map_err(|_| invalid("请选择有效的文字颜色。"))?;
    Ok([(value >> 16) as u8, (value >> 8) as u8, value as u8])
}

fn margin(frame: Size, position: Position) -> u32 {
    match position {
        Position::Center | Position::Custom { .. } => 0,
        _ => (f64::from(frame.width.min(frame.height)) * 0.02)
            .round()
            .max(4.0) as u32,
    }
}

fn bounds(frame: Size, size: Size, margin: u32) -> AppResult<()> {
    if size.width.saturating_add(2 * margin) > frame.width
        || size.height.saturating_add(2 * margin) > frame.height
    {
        return Err(invalid("水印超出了画面，请减小字号或图片宽度。"));
    }
    Ok(())
}

fn opacity(value: u8) -> AppResult<f32> {
    if !(10..=100).contains(&value) {
        return Err(invalid("透明度需在 10% 到 100% 之间。"));
    }
    Ok(f32::from(value) / 100.0)
}

fn text_image(
    tools: &Tools,
    text: &str,
    font_size: u32,
    rgb: &str,
    alpha: f32,
    frame: Size,
    margin: u32,
) -> AppResult<RgbaImage> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > 80 || text.chars().any(char::is_control) {
        return Err(invalid("请输入 1 到 80 个字符的单行水印文字。"));
    }
    if !(12..=200).contains(&font_size) {
        return Err(invalid("字号需在 12 到 200 之间。"));
    }
    let [r, g, b] = color(rgb)?;
    let font_path = tools
        .font
        .as_ref()
        .ok_or_else(|| invalid("找不到中文字库。"))?;
    let font = FontArc::try_from_vec(fs::read(font_path).map_err(AppError::io)?)
        .map_err(|_| invalid("无法加载水印字体。"))?;
    let scale = PxScale::from(font_size as f32);
    let scaled = font.as_scaled(scale);
    let mut advance = 0.0;
    let mut previous = None;
    let mut glyphs = Vec::new();
    for ch in text.chars() {
        let id = scaled.glyph_id(ch);
        if id.0 == 0 {
            return Err(invalid("当前字体缺少水印中的字符，请修改文字。"));
        }
        if let Some(previous) = previous {
            advance += scaled.kern(previous, id);
        }
        if let Some(glyph) =
            font.outline_glyph(id.with_scale_and_position(scale, point(advance, 0.0)))
        {
            glyphs.push(glyph);
        }
        advance += scaled.h_advance(id);
        previous = Some(id);
    }
    let mut boxes = glyphs.iter().map(|glyph| glyph.px_bounds());
    let first = boxes
        .next()
        .ok_or_else(|| invalid("请输入可见的水印文字。"))?;
    let extent = boxes.fold(first, |bounds, rect| ab_glyph::Rect {
        min: point(bounds.min.x.min(rect.min.x), bounds.min.y.min(rect.min.y)),
        max: point(bounds.max.x.max(rect.max.x), bounds.max.y.max(rect.max.y)),
    });
    let size = Size {
        width: extent.width() as u32,
        height: extent.height() as u32,
    };
    bounds(frame, size, margin)?;

    let mut bitmap = RgbaImage::new(size.width, size.height);
    for glyph in glyphs {
        let rect = glyph.px_bounds();
        let x = (rect.min.x - extent.min.x) as u32;
        let y = (rect.min.y - extent.min.y) as u32;
        glyph.draw(|dx, dy, coverage| {
            let pixel = bitmap.get_pixel_mut(x + dx, y + dy);
            let coverage = coverage.clamp(0.0, 1.0) * alpha;
            let combined = coverage + (f32::from(pixel[3]) / 255.0) * (1.0 - coverage);
            *pixel = Rgba([r, g, b, (combined * 255.0).round() as u8]);
        });
    }
    Ok(bitmap)
}

fn photo_image(
    path: &Path,
    width: u8,
    alpha: f32,
    frame: Size,
    margin: u32,
) -> AppResult<RgbaImage> {
    if !(5..=60).contains(&width) {
        return Err(invalid("图片宽度需在画面宽度的 5% 到 60% 之间。"));
    }
    let metadata = fs::metadata(path).map_err(AppError::io)?;
    if !metadata.is_file() || metadata.len() > 32 * 1024 * 1024 {
        return Err(invalid("请选择不超过 32 MB 的本地 PNG 或 JPEG 图片。"));
    }
    let mut reader = ImageReader::open(path)
        .map_err(AppError::io)?
        .with_guessed_format()
        .map_err(AppError::io)?;
    if !matches!(reader.format(), Some(ImageFormat::Png | ImageFormat::Jpeg)) {
        return Err(invalid("水印图片只支持 PNG 或 JPEG。"));
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(image_error)?;
    let orientation = decoder.orientation().map_err(image_error)?;
    let mut decoded = image::DynamicImage::from_decoder(decoder).map_err(image_error)?;
    decoded.apply_orientation(orientation);
    let width = (frame.width * u32::from(width) / 100).max(1);
    let height = (f64::from(width) * f64::from(decoded.height()) / f64::from(decoded.width()))
        .round()
        .max(1.0) as u32;
    bounds(frame, Size { width, height }, margin)?;
    let mut bitmap = decoded
        .resize_exact(width, height, FilterType::Lanczos3)
        .into_rgba8();
    for pixel in bitmap.pixels_mut() {
        pixel[3] = (f32::from(pixel[3]) * alpha).round() as u8;
    }
    Ok(bitmap)
}

pub fn prepare(tools: &Tools, watermark: &Watermark, frame: Size) -> AppResult<Option<Overlay>> {
    let (image, position, margin) = match watermark {
        Watermark::Off => return Ok(None),
        Watermark::Text {
            text,
            font_size,
            color,
            opacity: value,
            position,
        } => {
            let margin = margin(frame, *position);
            (
                text_image(
                    tools,
                    text,
                    *font_size,
                    color,
                    opacity(*value)?,
                    frame,
                    margin,
                )?,
                *position,
                margin,
            )
        }
        Watermark::Image {
            path,
            width_percent,
            opacity: value,
            position,
        } => {
            let margin = margin(frame, *position);
            (
                photo_image(path, *width_percent, opacity(*value)?, frame, margin)?,
                *position,
                margin,
            )
        }
    };
    let size = Size {
        width: image.width(),
        height: image.height(),
    };
    let (x, y) = position.coordinates(frame, size, margin);
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(
            &image,
            size.width,
            size.height,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(image_error)?;
    Ok(Some(Overlay { png, size, x, y }))
}
