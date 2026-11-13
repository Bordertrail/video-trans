#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod library;
mod media;
mod output;
mod playback;
mod transcode;
mod watermark;

use media::{AppError, AppResult, Media, Plan, Resolution, Settings, Size, Tools};
use output::Destination;
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
};
use tauri::{
    ipc::{Channel, Response},
    AppHandle, Emitter, Manager, State,
};
use tauri_plugin_opener::OpenerExt;
use transcode::{Control, Event};

#[derive(Default)]
struct Selection {
    request_id: u32,
    media: Option<Media>,
    preview_id: u32,
    preview_cancel: Option<Arc<AtomicBool>>,
}

struct ActiveJob {
    id: u32,
    control: Arc<Control>,
}

#[derive(Default)]
struct Playback {
    request_id: u32,
    server: Option<playback::Server>,
    preparation: Option<Arc<playback::Preparation>>,
    cached: Option<(playback::FileKey, Arc<playback::Proxy>)>,
    output_preview: Option<(Arc<playback::Proxy>, PathBuf)>,
}

impl Playback {
    fn stop(&mut self) {
        if let Some(server) = &self.server {
            server.clear();
        }
        if let Some(preparation) = &self.preparation {
            preparation.cancel();
        }
        self.output_preview = None;
    }

    fn publish(
        &mut self,
        path: PathBuf,
        mime: &'static str,
        proxy: Option<Arc<playback::Proxy>>,
    ) -> AppResult<String> {
        if self.server.is_none() {
            self.server = Some(playback::Server::start()?);
        }
        Ok(self.server.as_ref().unwrap().publish(path, mime, proxy))
    }
}

#[derive(Default)]
struct Session {
    selection: Mutex<Selection>,
    active: Mutex<Option<ActiveJob>>,
    next_job: AtomicU32,
    exit_after_job: AtomicBool,
    last_output: Mutex<Option<PathBuf>>,
    playback: Mutex<Playback>,
    workspace_playback: Mutex<Playback>,
    sample_playback: Mutex<Playback>,
    sample_reference: Mutex<Playback>,
}

