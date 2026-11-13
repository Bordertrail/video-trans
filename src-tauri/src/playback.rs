use crate::media::{AppError, AppResult};
use futures_util::StreamExt;
use http_body_util::{combinators::UnsyncBoxBody, BodyExt, Full, StreamBody};
use hyper::{
    body::{Bytes, Frame, Incoming},
    header,
    server::conn::http1,
    service::service_fn,
    Method, Request, Response, StatusCode,
};
use hyper_util::rt::{TokioIo, TokioTimer};
use std::{
    convert::Infallible,
    io::SeekFrom,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    time::{Duration, SystemTime},
};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

type Body = UnsyncBoxBody<Bytes, std::io::Error>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileKey {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
}

impl FileKey {
    pub fn new(path: PathBuf) -> AppResult<Self> {
        let metadata = path.metadata().map_err(AppError::io)?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(AppError::new("PlaybackFailed", "没有可播放的视频文件。"));
        }
        Ok(Self {
            path,
            size: metadata.len(),
            modified: metadata.modified().map_err(AppError::io)?,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn mime(&self) -> Option<&'static str> {
        match self
            .path
            .extension()?
            .to_str()?
            .to_ascii_lowercase()
            .as_str()
        {
            "mp4" | "m4v" => Some("video/mp4"),
            "mov" => Some("video/quicktime"),
            "webm" => Some("video/webm"),
            _ => None,
        }
    }
}

pub struct Proxy {
    directory: TempDir,
}

impl Proxy {
    pub fn new() -> AppResult<Self> {
        Ok(Self {
            directory: tempfile::Builder::new()
                .prefix("video-trans-play-")
                .tempdir()
                .map_err(AppError::io)?,
        })
    }

    pub fn path(&self) -> PathBuf {
        self.file("playback.mp4")
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
    }
}

struct Published {
    token: uuid::Uuid,
    path: PathBuf,
    mime: &'static str,
    // Keep the proxy alive for any range response already reading it.
    _proxy: Option<Arc<Proxy>>,
}

type Route = Arc<Mutex<Option<Arc<Published>>>>;

pub struct Server {
    address: String,
    route: Route,
}

impl Server {
    pub fn start() -> AppResult<Self> {
        let listener =
            std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(AppError::io)?;
        listener.set_nonblocking(true).map_err(AppError::io)?;
        let address = listener.local_addr().map_err(AppError::io)?.to_string();
        let route = Route::default();
        let active = route.clone();
        let host = address.clone();
        tauri::async_runtime::spawn(async move {
            let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                return;
            };
            while let Ok((socket, _)) = listener.accept().await {
                let active = active.clone();
                let host = host.clone();
                tauri::async_runtime::spawn(async move {
                    let _ =
                        http1::Builder::new()
                            .keep_alive(false)
                            .timer(TokioTimer::new())
                            .header_read_timeout(Duration::from_secs(5))
                            .serve_connection(
                                TokioIo::new(socket),
                                service_fn(move |request| {
                                    let route = active.clone();
                                    let host = host.clone();
                                    async move {
                                        Ok::<_, Infallible>(serve(request, &host, &route).await)
                                    }
                                }),
                            )
                            .await;
                });
            }
        });
        Ok(Self { address, route })
    }

    pub fn publish(&self, path: PathBuf, mime: &'static str, proxy: Option<Arc<Proxy>>) -> String {
        let token = uuid::Uuid::new_v4();
        *self.route.lock().unwrap() = Some(Arc::new(Published {
            token,
            path,
            mime,
            _proxy: proxy,
        }));
        format!("http://{}/{token}", self.address)
    }

    pub fn clear(&self) {
        *self.route.lock().unwrap() = None;
    }
}

fn empty() -> Body {
    Full::new(Bytes::new())
        .map_err(|error| match error {})
        .boxed_unsync()
}

fn error_response(status: StatusCode) -> Response<Body> {
    Response::builder().status(status).body(empty()).unwrap()
}

// Safari uses a small initial byte range, then additional ranges for playback and seeks.
fn byte_range(header: &str, len: u64) -> Option<(u64, u64)> {
    let (first, last) = header.strip_prefix("bytes=")?.split_once('-')?;
    if len == 0 {
        return None;
    }
    if first.is_empty() {
        let suffix = last.parse::<u64>().ok()?;
        return (suffix != 0).then_some((len.saturating_sub(suffix), len - 1));
    }
    let first = first.parse::<u64>().ok()?;
    let last = if last.is_empty() {
        len - 1
    } else {
        last.parse::<u64>().ok()?.min(len - 1)
    };
    (first <= last).then_some((first, last))
}

