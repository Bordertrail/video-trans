use crate::{
    media::{AppError, AppResult, Media, Plan, Settings, Tools},
    output::Destination,
    watermark::{self, Overlay},
};
use serde::Serialize;
use std::{
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tempfile::Builder;

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    Running,
    Progress { percent: Option<u8> },
    Finalizing,
    Succeeded { path: PathBuf, bytes: u64 },
    Cancelled,
}

#[derive(Default)]
enum Phase {
    #[default]
    Active,
    Cancelled,
    Finalizing,
}

#[derive(Default)]
pub struct Control(Mutex<Phase>);

impl Control {
    pub fn cancel(&self) -> bool {
        let mut phase = self.0.lock().unwrap();
        if matches!(*phase, Phase::Finalizing) {
            return false;
        }
        *phase = Phase::Cancelled;
        true
    }

    fn cancelled(&self) -> bool {
        matches!(*self.0.lock().unwrap(), Phase::Cancelled)
    }

    fn finalize(&self) -> bool {
        let mut phase = self.0.lock().unwrap();
        if matches!(*phase, Phase::Cancelled) {
            return false;
        }
        *phase = Phase::Finalizing;
        true
    }
}

fn missing_binary(error: std::io::Error) -> AppError {
    AppError::new("MissingBinary", "无法启动 FFmpeg。").diagnostic(error.to_string())
}

fn encoder_error(stderr: &[u8]) -> AppError {
    let detail = String::from_utf8_lossy(stderr);
    let lower = detail.to_ascii_lowercase();
    let (code, message) =
        if lower.contains("no space left on device") || lower.contains("disk quota exceeded") {
            ("DiskFull", "磁盘空间不足。")
        } else if lower.contains("permission denied") {
            ("PermissionDenied", "没有权限读取文件或写入目录。")
        } else {
            ("EncoderFailed", "FFmpeg 无法处理这段视频或生成输出文件。")
        };
    AppError::new(code, message).diagnostic(detail.trim())
}

struct ProcessOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn process(
    command: &mut Command,
    cancelled: impl Fn() -> bool,
    timeout: Option<Duration>,
    read_stdout: impl FnOnce(std::process::ChildStdout) -> std::io::Result<Vec<u8>> + Send + 'static,
) -> AppResult<Option<ProcessOutput>> {
    if cancelled() {
        return Ok(None);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(missing_binary)?;
    let stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let stdout_reader = thread::spawn(move || read_stdout(stdout));
    let stderr_reader = thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let mut retained = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let count = stderr.read(&mut buffer)?;
            if count == 0 {
                return Ok(retained);
            }
            retained.extend_from_slice(&buffer[..count]);
            if retained.len() > 24_000 {
                retained.drain(..retained.len() - 24_000);
            }
        }
    });
    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        timed_out |= timeout.is_some_and(|limit| started.elapsed() > limit);
        if cancelled() || timed_out {
            let _ = child.kill();
            break child.wait();
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => thread::sleep(Duration::from_millis(35)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(error);
            }
        }
    };
    let stdout = stdout_reader
        .join()
        .expect("FFmpeg stdout reader panicked")
        .map_err(AppError::io)?;
    let stderr = stderr_reader
        .join()
        .expect("FFmpeg stderr reader panicked")
        .map_err(AppError::io)?;
    let status = status.map_err(AppError::io)?;
    if cancelled() {
        return Ok(None);
    }
    if timed_out {
        return Err(AppError::new("PreviewFailed", "生成预览超时，请重试。"));
    }
    Ok(Some(ProcessOutput {
        status,
        stdout,
        stderr,
    }))
}

fn video_command(
    tools: &Tools,
    media: &Media,
    seek: Option<f64>,
    watermark_path: Option<&Path>,
) -> Command {
    let mut command = Tools::command(&tools.ffmpeg);
    command.args([
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "warning",
        "-filter_complex_threads",
        "1",
    ]);
    if let Some(seek) = seek {
        command.args(["-ss", &format!("{seek:.4}")]);
    }
    command
        .args(Tools::input_options())
        .arg("-i")
        .arg(&media.path);
    if let Some(path) = watermark_path {
        command.args(["-f", "png_pipe", "-i"]).arg(path);
    }
    command
}

