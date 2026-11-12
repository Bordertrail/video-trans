use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    code: &'static str,
    message: String,
    detail: Option<String>,
}

impl AppError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
        }
    }

    pub fn diagnostic(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn io(error: std::io::Error) -> Self {
        use std::io::ErrorKind;
        let (code, message) = match error.kind() {
            ErrorKind::StorageFull | ErrorKind::QuotaExceeded => ("DiskFull", "磁盘空间不足。"),
            ErrorKind::PermissionDenied => ("PermissionDenied", "没有权限读取文件或写入目录。"),
            _ => ("FileError", "无法访问文件或目录。"),
        };
        Self::new(code, message).diagnostic(error.to_string())
    }
}

#[derive(Clone)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub font: Option<PathBuf>,
}

impl Tools {
    pub fn command(path: &Path) -> Command {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
            let mut command = Command::new(path);
            command.creation_flags(CREATE_NO_WINDOW);
            command
        }
        #[cfg(not(windows))]
        Command::new(path)
    }

    pub fn input_options() -> [&'static str; 4] {
        [
            "-protocol_whitelist",
            "file,pipe",
            "-format_whitelist",
            "mov,matroska,avi,mpegts,mpeg,flv,ogg,asf",
        ]
    }
}

pub fn h264_encoder() -> [&'static str; 4] {
    #[cfg(target_os = "macos")]
    return ["-c:v", "h264_videotoolbox", "-allow_sw", "1"];
    #[cfg(not(target_os = "macos"))]
    ["-c:v", "libopenh264", "-threads", "0"]
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Mp4,
    Webm,
    Mov,
}