impl Session {
    fn media(&self, id: u32) -> AppResult<Media> {
        let selection = self.selection.lock().unwrap();
        if selection.request_id == id {
            if let Some(media) = &selection.media {
                return Ok(media.clone());
            }
        }
        Err(AppError::new("InvalidInput", "请先选择视频。"))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MediaInfo {
    media_id: u32,
    name: String,
    path: PathBuf,
    bytes: u64,
    duration: Option<f64>,
    original: Size,
    display: Size,
    has_audio: bool,
    unsupported: Option<&'static str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportPlan {
    size: Option<Size>,
    p1080: bool,
    p720: bool,
    p480: bool,
    resolution_error: Option<AppError>,
    target_size_error: Option<AppError>,
    clip_error: Option<AppError>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum JobEvent {
    State(Event),
    Error {
        #[serde(rename = "type")]
        kind: &'static str,
        error: AppError,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JobMessage {
    job_id: u32,
    #[serde(flatten)]
    event: JobEvent,
}

fn worker_error(error: impl std::fmt::Display) -> AppError {
    AppError::new("InternalError", "处理任务时发生错误。").diagnostic(error.to_string())
}

#[tauri::command]
async fn probe_video(path: PathBuf, request_id: u32, app: AppHandle) -> AppResult<MediaInfo> {
    let session = app.state::<Session>();
    {
        let mut selection = session.selection.lock().unwrap();
        if request_id <= selection.request_id {
            return Err(AppError::new("Cancelled", "已选择其他视频。"));
        }
        if let Some(cancel) = selection.preview_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        selection.request_id = request_id;
        selection.media = None;
    }
    let tools = app.state::<Tools>().inner().clone();
    let media = tauri::async_runtime::spawn_blocking(move || media::inspect(&tools, &path))
        .await
        .map_err(worker_error)??;
    let mut selection = session.selection.lock().unwrap();
    if selection.request_id != request_id {
        return Err(AppError::new("Cancelled", "已选择其他视频。"));
    }
    let info = MediaInfo {
        media_id: request_id,
        name: media
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        path: media.path.clone(),
        bytes: media.bytes,
        duration: media.duration,
        original: media.original,
        display: media.display,
        has_audio: media.audio.is_some(),
        unsupported: media.unsupported,
    };
    selection.media = Some(media);
    Ok(info)
}

#[tauri::command]
fn get_export_plan(settings: Settings, session: State<'_, Session>) -> AppResult<ExportPlan> {
    let media = session.media(settings.media_id)?;
    let (size, resolution_error) = match Plan::new(&media, settings.resolution) {
        Ok(plan) => (Some(plan.output), None),
        Err(error) => (None, Some(error)),
    };
    Ok(ExportPlan {
        size,
        resolution_error,
        target_size_error: media::SizeBudget::plan(&media, &settings).err(),
        clip_error: settings.output_duration(&media).err(),
        p1080: Plan::new(&media, Resolution::P1080).is_ok(),
        p720: Plan::new(&media, Resolution::P720).is_ok(),
        p480: Plan::new(&media, Resolution::P480).is_ok(),
    })
}

#[tauri::command]
async fn render_preview(
    settings: Settings,
    request_id: u32,
    app: AppHandle,
) -> AppResult<Response> {
    let session = app.state::<Session>();
    let cancel = Arc::new(AtomicBool::new(false));
    let media = {
        let mut selection = session.selection.lock().unwrap();
        if settings.media_id != selection.request_id || request_id <= selection.preview_id {
            return Err(AppError::new("Cancelled", "预览设置已更新。"));
        }
        let media = selection
            .media
            .clone()
            .ok_or_else(|| AppError::new("InvalidInput", "请先选择视频。"))?;
        if let Some(previous) = selection.preview_cancel.replace(cancel.clone()) {
            previous.store(true, Ordering::Relaxed);
        }
        selection.preview_id = request_id;
        media
    };
    let tools = app.state::<Tools>().inner().clone();
    let bytes = tauri::async_runtime::spawn_blocking(move || {
        transcode::preview(&tools, &media, &settings, &cancel)
    })
    .await
    .map_err(worker_error)??;
    Ok(Response::new(bytes.into_packet()))
}

#[tauri::command]
fn start_export(
    settings: Settings,
    path: PathBuf,
    overwrite: bool,
    on_event: Channel<JobMessage>,
    app: AppHandle,
) -> AppResult<u32> {
    let session = app.state::<Session>();
    let media = session.media(settings.media_id)?;
    Plan::new(&media, settings.resolution)?;
    settings.output_duration(&media)?;
    media::SizeBudget::plan(&media, &settings)?;
    let destination = Destination::prepare(&media.path, &path, settings.format, overwrite)?;

    let id = session.next_job.fetch_add(1, Ordering::Relaxed) + 1;
    let control = Arc::new(Control::default());
    {
        let mut active = session.active.lock().unwrap();
        if active.is_some() {
            return Err(AppError::new("Busy", "当前已有转码任务。"));
        }
        *active = Some(ActiveJob {
            id,
            control: control.clone(),
        });
    }

    let tools = app.state::<Tools>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let progress = on_event.clone();
        let export_app = app.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            if let Err(error) = export_app
                .state::<library::Library>()
                .remember_export(&destination.directory)
            {
                eprintln!("Could not save export directory history: {error:?}");
            }
            transcode::export(
                &tools,
                &media,
                &settings,
                destination,
                &control,
                move |event| {
                    let _ = progress.send(JobMessage {
                        job_id: id,
                        event: JobEvent::State(event),
                    });
                },
            )
        })
        .await
        .map_err(worker_error)
        .and_then(|result| result);

        let session = app.state::<Session>();
        let event = match result {
            Ok(event) => {
                if let Event::Succeeded { path, .. } = &event {
                    *session.last_output.lock().unwrap() = Some(path.clone());
                    if let Some(directory) = path.parent() {
                        if let Err(error) =
                            app.state::<library::Library>().remember_export(directory)
                        {
                            eprintln!("Could not save export directory history: {error:?}");
                        }
                    }
                }
                JobEvent::State(event)
            }
            Err(error) => JobEvent::Error {
                kind: "failed",
                error,
            },
        };
        *session.active.lock().unwrap() = None;
        let _ = on_event.send(JobMessage { job_id: id, event });
        if session.exit_after_job.load(Ordering::Acquire) {
            app.exit(0);
        }
    });
    Ok(id)
}

#[tauri::command]
fn cancel_export(job_id: u32, session: State<'_, Session>) -> bool {
    session
        .active
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|job| job.id == job_id && job.control.cancel())
}

