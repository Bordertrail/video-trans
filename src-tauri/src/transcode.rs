use crate::{
    media::{AppError, AppResult, Clip, Media, OutputFormat, Plan, Settings, SizeBudget, Tools},
    output::Destination,
    playback,
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
    Retrying,
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

fn filters(input: &str, plan: Plan, overlay: Option<&Overlay>, preview: bool) -> String {
    let mut graph = format!("{input}{}", plan.video_filter());
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

fn concat_filters(media: &Media, clips: &[Clip]) -> String {
    use std::fmt::Write as _;

    let count = clips.len();
    let mut graph = format!("[0:{}]split={count}", media.video);
    for index in 0..count {
        write!(graph, "[v{index}]").unwrap();
    }
    graph.push(';');
    if let Some(audio) = media.audio {
        // Keep source time when audio starts late or ends before the video.
        let end = clips[count - 1].end;
        write!(
            graph,
            "[0:{}]aresample=async=1:first_pts=0,apad=whole_dur={end:.6},asplit={count}",
            audio.index
        )
        .unwrap();
        for index in 0..count {
            write!(graph, "[a{index}]").unwrap();
        }
        graph.push(';');
    }
    for (index, clip) in clips.iter().enumerate() {
        let bounds = format!("start={:.6}:end={:.6}", clip.start, clip.end);
        write!(
            graph,
            "[v{index}]trim={bounds},setpts=PTS-STARTPTS[cv{index}];"
        )
        .unwrap();
        if media.audio.is_some() {
            write!(
                graph,
                "[a{index}]atrim={bounds},asetpts=PTS-STARTPTS[ca{index}];"
            )
            .unwrap();
        }
    }
    for index in 0..count {
        write!(graph, "[cv{index}]").unwrap();
        if media.audio.is_some() {
            write!(graph, "[ca{index}]").unwrap();
        }
    }
    if media.audio.is_some() {
        write!(graph, "concat=n={count}:v=1:a=1[joined][audio];").unwrap();
    } else {
        write!(graph, "concat=n={count}:v=1:a=0[joined];").unwrap();
    }
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
    let mut seek = settings
        .clips
        .first()
        .map(|clip| clip.start + (clip.duration() * 0.1).min(5.0))
        .or_else(|| media.duration.map(|duration| (duration * 0.1).min(5.0)));
    let graph = filters(&format!("[0:{}]", media.video), plan, None, true);
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
    let directory = Builder::new()
        .prefix(".video-trans-")
        .tempdir_in(&destination.directory)
        .map_err(AppError::io)?;
    let temporary = directory
        .path()
        .join(format!("output.{}", settings.format.extension()));
    if !encode(
        tools,
        media,
        settings,
        &temporary,
        || control.cancelled(),
        send.clone(),
    )? || !control.finalize()
    {
        return Ok(Event::Cancelled);
    }
    send(Event::Finalizing);
    let (path, bytes) = destination.commit(&temporary)?;
    Ok(Event::Succeeded { path, bytes })
}

pub fn preview_movie(
    tools: &Tools,
    media: &Media,
    settings: &Settings,
    output: &Path,
    cancel: &AtomicBool,
    send: impl Fn(Event) + Send + Clone + 'static,
) -> AppResult<()> {
    if !encode(
        tools,
        media,
        settings,
        output,
        || cancel.load(Ordering::Relaxed),
        send,
    )? {
        return Err(AppError::new("Cancelled", "预览已取消。"));
    }
    playback::FileKey::new(output.to_path_buf())?;
    Ok(())
}

fn encode(
    tools: &Tools,
    media: &Media,
    settings: &Settings,
    output: &Path,
    cancelled: impl Fn() -> bool,
    send: impl Fn(Event) + Send + Clone + 'static,
) -> AppResult<bool> {
    let plan = Plan::new(media, settings.resolution)?;
    let duration = settings.output_duration(media)?;
    let mut budget = SizeBudget::plan(media, settings)?;
    if cancelled() {
        return Ok(false);
    }
    let overlay = watermark::prepare(tools, &settings.watermark, plan.output)?;
    let watermark_path = output.with_file_name("watermark.png");
    if let Some(mark) = &overlay {
        std::fs::write(&watermark_path, &mark.png).map_err(AppError::io)?;
    }
    let input = format!("[0:{}]", media.video);
    let multiple = settings.clips.len() > 1;
    let single = match settings.clips.as_slice() {
        [clip] => Some(*clip),
        _ => None,
    };
    let (mut graph, video_input) = if multiple {
        (concat_filters(media, &settings.clips), "[joined]")
    } else {
        (String::new(), input.as_str())
    };
    graph.push_str(&filters(video_input, plan, overlay.as_ref(), false));
    send(Event::Running);
    for attempt in 0..3 {
        let mut command = video_command(
            tools,
            media,
            single.map(|clip| clip.start),
            overlay.as_ref().map(|_| watermark_path.as_path()),
        );
        command
            .args(["-y", "-nostats", "-progress", "pipe:1", "-filter_complex"])
            .arg(&graph)
            .args(["-map", "[video]"]);
        if let Some(audio) = media.audio {
            if multiple {
                command.args(["-map", "[audio]"]);
            } else {
                command.args(["-map", &format!("0:{}", audio.index)]);
            }
        }
        if let Some(clip) = single {
            command.args(["-t", &format!("{:.4}", clip.duration())]);
        }
        command
            .args(settings.format.encoding(media, plan.output, budget))
            .arg(output);

        let progress = send.clone();
        send(Event::Progress {
            percent: duration.map(|_| 0),
        });
        let result = process(&mut command, &cancelled, None, move |stdout| {
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
        })?;
        let Some(ProcessOutput { status, stderr, .. }) = result else {
            return Ok(false);
        };
        if !status.success() {
            return Err(encoder_error(&stderr));
        }
        if cancelled() {
            return Ok(false);
        }
        let (Some(limit), Some(rates)) = (settings.target_bytes, budget.as_mut()) else {
            break;
        };
        let actual = output.metadata().map_err(AppError::io)?.len();
        if actual <= limit {
            break;
        }
        let next = (f64::from(rates.video_kbps) * limit as f64 / actual as f64 * 0.85) as u32;
        if attempt == 2 || rates.video_kbps == SizeBudget::MIN_VIDEO_KBPS {
            return Err(AppError::new(
                "TargetSizeExceeded",
                "未能将完整视频压缩到目标大小。请调高目标大小，或选择更低的分辨率。",
            ));
        }
        rates.video_kbps = next.max(SizeBudget::MIN_VIDEO_KBPS);
        send(Event::Retrying);
    }
    Ok(true)
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
    playback_copy(tools, &media, output, None, false, cancel)
}

pub fn preview_movie_proxy(
    tools: &Tools,
    path: &Path,
    output: &Path,
    cancel: &AtomicBool,
) -> AppResult<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(AppError::new("Cancelled", "已取消准备播放。"));
    }
    let media = crate::media::inspect(tools, path)?;
    playback_copy(tools, &media, output, None, true, cancel)
}