fn filters(media: &Media, plan: Plan, overlay: Option<&Overlay>, preview: bool) -> String {
    let mut graph = format!("[0:{}]{}", media.video, plan.video_filter());
    if let Some(overlay) = overlay {
        graph.push_str(&format!(
            "[base];[base][1:v]overlay={}:{}:eof_action=repeat:format=yuv444",
            overlay.x, overlay.y
        ));
    }
    if preview {
        graph.push_str(
            ",scale=w='min(960,iw)':h='min(960,ih)':force_original_aspect_ratio=decrease",
        );
    }
    graph.push_str("[video]");
    graph
}

pub struct Preview {
    pub frame: Vec<u8>,
    pub overlay: Option<Overlay>,
}

impl Preview {
    pub fn into_packet(self) -> Vec<u8> {
        // Five little-endian u32 values, then the JPEG and optional watermark PNG.
        let [x, y, width, height] = self.overlay.as_ref().map_or([0; 4], |mark| {
            [mark.x, mark.y, mark.size.width, mark.size.height]
        });
        let mut packet = Vec::with_capacity(
            20 + self.frame.len() + self.overlay.as_ref().map_or(0, |mark| mark.png.len()),
        );
        for value in [self.frame.len() as u32, x, y, width, height] {
            packet.extend_from_slice(&value.to_le_bytes());
        }
        packet.extend_from_slice(&self.frame);
        if let Some(mark) = self.overlay {
            packet.extend_from_slice(&mark.png);
        }
        packet
    }
}

pub fn preview(
    tools: &Tools,
    media: &Media,
    settings: &Settings,
    cancel: &AtomicBool,
) -> AppResult<Preview> {
    let plan = Plan::new(media, settings.resolution)?;
    let overlay = watermark::prepare(tools, &settings.watermark, plan.output)?;
    let mut seek = media.duration.map(|duration| (duration * 0.1).min(5.0));
    let graph = filters(media, plan, None, true);
    loop {
        let mut command = video_command(tools, media, seek, None);
        command.arg("-filter_complex").arg(&graph).args([
            "-map",
            "[video]",
            "-frames:v",
            "1",
            "-an",
            "-c:v",
            "mjpeg",
            "-q:v",
            "3",
            "-f",
            "image2pipe",
            "pipe:1",
        ]);
        let result = process(
            &mut command,
            || cancel.load(Ordering::Relaxed),
            Some(Duration::from_secs(25)),
            |mut stdout| {
                let mut bytes = Vec::new();
                stdout.read_to_end(&mut bytes)?;
                Ok(bytes)
            },
        )?;
        let Some(ProcessOutput {
            status,
            stdout: bytes,
            stderr,
        }) = result
        else {
            return Err(AppError::new("Cancelled", "预览已取消。"));
        };
        if status.success() && !bytes.is_empty() {
            return Ok(Preview {
                frame: bytes,
                overlay,
            });
        }
        if seek.take().is_none() {
            return Err(AppError::new("PreviewFailed", "无法从此视频生成预览。")
                .diagnostic(String::from_utf8_lossy(&stderr)));
        }
    }
}

