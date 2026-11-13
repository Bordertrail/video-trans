use crate::{
    media::{inspect, Clip, OutputFormat, Position, Resolution, Settings, Size, Tools, Watermark},
    output::Destination,
    transcode::{self, Control, Event},
};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{atomic::AtomicBool, Arc},
};

fn command(program: &Path, args: &[&str]) -> Vec<u8> {
    let result = Command::new(program).args(args).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}

fn tools() -> Tools {
    let path =
        |variable| PathBuf::from(std::env::var_os(variable).expect("provide the media tools"));
    Tools {
        ffmpeg: path("VIDEO_TRANS_FFMPEG"),
        ffprobe: path("VIDEO_TRANS_FFPROBE"),
        font: Some(path("VIDEO_TRANS_FONT")),
    }
}

#[test]
#[ignore = "requires the bundled FFmpeg, FFprobe and Chinese font"]
fn size_limited_export_keeps_the_full_video() {
    let tools = tools();
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.mp4");
    command(
        &tools.ffmpeg,
        &[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=1280x720:rate=30",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440",
            "-t",
            "8",
            "-c:v",
            "mpeg4",
            "-q:v",
            "2",
            "-c:a",
            "aac",
            source.to_str().unwrap(),
        ],
    );
    let media = inspect(&tools, &source).unwrap();
    assert!(media.bytes > 1_000_000);
    for format in [OutputFormat::Mp4, OutputFormat::Mov, OutputFormat::Webm] {
        let settings = Settings {
            media_id: 1,
            format,
            resolution: Resolution::P480,
            target_bytes: Some(1_000_000),
            clips: Vec::new(),
            watermark: Watermark::Off,
        };
        let output = directory
            .path()
            .join(format!("limited.{}", format.extension()));
        let destination = Destination::prepare(&source, &output, format, false).unwrap();
        let event = transcode::export(
            &tools,
            &media,
            &settings,
            destination,
            &Control::default(),
            |_| {},
        )
        .unwrap();
        assert!(matches!(
            event,
            Event::Succeeded {
                bytes: 1..=1_000_000,
                ..
            }
        ));
        let result = inspect(&tools, &output).unwrap();
        assert_eq!(result.display.height, 480);
        assert!(result.audio.is_some());
        assert!((result.duration.unwrap() - media.duration.unwrap()).abs() < 0.1);
    }

    let settings = Settings {
        media_id: 1,
        format: OutputFormat::Webm,
        resolution: Resolution::P480,
        target_bytes: Some(1_000_000),
        clips: vec![Clip {
            start: 2.0,
            end: 5.0,
        }],
        watermark: Watermark::Text {
            text: "Clip".into(),
            font_size: 32,
            color: "#ffffff".into(),
            opacity: 80,
            position: Position::BottomRight,
        },
    };
    let output = directory.path().join("trimmed.webm");
    let destination = Destination::prepare(&source, &output, settings.format, false).unwrap();
    assert!(matches!(
        transcode::export(
            &tools,
            &media,
            &settings,
            destination,
            &Control::default(),
            |_| {}
        )
        .unwrap(),
        Event::Succeeded {
            bytes: 1..=1_000_000,
            ..
        }
    ));
    let result = inspect(&tools, &output).unwrap();
    assert!((result.duration.unwrap() - 3.0).abs() < 0.1);
    assert!(result.audio.is_some());
    let proxy = crate::playback::Proxy::new().unwrap();
    let (sample, mime, range) =
        transcode::compression_sample(&tools, &media, &settings, &proxy, &AtomicBool::new(false))
            .unwrap();
    assert_eq!((mime, range.start, range.end), ("video/webm", 2.0, 5.0));
    for path in [sample, proxy.path()] {
        let sample = inspect(&tools, &path).unwrap();
        assert!((sample.duration.unwrap() - 3.0).abs() < 0.1);
        assert!(sample.audio.is_some());
    }
    let mut settings = settings;
    settings.clips.push(Clip {
        start: 6.0,
        end: 7.0,
    });
    let preview = proxy.file("preview.webm");
    let cancel = AtomicBool::new(false);
    transcode::preview_movie(&tools, &media, &settings, &preview, &cancel, |_| {}).unwrap();
    let result = inspect(&tools, &preview).unwrap();
    assert!(result.bytes <= 1_000_000);
    assert_eq!(result.display.height, 480);
    assert!((result.duration.unwrap() - 4.0).abs() < 0.1);
    assert!(result.audio.is_some());
    let compatible = proxy.file("compatible.mp4");
    transcode::preview_movie_proxy(&tools, &preview, &compatible, &cancel).unwrap();
    let result = inspect(&tools, &compatible).unwrap();
    assert_eq!(result.display.height, 480);
    assert!((result.duration.unwrap() - 4.0).abs() < 0.1);
    assert!(result.audio.is_some());
}