fn playback_copy(
    tools: &Tools,
    media: &Media,
    output: &Path,
    clip: Option<Clip>,
    keep_size: bool,
    cancel: &AtomicBool,
) -> AppResult<()> {
    let scale = if keep_size {
        media.display
    } else {
        crate::media::scaled_size(media.display, crate::media::Resolution::P720)
            .unwrap_or(media.display)
    };
    if !keep_size && (scale.width > 1280 || scale.height > 1280) {
        return Err(AppError::new(
            "PlaybackFailed",
            "无法为此画面尺寸生成播放预览。",
        ));
    }
    let width = scale.width.next_multiple_of(2);
    let height = scale.height.next_multiple_of(2);
    let filter = format!("scale={width}:{height}:flags=bicubic,setsar=1");

    let mut command = video_command(tools, media, clip.map(|clip| clip.start), None);
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
    command.args(crate::media::h264_encoder()).args([
        "-pix_fmt",
        "yuv420p",
        "-b:v",
        if keep_size || clip.is_some() {
            "16000k"
        } else {
            "2200k"
        },
        "-fps_mode",
        "passthrough",
        "-movflags",
        "+faststart",
        "-f",
        "mp4",
    ]);
    if let Some(clip) = clip {
        command.args(["-t", &format!("{:.4}", clip.duration())]);
    }
    command.arg(output);

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

pub fn compression_sample(
    tools: &Tools,
    media: &Media,
    settings: &Settings,
    proxy: &crate::playback::Proxy,
    cancel: &AtomicBool,
) -> AppResult<(PathBuf, &'static str, Clip)> {
    let duration = settings
        .output_duration(media)?
        .filter(|duration| *duration > 0.0)
        .ok_or_else(|| AppError::new("InvalidClip", "需要已知的视频时长才能生成压缩样片。"))?;
    let (start, duration) = settings
        .clips
        .first()
        .map_or((0.0, duration), |clip| (clip.start, clip.duration()));
    let clip = Clip {
        start,
        end: start + duration.min(5.0),
    };
    let plan = Plan::new(media, settings.resolution)?;
    let budget = SizeBudget::plan(media, settings)?;
    let overlay = watermark::prepare(tools, &settings.watermark, plan.output)?;
    let watermark_path = proxy.file("watermark.png");
    if let Some(mark) = &overlay {
        std::fs::write(&watermark_path, &mark.png).map_err(AppError::io)?;
    }
    // MP4 and MOV share the same video and audio encoders. Use MP4 for browser playback.
    let (format, mime) = if settings.format == OutputFormat::Webm {
        (OutputFormat::Webm, "video/webm")
    } else {
        (OutputFormat::Mp4, "video/mp4")
    };
    let output = proxy.file(&format!("sample.{}", format.extension()));
    let mut command = video_command(
        tools,
        media,
        Some(start),
        overlay.as_ref().map(|_| watermark_path.as_path()),
    );
    command.args([
        "-filter_complex",
        &filters(
            &format!("[0:{}]", media.video),
            plan,
            overlay.as_ref(),
            false,
        ),
        "-map",
        "[video]",
    ]);
    if let Some(audio) = media.audio {
        command.args(["-map", &format!("0:{}", audio.index)]);
    }
    command
        .args(["-t", &format!("{:.4}", clip.duration())])
        .args(format.encoding(media, plan.output, budget))
        .arg(&output);
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
        Some(result) if result.status.success() => {}
        Some(result) => return Err(encoder_error(&result.stderr)),
        None => return Err(AppError::new("Cancelled", "预览已取消。")),
    }
    playback_copy(tools, media, &proxy.path(), Some(clip), false, cancel)?;
    Ok((output, mime, clip))
}