pub fn export(
    tools: &Tools,
    media: &Media,
    settings: &Settings,
    destination: Destination,
    control: &Control,
    send: impl Fn(Event) + Send + Clone + 'static,
) -> AppResult<Event> {
    let plan = Plan::new(media, settings.resolution)?;
    let directory = Builder::new()
        .prefix(".video-trans-")
        .tempdir_in(&destination.directory)
        .map_err(AppError::io)?;
    let overlay = watermark::prepare(tools, &settings.watermark, plan.output);
    if control.cancelled() {
        return Ok(Event::Cancelled);
    }
    let overlay = overlay?;

    let temporary = directory
        .path()
        .join(format!("output.{}", settings.format.extension()));
    let watermark_path = directory.path().join("watermark.png");
    if let Some(mark) = &overlay {
        std::fs::write(&watermark_path, &mark.png).map_err(AppError::io)?;
    }
    let mut command = video_command(
        tools,
        media,
        None,
        overlay.as_ref().map(|_| watermark_path.as_path()),
    );
    command
        .args(["-nostats", "-progress", "pipe:1", "-filter_complex"])
        .arg(filters(media, plan, overlay.as_ref(), false))
        .args(["-map", "[video]"]);
    if let Some(audio) = media.audio {
        command.args(["-map", &format!("0:{}", audio.index)]);
    }
    command
        .args(settings.format.encoding(media, plan.output))
        .arg(&temporary);

    let duration = media.duration;
    let progress = send.clone();
    send(Event::Running);
    send(Event::Progress {
        percent: duration.map(|_| 0),
    });
    let result = process(
        &mut command,
        || control.cancelled(),
        None,
        move |stdout| {
            let mut last = 0;
            if let Some(duration) = duration {
                for line in BufReader::new(stdout).lines() {
                    let line = line?;
                    if let Some(elapsed) = line
                        .strip_prefix("out_time_us=")
                        .and_then(|value| value.parse::<i64>().ok())
                    {
                        let percent = (elapsed as f64 / duration / 10_000.0).clamp(0.0, 99.0) as u8;
                        if percent > last {
                            last = percent;
                            progress(Event::Progress {
                                percent: Some(percent),
                            });
                        }
                    }
                }
            } else {
                std::io::copy(&mut BufReader::new(stdout), &mut std::io::sink())?;
            }
            Ok(Vec::new())
        },
    )?;
    let Some(ProcessOutput { status, stderr, .. }) = result else {
        return Ok(Event::Cancelled);
    };
    if !status.success() {
        return Err(encoder_error(&stderr));
    }
    if !control.finalize() {
        return Ok(Event::Cancelled);
    }
    send(Event::Finalizing);
    let (path, bytes) = destination.commit(Path::new(&temporary))?;
    Ok(Event::Succeeded { path, bytes })
}

pub fn playback_proxy(
    tools: &Tools,
    path: &Path,
    output: &Path,
    cancel: &AtomicBool,
) -> AppResult<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(AppError::new("Cancelled", "已取消准备播放。"));
    }
    let media = crate::media::inspect(tools, path)?;
    let scale = crate::media::scaled_size(media.display, crate::media::Resolution::P720)
        .unwrap_or(media.display);
    if scale.width > 1280 || scale.height > 1280 {
        return Err(AppError::new(
            "PlaybackFailed",
            "无法为此画面尺寸生成播放预览。",
        ));
    }
    let width = scale.width.next_multiple_of(2);
    let height = scale.height.next_multiple_of(2);
    let filter = format!("scale={width}:{height}:flags=bicubic,setsar=1");

    let mut command = video_command(tools, &media, None, None);
    command.args(["-map", &format!("0:{}", media.video), "-vf", &filter]);
    if let Some(audio) = media.audio {
        command.args([
            "-map",
            &format!("0:{}", audio.index),
            "-c:a",
            "aac",
            "-ac",
            "2",
            "-b:a",
            "128k",
        ]);
    }
    command
        .args(crate::media::h264_encoder())
        .args([
            "-pix_fmt",
            "yuv420p",
            "-b:v",
            "2200k",
            "-fps_mode",
            "passthrough",
            "-movflags",
            "+faststart",
            "-f",
            "mp4",
        ])
        .arg(output);

    let result = process(
        &mut command,
        || cancel.load(Ordering::Relaxed),
        None,
        |mut stdout| {
            std::io::copy(&mut stdout, &mut std::io::sink())?;
            Ok(Vec::new())
        },
    )?;
    match result {
        Some(result) if result.status.success() => Ok(()),
        Some(result) => Err(AppError::new("PlaybackFailed", "无法生成兼容的播放预览。")
            .diagnostic(String::from_utf8_lossy(&result.stderr))),
        None => Err(AppError::new("Cancelled", "已取消准备播放。")),
    }
}