#[test]
#[ignore = "requires the bundled FFmpeg, FFprobe and Chinese font"]
fn joins_only_the_selected_ranges_in_source_order() {
    let tools = tools();
    let directory = tempfile::tempdir().unwrap();
    for with_audio in [true, false] {
        let source = directory.path().join(format!("source-{with_audio}.mp4"));
        let mut args = vec![
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=red:s=160x96:r=30:d=2[a];color=green:s=160x96:r=30:d=2[b];color=blue:s=160x96:r=30:d=2[c];[a][b][c]concat=n=3:v=1:a=0",
        ];
        if with_audio {
            // The audio ends before the second selection starts.
            args.extend([
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=2",
                "-c:a",
                "aac",
            ]);
        }
        args.extend(["-c:v", "mpeg4", source.to_str().unwrap()]);
        command(&tools.ffmpeg, &args);
        let media = inspect(&tools, &source).unwrap();
        let mut settings = Settings {
            media_id: 1,
            format: OutputFormat::Mp4,
            resolution: Resolution::Original,
            target_bytes: None,
            clips: vec![
                Clip {
                    start: 0.5,
                    end: 1.5,
                },
                Clip {
                    start: 4.5,
                    end: 5.5,
                },
            ],
            watermark: Watermark::Text {
                text: "Joined".into(),
                font_size: 16,
                color: "#ffffff".into(),
                opacity: 80,
                position: Position::BottomRight,
            },
        };
        assert_eq!(settings.output_duration(&media).unwrap(), Some(2.0));
        let output = directory.path().join(format!("joined-{with_audio}.mp4"));
        let destination = Destination::prepare(&source, &output, settings.format, false).unwrap();
        assert!(matches!(
            transcode::export(
                &tools,
                &media,
                &settings,
                destination,
                &Control::default(),
                |_| {}
            )
            .unwrap(),
            Event::Succeeded { bytes: 1.., .. }
        ));
        let result = inspect(&tools, &output).unwrap();
        assert!((result.duration.unwrap() - 2.0).abs() < 0.1);
        assert_eq!(result.audio.is_some(), with_audio);
        let frames = command(
            &tools.ffmpeg,
            &[
                "-v",
                "error",
                "-i",
                output.to_str().unwrap(),
                "-vf",
                "crop=2:2:10:10",
                "-pix_fmt",
                "rgb24",
                "-fps_mode",
                "passthrough",
                "-f",
                "rawvideo",
                "pipe:1",
            ],
        );
        assert_eq!(frames.len(), 60 * 12);
        for (index, frame) in frames.chunks_exact(12).enumerate() {
            let [r, g, b] = frame[..3] else {
                unreachable!()
            };
            assert!(if index < 30 {
                r > 200 && g < 25 && b < 25
            } else {
                b > 200 && r < 25 && g < 25
            });
        }
        settings.clips[1].start = 1.0;
        assert!(settings.output_duration(&media).is_err());
    }
}