#[tauri::command]
fn exit_after_export(app: AppHandle) {
    let session = app.state::<Session>();
    let active = session.active.lock().unwrap();
    if let Some(job) = active.as_ref() {
        session.exit_after_job.store(true, Ordering::Release);
        job.control.cancel();
    } else {
        drop(active);
        app.exit(0);
    }
}

fn playback_cancelled() -> AppError {
    AppError::new("Cancelled", "已切换播放内容。")
}

#[tauri::command]
async fn playback_url(
    media_id: Option<u32>,
    request_id: u32,
    compatible: bool,
    app: AppHandle,
) -> AppResult<Option<String>> {
    let session = app.state::<Session>();
    let path = match media_id {
        Some(id) => session.media(id)?.path,
        None => session
            .last_output
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| AppError::new("PlaybackFailed", "没有可播放的成品。"))?,
    };
    playback_for_path(path, request_id, compatible, &session.playback, &app).await
}

#[tauri::command]
async fn output_preview_url(
    settings: Settings,
    request_id: u32,
    compatible: bool,
    on_event: Channel<Event>,
    app: AppHandle,
) -> AppResult<String> {
    let session = app.state::<Session>();
    let media = session.media(settings.media_id)?;
    let (preparation, encoded) = {
        let mut playback = session.playback.lock().unwrap();
        if request_id < playback.request_id {
            return Err(playback_cancelled());
        }
        let encoded = if compatible && request_id == playback.request_id {
            playback.output_preview.take()
        } else {
            None
        };
        playback.stop();
        playback.request_id = request_id;
        let preparation = Arc::new(playback::Preparation::default());
        playback.preparation = Some(preparation.clone());
        (preparation, encoded)
    };
    let worker = preparation.clone();
    let tools = app.state::<Tools>().inner().clone();
    let output = tauri::async_runtime::spawn_blocking(move || {
        let result = (|| {
            let (proxy, path) = match encoded {
                Some(encoded) => encoded,
                None => {
                    let proxy = Arc::new(playback::Proxy::new()?);
                    let path = proxy.file(&format!("preview.{}", settings.format.extension()));
                    transcode::preview_movie(
                        &tools,
                        &media,
                        &settings,
                        &path,
                        &worker.cancelled,
                        move |event| {
                            let _ = on_event.send(event);
                        },
                    )?;
                    (proxy, path)
                }
            };
            if compatible {
                let playable = proxy.path();
                transcode::preview_movie_proxy(&tools, &path, &playable, &worker.cancelled)?;
                return Ok((proxy, "video/mp4", playable));
            }
            let mime = match settings.format {
                media::OutputFormat::Mp4 => "video/mp4",
                media::OutputFormat::Mov => "video/quicktime",
                media::OutputFormat::Webm => "video/webm",
            };
            Ok((proxy, mime, path))
        })();
        worker.finish();
        result
    })
    .await
    .map_err(worker_error)?;
    let mut playback = session.playback.lock().unwrap();
    if request_id != playback.request_id || preparation.cancelled.load(Ordering::Relaxed) {
        return Err(playback_cancelled());
    }
    playback.preparation = None;
    let (proxy, mime, path) = output?;
    if !compatible {
        playback.output_preview = Some((proxy.clone(), path.clone()));
    }
    playback.publish(path, mime, Some(proxy))
}

#[tauri::command]
async fn workspace_playback_url(
    workspace: PathBuf,
    path: PathBuf,
    request_id: u32,
    compatible: bool,
    app: AppHandle,
) -> AppResult<Option<String>> {
    let path = app
        .state::<library::Library>()
        .preview_video(&workspace, &path)?;
    let session = app.state::<Session>();
    playback_for_path(
        path,
        request_id,
        compatible,
        &session.workspace_playback,
        &app,
    )
    .await
}