async fn serve(request: Request<Incoming>, host: &str, route: &Route) -> Response<Body> {
    if request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        != Some(host)
    {
        return error_response(StatusCode::BAD_REQUEST);
    }
    let head = request.method() == Method::HEAD;
    if !head && request.method() != Method::GET {
        return error_response(StatusCode::METHOD_NOT_ALLOWED);
    }
    let token = request
        .uri()
        .path()
        .strip_prefix('/')
        .and_then(|path| uuid::Uuid::parse_str(path).ok());
    let file = route.lock().unwrap().clone();
    let Some(file) = file.filter(|file| Some(file.token) == token) else {
        return error_response(StatusCode::NOT_FOUND);
    };
    let Ok(mut input) = tokio::fs::File::open(&file.path).await else {
        return error_response(StatusCode::NOT_FOUND);
    };
    let Ok(metadata) = input.metadata().await else {
        return error_response(StatusCode::NOT_FOUND);
    };
    let len = metadata.len();
    if !metadata.is_file() || len == 0 {
        return error_response(StatusCode::NOT_FOUND);
    }

    let mut response = Response::builder()
        .header(header::CONTENT_TYPE, file.mime)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "no-store");
    let count = if let Some(range) = request.headers().get(header::RANGE) {
        let Some((start, end)) = range.to_str().ok().and_then(|range| byte_range(range, len))
        else {
            return response
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{len}"))
                .body(empty())
                .unwrap();
        };
        if !head && input.seek(SeekFrom::Start(start)).await.is_err() {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR);
        }
        response = response
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"));
        end - start + 1
    } else {
        len
    };
    let body = if head {
        empty()
    } else {
        let stream = ReaderStream::with_capacity(input.take(count), 64 * 1024).map(move |bytes| {
            let _lease = &file;
            bytes.map(Frame::data)
        });
        BodyExt::boxed_unsync(StreamBody::new(stream))
    };
    response
        .header(header::CONTENT_LENGTH, count)
        .body(body)
        .unwrap()
}

#[derive(Default)]
pub struct Preparation {
    pub cancelled: AtomicBool,
    done: Mutex<bool>,
    wake: Condvar,
}

impl Preparation {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn finish(&self) {
        *self.done.lock().unwrap() = true;
        self.wake.notify_all();
    }

    pub fn wait(&self) {
        let _finished = self.wake.wait_timeout_while(
            self.done.lock().unwrap(),
            Duration::from_secs(2),
            |done| !*done,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{byte_range, Server};
    use std::{
        io::{Read, Write},
        net::TcpStream,
        time::Duration,
    };

    #[test]
    fn supports_playback_and_seek_ranges() {
        assert_eq!(byte_range("bytes=0-1", 100), Some((0, 1)));
        assert_eq!(byte_range("bytes=60-", 100), Some((60, 99)));
        assert_eq!(byte_range("bytes=-20", 100), Some((80, 99)));
        assert_eq!(byte_range("bytes=60-200", 100), Some((60, 99)));
        assert_eq!(byte_range("bytes=100-", 100), None);
        assert_eq!(byte_range("bytes=0-1,50-60", 100), None);
    }

    #[test]
    #[ignore = "requires the ability to bind a loopback socket"]
    fn serves_only_the_current_video_and_its_requested_bytes() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"0123456789").unwrap();
        let server = Server::start().unwrap();
        let url = server.publish(file.path().to_path_buf(), "video/mp4", None);
        let (host, token) = url
            .strip_prefix("http://")
            .unwrap()
            .split_once('/')
            .unwrap();
        let request = |method: &str, range: &str| {
            let mut socket = TcpStream::connect(host).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            write!(
                socket,
                "{method} /{token} HTTP/1.1\r\nHost: {host}\r\n{range}Connection: close\r\n\r\n"
            )
            .unwrap();
            let mut response = String::new();
            socket.read_to_string(&mut response).unwrap();
            response
        };
        let header = request("HEAD", "");
        assert!(header.contains("200 OK"));
        assert!(header.contains("content-length: 10"));
        assert!(header.ends_with("\r\n\r\n"));
        let partial = request("GET", "Range: bytes=4-6\r\n");
        assert!(partial.contains("206 Partial Content"));
        assert!(partial.contains("content-range: bytes 4-6/10"));
        assert!(partial.ends_with("\r\n\r\n456"));
        server.clear();
        assert!(request("GET", "").contains("404 Not Found"));
    }
}