impl OutputFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Webm => "webm",
            Self::Mov => "mov",
        }
    }

    pub fn encoding(self, media: &Media, size: Size) -> Vec<OsString> {
        let mut args = Vec::new();
        match self {
            Self::Mp4 | Self::Mov => {
                let rate = (6000.0 * f64::from(size.width) * f64::from(size.height) / 2_073_600.0
                    * media.fps.unwrap_or(30.0)
                    / 30.0)
                    .clamp(1000.0, 45000.0)
                    .round() as u32;
                args.extend(h264_encoder().map(OsString::from));
                args.extend(["-pix_fmt", "yuv420p", "-b:v"].map(OsString::from));
                args.push(format!("{rate}k").into());
                args.extend(["-movflags", "+faststart"].map(OsString::from));
            }
            Self::Webm => args.extend(
                [
                    "-c:v",
                    "libvpx-vp9",
                    "-pix_fmt",
                    "yuv420p",
                    "-crf",
                    "32",
                    "-b:v",
                    "0",
                    "-deadline",
                    "good",
                    "-cpu-used",
                    "4",
                ]
                .map(OsString::from),
            ),
        }
        if let Some(audio) = media.audio {
            let (codec, rate) = if self == Self::Webm {
                ("libopus", "128k")
            } else {
                ("aac", "160k")
            };
            args.extend(["-c:a", codec, "-b:a", rate].map(OsString::from));
            if audio.channels != Some(1) {
                args.extend(["-ac", "2"].map(OsString::from));
            }
        }
        args.extend(["-fps_mode", "passthrough", "-f", self.extension()].map(OsString::from));
        args
    }
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Resolution {
    Original,
    #[serde(rename = "1080p")]
    P1080,
    #[serde(rename = "720p")]
    P720,
    #[serde(rename = "480p")]
    P480,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Position {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Center,
    #[serde(untagged)]
    Custom {
        x: f64,
        y: f64,
    },
}

impl Position {
    pub fn coordinates(self, frame: Size, mark: Size, margin: u32) -> (u32, u32) {
        let right = frame.width - mark.width - margin;
        let bottom = frame.height - mark.height - margin;
        match self {
            Self::TopLeft => (margin, margin),
            Self::TopRight => (right, margin),
            Self::BottomLeft => (margin, bottom),
            Self::BottomRight => (right, bottom),
            Self::Center => (
                (frame.width - mark.width) / 2,
                (frame.height - mark.height) / 2,
            ),
            Self::Custom { x, y } => (
                (x.clamp(0.0, 1.0) * f64::from(frame.width - mark.width)).round() as u32,
                (y.clamp(0.0, 1.0) * f64::from(frame.height - mark.height)).round() as u32,
            ),
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum Watermark {
    Off,
    Text {
        text: String,
        font_size: u32,
        color: String,
        opacity: u8,
        position: Position,
    },
    Image {
        path: PathBuf,
        width_percent: u8,
        opacity: u8,
        position: Position,
    },
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub media_id: u32,
    pub format: OutputFormat,
    pub resolution: Resolution,
    pub watermark: Watermark,
}

#[derive(Clone, Copy)]
pub struct Audio {
    pub index: u32,
    pub channels: Option<u32>,
}

#[derive(Clone)]
pub struct Media {
    pub path: PathBuf,
    pub video: u32,
    pub audio: Option<Audio>,
    pub original: Size,
    pub display: Size,
    pub duration: Option<f64>,
    pub fps: Option<f64>,
    pub bytes: u64,
    pub unsupported: Option<&'static str>,
}

#[derive(Deserialize)]
struct Probe {
    streams: Vec<Stream>,
    format: Option<Container>,
}

#[derive(Deserialize)]
struct Container {
    duration: Option<String>,
}

#[derive(Deserialize)]
struct Stream {
    index: u32,
    codec_type: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    channels: Option<u32>,
    sample_aspect_ratio: Option<String>,
    duration: Option<String>,
    avg_frame_rate: Option<String>,
    color_transfer: Option<String>,
    #[serde(default)]
    disposition: Disposition,
    #[serde(default)]
    side_data_list: Vec<SideData>,
    tags: Option<Tags>,
}

#[derive(Deserialize, Default)]
struct Disposition {
    #[serde(default)]
    default: u8,
    #[serde(default)]
    attached_pic: u8,
}

#[derive(Deserialize)]
struct SideData {
    side_data_type: Option<String>,
    rotation: Option<f64>,
}
#[derive(Deserialize)]
struct Tags {
    rotate: Option<String>,
}

fn number(value: Option<&str>) -> Option<f64> {
    value?
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0)
}

fn ratio(value: Option<&str>, delimiter: char) -> Option<f64> {
    let (a, b) = value?.split_once(delimiter)?;
    Some(number(Some(a))? / number(Some(b))?).filter(|value| value.is_finite() && *value > 0.0)
}

pub fn inspect(tools: &Tools, path: &Path) -> AppResult<Media> {
    let path = dunce::canonicalize(path).map_err(AppError::io)?;
    let metadata = fs::metadata(&path).map_err(AppError::io)?;
    if !metadata.is_file() {
        return Err(AppError::new("InvalidInput", "请选择一个本地视频文件。"));
    }

    let output = Tools::command(&tools.ffprobe)
        .args([
            "-v",
            "error",
            "-show_format",
            "-show_streams",
            "-of",
            "json",
        ])
        .args(Tools::input_options())
        .arg(&path)
        .output()
        .map_err(|err| {
            AppError::new("MissingBinary", "无法启动 FFprobe。").diagnostic(err.to_string())
        })?;
    if !output.status.success() {
        return Err(AppError::new(
            "InvalidInput",
            "无法读取此视频，文件可能损坏或格式不受支持。",
        )
        .diagnostic(String::from_utf8_lossy(&output.stderr)));
    }
    let probe: Probe = serde_json::from_slice(&output.stdout).map_err(|err| {
        AppError::new("InvalidInput", "无法读取视频信息。").diagnostic(err.to_string())
    })?;

    let video = probe
        .streams
        .iter()
        .filter(|stream| {
            stream.codec_type.as_deref() == Some("video")
                && stream.disposition.attached_pic == 0
                && stream.width.unwrap_or_default() > 0
                && stream.height.unwrap_or_default() > 0
        })
        .min_by_key(|stream| (stream.disposition.default == 0, stream.index))
        .ok_or_else(|| AppError::new("UnsupportedVideo", "文件里没有可用的视频轨道。"))?;
    let audio = probe
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("audio"))
        .min_by_key(|stream| (stream.disposition.default == 0, stream.index))
        .map(|stream| Audio {
            index: stream.index,
            channels: stream.channels,
        });
    let original = Size {
        width: video.width.unwrap(),
        height: video.height.unwrap(),
    };
    let mut display = Size {
        width: (f64::from(original.width)
            * ratio(video.sample_aspect_ratio.as_deref(), ':').unwrap_or(1.0))
        .round()
        .max(1.0) as u32,
        height: original.height,
    };
    let rotation = video
        .side_data_list
        .iter()
        .find_map(|side| side.rotation)
        .or_else(|| video.tags.as_ref()?.rotate.as_deref()?.parse().ok())
        .unwrap_or(0.0);
    let quarter_turns = (rotation / 90.0).round();
    let unsupported_rotation = (rotation - quarter_turns * 90.0).abs() > 0.1;
    if quarter_turns.rem_euclid(2.0) == 1.0 {
        std::mem::swap(&mut display.width, &mut display.height);
    }
    let hdr = matches!(
        video.color_transfer.as_deref(),
        Some("smpte2084" | "arib-std-b67")
    ) || video.side_data_list.iter().any(|side| {
        side.side_data_type.as_deref().is_some_and(|kind| {
            let kind = kind.to_ascii_lowercase();
            kind.contains("dovi") || kind.contains("dolby vision")
        })
    });
    let unsupported = if hdr {
        Some("此版本支持 SDR 导出，暂不支持 HDR 或杜比视界素材。")
    } else if unsupported_rotation {
        Some("此视频的旋转角度暂不支持。")
    } else {
        None
    };

    Ok(Media {
        bytes: metadata.len(),
        path,
        video: video.index,
        audio,
        original,
        display,
        duration: number(video.duration.as_deref())
            .or_else(|| number(probe.format.as_ref()?.duration.as_deref())),
        fps: ratio(video.avg_frame_rate.as_deref(), '/'),
        unsupported,
    })
}

pub fn scaled_size(source: Size, resolution: Resolution) -> Option<Size> {
    let (long, short) = match resolution {
        Resolution::Original => return Some(source),
        Resolution::P1080 => (1920, 1080),
        Resolution::P720 => (1280, 720),
        Resolution::P480 => (854, 480),
    };
    let (width, height) = match source.width.cmp(&source.height) {
        std::cmp::Ordering::Greater => (long, short),
        std::cmp::Ordering::Less => (short, long),
        std::cmp::Ordering::Equal => (short, short),
    };
    let factor = (f64::from(width) / f64::from(source.width))
        .min(f64::from(height) / f64::from(source.height));
    if factor >= 1.0 {
        return None;
    }
    let even = |dimension| ((f64::from(dimension) * factor / 2.0) + 1e-9).floor() as u32 * 2;
    let size = Size {
        width: even(source.width),
        height: even(source.height),
    };
    (size.width >= 2 && size.height >= 2).then_some(size)
}

#[derive(Clone, Copy)]
pub struct Plan {
    pub scale: Size,
    pub output: Size,
}

impl Plan {
    pub fn new(media: &Media, resolution: Resolution) -> AppResult<Self> {
        if let Some(reason) = media.unsupported {
            return Err(AppError::new("UnsupportedVideo", reason));
        }
        let scale = scaled_size(media.display, resolution).ok_or_else(|| {
            AppError::new("InvalidResolution", "此分辨率档位无法进一步缩小视频。")
        })?;
        if scale.width > 8192 || scale.height > 8192 {
            return Err(AppError::new(
                "InvalidResolution",
                "画面尺寸过大，请选择较低的输出分辨率。",
            ));
        }
        let output = Size {
            width: scale.width.next_multiple_of(2),
            height: scale.height.next_multiple_of(2),
        };
        Ok(Self { scale, output })
    }

    pub fn video_filter(self) -> String {
        let Size { width, height } = self.scale;
        let mut filter = format!("scale={width}:{height}:flags=lanczos");
        if self.scale != self.output {
            filter.push_str(&format!(
                ",pad={}:{}:0:0",
                self.output.width, self.output.height
            ));
        }
        filter.push_str(",setsar=1");
        filter
    }
}