async fn playback_for_path(
    path: PathBuf,
    request_id: u32,
    compatible: bool,
    state: &Mutex<Playback>,
    app: &AppHandle,
) -> AppResult<Option<String>> {
    let key = playback::FileKey::new(path)?;

    let preparation = {
        let mut playback = state.lock().unwrap();
        if request_id < playback.request_id {
            return Err(playback_cancelled());
        }
        playback.stop();
        playback.request_id = request_id;
        if !compatible {
            return key
                .mime()
                .map(|mime| playback.publish(key.path().to_path_buf(), mime, None))
                .transpose();
        }
        if let Some(proxy) = playback
            .cached
            .as_ref()
            .filter(|(cached, _)| *cached == key)
            .map(|(_, proxy)| proxy.clone())
        {
            return playback
                .publish(proxy.path(), "video/mp4", Some(proxy))
                .map(Some);
        }
        let preparation = Arc::new(playback::Preparation::default());
        playback.preparation = Some(preparation.clone());
        preparation
    };

    let tools = app.state::<Tools>().inner().clone();
    let worker = preparation.clone();
    let input = key.path().to_path_buf();
    let proxy = tauri::async_runtime::spawn_blocking(move || {
        let result = playback::Proxy::new().and_then(|proxy| {
            transcode::playback_proxy(&tools, &input, &proxy.path(), &worker.cancelled)?;
            Ok(Arc::new(proxy))
        });
        worker.finish();
        result
    })
    .await
    .map_err(worker_error)?;

    let mut playback = state.lock().unwrap();
    if request_id != playback.request_id || preparation.cancelled.load(Ordering::Relaxed) {
        return Err(playback_cancelled());
    }
    playback.preparation = None;
    let proxy = proxy?;
    playback.cached = Some((key, proxy.clone()));
    playback
        .publish(proxy.path(), "video/mp4", Some(proxy))
        .map(Some)
}

#[tauri::command]
fn stop_playback(request_id: u32, session: State<'_, Session>) {
    stop_playback_at(&session.playback, request_id);
}

#[tauri::command]
fn stop_workspace_playback(request_id: u32, session: State<'_, Session>) {
    stop_playback_at(&session.workspace_playback, request_id);
}

#[derive(Serialize)]
struct Sample {
    compressed: String,
    reference: String,
    start: f64,
    end: f64,
}

#[tauri::command]
async fn compression_sample(
    settings: Settings,
    request_id: u32,
    app: AppHandle,
) -> AppResult<Sample> {
    let session = app.state::<Session>();
    let media = session.media(settings.media_id)?;
    let preparation = {
        let mut playback = session.sample_playback.lock().unwrap();
        if request_id < playback.request_id {
            return Err(playback_cancelled());
        }
        playback.stop();
        playback.request_id = request_id;
        let preparation = Arc::new(playback::Preparation::default());
        playback.preparation = Some(preparation.clone());
        preparation
    };
    let worker = preparation.clone();
    let tools = app.state::<Tools>().inner().clone();
    let sample = tauri::async_runtime::spawn_blocking(move || {
        let result = playback::Proxy::new().and_then(|proxy| {
            let (path, mime, clip) = transcode::compression_sample(
                &tools,
                &media,
                &settings,
                &proxy,
                &worker.cancelled,
            )?;
            Ok((Arc::new(proxy), path, mime, clip))
        });
        worker.finish();
        result
    })
    .await
    .map_err(worker_error)?;
    let mut playback = session.sample_playback.lock().unwrap();
    if request_id != playback.request_id || preparation.cancelled.load(Ordering::Relaxed) {
        return Err(playback_cancelled());
    }
    playback.preparation = None;
    let (proxy, path, mime, clip) = sample?;
    let mut reference = session.sample_reference.lock().unwrap();
    reference.request_id = request_id;
    Ok(Sample {
        compressed: playback.publish(path, mime, Some(proxy.clone()))?,
        reference: reference.publish(proxy.path(), "video/mp4", Some(proxy))?,
        start: clip.start,
        end: clip.end,
    })
}

#[tauri::command]
fn stop_compression_sample(request_id: u32, session: State<'_, Session>) {
    stop_playback_at(&session.sample_playback, request_id);
    stop_playback_at(&session.sample_reference, request_id);
}

