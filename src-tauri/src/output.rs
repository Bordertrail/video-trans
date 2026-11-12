use crate::media::{AppError, AppResult, OutputFormat};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, PartialEq, Eq)]
struct Identity {
    device: u64,
    inode: u64,
    size: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    changed: (i64, i64),
}

#[cfg(unix)]
fn identity(_: &Path, metadata: fs::Metadata) -> io::Result<Identity> {
    use std::os::unix::fs::MetadataExt;
    Ok(Identity {
        device: metadata.dev(),
        inode: metadata.ino(),
        size: metadata.len(),
        modified: metadata.modified()?,
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    })
}

#[cfg(windows)]
fn identity(path: &Path, _: fs::Metadata) -> io::Result<Identity> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let file = fs::File::open(path)?;
    let metadata = file.metadata()?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // The file owns the handle and info is writable for the duration of the call.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Identity {
        device: u64::from(info.dwVolumeSerialNumber),
        inode: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        size: metadata.len(),
        modified: metadata.modified()?,
    })
}

pub struct Destination {
    pub path: PathBuf,
    pub directory: PathBuf,
    existing: Option<Identity>,
}

fn conflict(message: &str) -> AppError {
    AppError::new("OutputConflict", message)
}

fn existing(path: &Path) -> AppResult<Option<Identity>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            identity(path, metadata).map(Some).map_err(AppError::io)
        }
        Ok(_) => Err(AppError::new(
            "InvalidOutput",
            "请选择普通文件作为保存目标。",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AppError::io(error)),
    }
}

impl Destination {
    pub fn prepare(
        source: &Path,
        path: &Path,
        format: OutputFormat,
        overwrite: bool,
    ) -> AppResult<Self> {
        let name = path
            .file_name()
            .ok_or_else(|| AppError::new("InvalidOutput", "请选择成品文件名。"))?;
        let directory = dunce::canonicalize(path.parent().unwrap_or_else(|| Path::new(".")))
            .map_err(AppError::io)?;
        let path = directory.join(name);
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        if !extension.eq_ignore_ascii_case(format.extension()) {
            return Err(AppError::new(
                "InvalidOutput",
                format!("文件扩展名应为 .{}。", format.extension()),
            ));
        }
        let existing = existing(&path)?;
        let source_identity =
            identity(source, fs::metadata(source).map_err(AppError::io)?).map_err(AppError::io)?;
        if path == source
            || existing.as_ref().is_some_and(|target| {
                target.device == source_identity.device && target.inode == source_identity.inode
            })
        {
            return Err(AppError::new(
                "InvalidOutput",
                "成品不能覆盖源视频，请选择其他文件名。",
            ));
        }
        if existing.is_some() && !overwrite {
            return Err(conflict("已有同名文件。"));
        }
        Ok(Self {
            path,
            directory,
            existing,
        })
    }

    pub fn commit(self, temporary: &Path) -> AppResult<(PathBuf, u64)> {
        let size = fs::metadata(temporary).map_err(AppError::io)?.len();
        if size == 0 {
            return Err(AppError::new("EncoderFailed", "编码器生成了空文件。"));
        }
        if self.existing.is_some() {
            if existing(&self.path)? != self.existing {
                return Err(conflict("同名文件在转码期间发生变化，请重新选择保存位置。"));
            }
            fs::rename(temporary, &self.path).map_err(AppError::io)?;
        } else {
            rename_exclusive(temporary, &self.path)?;
        }
        Ok((self.path, size))
    }
}

fn rename_exclusive(from: &Path, to: &Path) -> AppResult<()> {
    move_exclusive(from, to).map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            conflict("转码期间出现了同名文件，原文件已保留。")
        } else {
            AppError::io(error)
        }
    })
}

#[cfg(target_os = "macos")]
fn move_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let from = CString::new(from.as_os_str().as_bytes()).map_err(io::Error::other)?;
    let to = CString::new(to.as_os_str().as_bytes()).map_err(io::Error::other)?;
    // The C strings remain valid for the duration of the macOS system call.
    if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) } == 0 {
        return Ok(());
    }
    Err(io::Error::last_os_error())
}

#[cfg(windows)]
fn move_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    let from: Vec<u16> = from.as_os_str().encode_wide().chain([0]).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain([0]).collect();
    // Without MOVEFILE_REPLACE_EXISTING, an existing destination is preserved.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } != 0 {
        return Ok(());
    }
    Err(io::Error::last_os_error())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn move_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    // The temporary directory owns the original link and removes it after export.
    fs::hard_link(from, to)
}