#[test]
#[ignore = "requires the bundled FFmpeg, FFprobe and Chinese font"]
fn media_pipeline() {
    let tools = tools();
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("横屏 sample.mov");
    command(
        &tools.ffmpeg,
        &[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:size=1920x1080:rate=24",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-t",
            "2",
            "-c:v",
            "mpeg4",
            "-c:a",
            "aac",
            source.to_str().unwrap(),
        ],
    );
    let media = inspect(&tools, &source).unwrap();
    assert_eq!(
        media.display,
        Size {
            width: 1920,
            height: 1080
        }
    );

    let image = directory.path().join("transparent logo.png");
    image::RgbaImage::from_fn(100, 40, |x, _| {
        if x < 35 {
            image::Rgba([0, 0, 0, 0])
        } else {
            image::Rgba([255, 0, 0, 220])
        }
    })
    .save(&image)
    .unwrap();
    let text = Watermark::Text {
        text: "中文 100%".into(),
        font_size: 100,
        color: "#ffffff".into(),
        opacity: 80,
        position: Position::BottomRight,
    };
    let logo = Watermark::Image {
        path: image,
        width_percent: 30,
        opacity: 100,
        position: Position::Custom { x: 0.5, y: 0.5 },
    };

    for (format, video, audio, watermark) in [
        (OutputFormat::Mp4, "h264", "aac", text.clone()),
        (OutputFormat::Webm, "vp9", "opus", logo),
        (OutputFormat::Mov, "h264", "aac", Watermark::Off),
    ] {
        let settings = Settings {
            media_id: 1,
            format,
            resolution: Resolution::P720,
            target_bytes: None,
            clips: Vec::new(),
            watermark,
        };
        let preview =
            transcode::preview(&tools, &media, &settings, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            image::load_from_memory(&preview.frame)
                .unwrap()
                .into_rgb8()
                .dimensions(),
            (960, 540)
        );
        if matches!(settings.watermark, Watermark::Image { .. }) {
            let mark = preview.overlay.as_ref().unwrap();
            assert_eq!(
                (mark.x, mark.y, mark.size.width, mark.size.height),
                (448, 283, 384, 154)
            );
            let packet = preview.into_packet();
            let jpeg_len = u32::from_le_bytes(packet[..4].try_into().unwrap()) as usize;
            assert_eq!(&packet[20 + jpeg_len..][..8], b"\x89PNG\r\n\x1a\n");
        }
        let output = directory
            .path()
            .join(format!("输出成品.{}", format.extension()));
        let destination = Destination::prepare(&source, &output, format, false).unwrap();
        assert!(matches!(
            transcode::export(
                &tools,
                &media,
                &settings,
                destination,
                &Control::default(),
                |_| {}
            )
            .unwrap(),
            Event::Succeeded { .. }
        ));
        let data = command(
            &tools.ffprobe,
            &[
                "-v",
                "error",
                "-show_entries",
                "stream=codec_name,width,height",
                "-of",
                "json",
                output.to_str().unwrap(),
            ],
        );
        let data: serde_json::Value = serde_json::from_slice(&data).unwrap();
        assert_eq!(data["streams"][0]["codec_name"], video);
        assert_eq!(data["streams"][0]["width"], 1280);
        assert_eq!(data["streams"][0]["height"], 720);
        assert_eq!(data["streams"][1]["codec_name"], audio);

        let pixels = command(
            &tools.ffmpeg,
            &[
                "-v",
                "error",
                "-i",
                output.to_str().unwrap(),
                "-frames:v",
                "1",
                "-c:v",
                "png",
                "-f",
                "image2pipe",
                "pipe:1",
            ],
        );
        let pixels = image::load_from_memory(&pixels).unwrap().into_rgb8();
        match settings.watermark {
            Watermark::Text { .. } => assert!(pixels
                .enumerate_pixels()
                .any(|(x, y, pixel)| x > 900 && y > 600 && pixel[0] > 120)),
            Watermark::Image { .. } => {
                assert!(pixels.get_pixel(630, 320)[0] > 100);
                assert!(pixels.get_pixel(470, 320)[0] < 30);
                assert!(pixels.get_pixel(630, 265)[0] < 30);
            }
            Watermark::Off => {}
        }
    }

    let proxy = directory.path().join("playback.mp4");
    transcode::playback_proxy(&tools, &source, &proxy, &AtomicBool::new(false)).unwrap();
    let proxy_data = command(
        &tools.ffprobe,
        &[
            "-v",
            "error",
            "-show_entries",
            "stream=codec_name,width,height",
            "-of",
            "json",
            proxy.to_str().unwrap(),
        ],
    );
    let proxy_data: serde_json::Value = serde_json::from_slice(&proxy_data).unwrap();
    assert_eq!(proxy_data["streams"][0]["codec_name"], "h264");
    assert_eq!(proxy_data["streams"][0]["width"], 1280);
    assert_eq!(proxy_data["streams"][0]["height"], 720);
    assert_eq!(proxy_data["streams"][1]["codec_name"], "aac");

    let vertical = directory.path().join("竖屏.mov");
    command(
        &tools.ffmpeg,
        &[
            "-v",
            "error",
            "-display_rotation",
            "90",
            "-i",
            source.to_str().unwrap(),
            "-map",
            "0:v",
            "-c",
            "copy",
            vertical.to_str().unwrap(),
        ],
    );
    let vertical = inspect(&tools, &vertical).unwrap();
    assert_eq!(
        vertical.display,
        Size {
            width: 1080,
            height: 1920
        }
    );
    assert!(vertical.audio.is_none());
    let settings = Settings {
        media_id: 2,
        format: OutputFormat::Webm,
        target_bytes: None,
        clips: Vec::new(),
        resolution: Resolution::P720,
        watermark: text,
    };
    let preview =
        transcode::preview(&tools, &vertical, &settings, &AtomicBool::new(false)).unwrap();
    assert_eq!(
        image::load_from_memory(&preview.frame).unwrap().height(),
        960
    );

    let output = directory.path().join("protected.webm");
    std::fs::write(&output, b"original").unwrap();
    let destination = Destination::prepare(&vertical.path, &output, settings.format, true).unwrap();
    let control = Arc::new(Control::default());
    let from_progress = control.clone();
    let event = transcode::export(
        &tools,
        &vertical,
        &settings,
        destination,
        &control,
        move |event| {
            if matches!(event, Event::Progress { percent: Some(1..) }) {
                from_progress.cancel();
            }
        },
    )
    .unwrap();
    assert!(matches!(event, Event::Cancelled));
    assert_eq!(std::fs::read(&output).unwrap(), b"original");
    assert!(std::fs::read_dir(directory.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".video-trans-")
    }));
}

#[test]
fn exclusive_commit_preserves_a_new_file() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.mp4");
    let output = directory.path().join("output.mp4");
    let temp = directory.path().join("temporary.mp4");
    std::fs::write(&source, b"source").unwrap();
    std::fs::write(&temp, b"new").unwrap();
    let destination = Destination::prepare(&source, &output, OutputFormat::Mp4, false).unwrap();
    std::fs::write(&output, b"existing").unwrap();
    assert!(destination.commit(&temp).is_err());
    assert_eq!(std::fs::read(output).unwrap(), b"existing");
}

#[test]
fn custom_position_stays_inside_the_frame() {
    let frame = Size {
        width: 720,
        height: 1280,
    };
    let mark = Size {
        width: 90,
        height: 50,
    };
    let position: Position = serde_json::from_str(r#"{"x": -0.1, "y": 1.1}"#).unwrap();
    assert_eq!(position.coordinates(frame, mark, 0), (0, 1230));
    let preset: Position = serde_json::from_str(r#""bottomRight""#).unwrap();
    assert_eq!(preset.coordinates(frame, mark, 14), (616, 1216));
}