fn stop_playback_at(state: &Mutex<Playback>, request_id: u32) {
    let mut playback = state.lock().unwrap();
    if request_id >= playback.request_id {
        playback.request_id = request_id;
        playback.stop();
    }
}

#[tauri::command]
fn open_result(reveal: bool, app: AppHandle) -> AppResult<()> {
    let path = app
        .state::<Session>()
        .last_output
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| AppError::new("FileError", "没有可打开的成品。"))?;
    if reveal {
        app.opener()
            .reveal_item_in_dir(&path)
            .map_err(worker_error)?;
    } else {
        app.opener()
            .open_path(path.to_string_lossy().into_owned(), None::<&str>)
            .map_err(worker_error)?;
    }
    Ok(())
}

fn asset(app: &AppHandle, name: &str, override_variable: &str) -> PathBuf {
    let is_sidecar = matches!(name, "ffmpeg" | "ffprobe");
    let filename = if is_sidecar {
        format!("{name}{}", std::env::consts::EXE_SUFFIX)
    } else {
        name.to_owned()
    };
    if cfg!(debug_assertions) {
        if let Some(path) = std::env::var_os(override_variable) {
            return PathBuf::from(path);
        }
        let project = Path::new(env!("CARGO_MANIFEST_DIR"));
        let staged = if is_sidecar {
            project.join("binaries").join(format!(
                "{name}-{}{}",
                env!("TAURI_ENV_TARGET_TRIPLE"),
                std::env::consts::EXE_SUFFIX
            ))
        } else {
            project.join("resources").join(name)
        };
        if staged.is_file() {
            return staged;
        }
    }
    let bundled = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|folder| folder.join(&filename)));
    if let Some(bundled) = bundled.filter(|path| path.is_file()) {
        return bundled;
    }
    app.path()
        .resource_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(filename)
}

fn font_path(app: &AppHandle) -> Option<PathBuf> {
    let resource = asset(app, "NotoSansCJKsc-Regular.otf", "VIDEO_TRANS_FONT");
    if resource.is_file() {
        return Some(resource);
    }
    #[cfg(target_os = "macos")]
    {
        let system = Path::new("/System/Library/Fonts/Hiragino Sans GB.ttc");
        return system.is_file().then(|| system.to_path_buf());
    }
    #[cfg(not(target_os = "macos"))]
    None
}

fn main() {
    tauri::Builder::default()
        .manage(Session::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            app.manage(library::Library::new(app.path().app_data_dir()?));
            app.manage(Tools {
                ffmpeg: asset(app.handle(), "ffmpeg", "VIDEO_TRANS_FFMPEG"),
                ffprobe: asset(app.handle(), "ffprobe", "VIDEO_TRANS_FFPROBE"),
                font: font_path(app.handle()),
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let session = window.state::<Session>();
                if session.active.lock().unwrap().is_some() {
                    api.prevent_close();
                    let _ = window.emit("close-while-running", ());
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            library::library_preferences,
            library::add_workspace,
            library::remove_workspace,
            library::workspace_videos,
            library::set_export_directory,
            library::remove_export_history,
            library::export_directory,
            probe_video,
            get_export_plan,
            render_preview,
            start_export,
            cancel_export,
            exit_after_export,
            open_result,
            playback_url,
            output_preview_url,
            stop_playback,
            workspace_playback_url,
            stop_workspace_playback,
            compression_sample,
            stop_compression_sample,
        ])
        .build(tauri::generate_context!())
        .expect("could not start the desktop app")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                let session = app.state::<Session>();
                for state in [
                    &session.playback,
                    &session.workspace_playback,
                    &session.sample_playback,
                    &session.sample_reference,
                ] {
                    let mut playback = state.lock().unwrap();
                    playback.stop();
                    if let Some(preparation) = playback.preparation.take() {
                        preparation.wait();
                    }
                    playback.cached = None;
                }
            }
            if let tauri::RunEvent::ExitRequested {
                api, code: None, ..
            } = event
            {
                let session = app.state::<Session>();
                if session.active.lock().unwrap().is_some() {
                    api.prevent_exit();
                    let _ = app.emit("close-while-running", ());
                }
            }
        });
}

#[cfg(test)]
mod smoke_tests;
