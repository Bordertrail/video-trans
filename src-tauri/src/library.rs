use crate::media::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::UNIX_EPOCH,
};
use tauri::State;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Preferences {
    workspaces: Vec<PathBuf>,
    default_export_directory: Option<PathBuf>,
    export_history: Vec<PathBuf>,
}

pub struct Library {
    file: PathBuf,
    lock: Mutex<()>,
}

impl Library {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            file: directory.join("library.json"),
            lock: Mutex::new(()),
        }
    }

    fn read(&self) -> AppResult<Preferences> {
        match fs::read(&self.file) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(preferences_error),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Preferences::default()),
            Err(error) => Err(preferences_error(error)),
        }
    }

    fn update(&self, change: impl FnOnce(&mut Preferences)) -> AppResult<Preferences> {
        let _guard = self.lock.lock().unwrap();
        let mut preferences = self.read()?;
        change(&mut preferences);
        let directory = self.file.parent().unwrap();
        fs::create_dir_all(directory).map_err(preferences_error)?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(directory).map_err(preferences_error)?;
        serde_json::to_writer_pretty(&mut temporary, &preferences).map_err(preferences_error)?;
        temporary.flush().map_err(preferences_error)?;
        temporary.as_file().sync_all().map_err(preferences_error)?;
        temporary.persist(&self.file).map_err(preferences_error)?;
        Ok(preferences)
    }

    pub fn preview_video(&self, workspace: &Path, path: &Path) -> AppResult<PathBuf> {
        if !self.read()?.workspaces.iter().any(|root| root == workspace) {
            return Err(AppError::new("InvalidWorkspace", "请先添加此工作空间。"));
        }
        let root = directory(workspace)?;
        let video = dunce::canonicalize(path).map_err(AppError::io)?;
        if !video.starts_with(&root) || !video.is_file() || video_format(&video).is_none() {
            return Err(AppError::new(
                "InvalidWorkspaceVideo",
                "请选择此工作空间中的视频。",
            ));
        }
        Ok(video)
    }

    pub fn remember_export(&self, directory: &Path) -> AppResult<()> {
        self.update(|preferences| {
            preferences.export_history.retain(|path| path != directory);
            preferences
                .export_history
                .insert(0, directory.to_path_buf());
        })?;
        Ok(())
    }
}

fn preferences_error(error: impl std::fmt::Display) -> AppError {
    AppError::new("PreferencesError", "无法读取或保存工作空间和导出目录设置。")
        .diagnostic(error.to_string())
}

fn directory(path: &Path) -> AppResult<PathBuf> {
    let canonical = dunce::canonicalize(path).map_err(AppError::io)?;
    if !canonical.is_dir() {
        return Err(AppError::new(
            "InvalidDirectory",
            "请选择一个可用的文件夹。",
        ));
    }
    Ok(canonical)
}

#[tauri::command]
pub fn library_preferences(library: State<'_, Library>) -> AppResult<Preferences> {
    library.read()
}

#[derive(Serialize)]
pub struct AddedWorkspace {
    path: PathBuf,
    preferences: Preferences,
}

#[tauri::command]
pub fn add_workspace(path: PathBuf, library: State<'_, Library>) -> AppResult<AddedWorkspace> {
    let path = directory(&path)?;
    let preferences = library.update(|preferences| {
        if !preferences.workspaces.contains(&path) {
            preferences.workspaces.push(path.clone());
        }
    })?;
    Ok(AddedWorkspace { path, preferences })
}

#[tauri::command]
pub fn remove_workspace(path: PathBuf, library: State<'_, Library>) -> AppResult<Preferences> {
    library.update(|preferences| preferences.workspaces.retain(|root| root != &path))
}

#[tauri::command]
pub fn set_export_directory(
    path: Option<PathBuf>,
    library: State<'_, Library>,
) -> AppResult<Preferences> {
    let path = path.as_deref().map(directory).transpose()?;
    library.update(|preferences| preferences.default_export_directory = path)
}

#[tauri::command]
pub fn export_directory(
    path: Option<PathBuf>,
    library: State<'_, Library>,
) -> AppResult<Option<PathBuf>> {
    if let Some(path) = path {
        return directory(&path).map(Some);
    }
    // An ejected drive or deleted default folder leaves the source directory as the fallback.
    Ok(library
        .read()
        .unwrap_or_default()
        .default_export_directory
        .filter(|path| path.is_dir()))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceVideo {
    path: PathBuf,
    relative_path: PathBuf,
    format: String,
    modified_ms: Option<u64>,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceListing {
    videos: Vec<WorkspaceVideo>,
    skipped: usize,
}

#[tauri::command]
pub async fn workspace_videos(
    path: PathBuf,
    library: State<'_, Library>,
) -> AppResult<WorkspaceListing> {
    if !library.read()?.workspaces.contains(&path) {
        return Err(AppError::new("InvalidWorkspace", "请先添加此工作空间。"));
    }
    tauri::async_runtime::spawn_blocking(move || scan(&path))
        .await
        .map_err(|error| {
            AppError::new("WorkspaceError", "无法读取工作空间。").diagnostic(error.to_string())
        })?
}

fn scan(root: &Path) -> AppResult<WorkspaceListing> {
    let root = directory(root)?;
    let mut listing = WorkspaceListing::default();
    let mut directories = vec![root.clone()];
    while let Some(folder) = directories.pop() {
        let entries = match fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(error) if folder == root => return Err(AppError::io(error)),
            Err(_) => {
                listing.skipped += 1;
                continue;
            }
        };
        for entry in entries {
            let Ok(entry) = entry else {
                listing.skipped += 1;
                continue;
            };
            let name = entry.file_name();
            if name.as_encoded_bytes().starts_with(b".") {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                listing.skipped += 1;
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                directories.push(path);
            } else if kind.is_file() && path.to_str().is_some() {
                let Some(format) = video_format(&path) else {
                    continue;
                };
                let modified_ms = entry
                    .metadata()
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map(|elapsed| elapsed.as_millis() as u64);
                let relative_path = path.strip_prefix(&root).unwrap().to_path_buf();
                listing.videos.push(WorkspaceVideo {
                    path,
                    relative_path,
                    format,
                    modified_ms,
                });
            }
        }
    }
    Ok(listing)
}

fn video_format(path: &Path) -> Option<String> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "mp4"
            | "m4v"
            | "mov"
            | "mkv"
            | "webm"
            | "avi"
            | "ts"
            | "mts"
            | "m2ts"
            | "mpg"
            | "mpeg"
            | "vob"
            | "flv"
            | "ogv"
            | "wmv"
            | "asf"
            | "3gp"
            | "3g2"
    )
    .then_some(extension)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_nested_video_files_without_following_links() {
        let workspace = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(workspace.path()).unwrap();
        fs::create_dir(root.join("素材")).unwrap();
        fs::create_dir(root.join(".hidden")).unwrap();
        fs::write(root.join("素材/旅行.MOV"), b"").unwrap();
        fs::write(root.join("片段.mp4"), b"").unwrap();
        fs::write(root.join("说明.txt"), b"").unwrap();
        fs::write(root.join(".hidden/hidden.mp4"), b"").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();

        let result = scan(&root).unwrap();
        assert_eq!(result.skipped, 0);
        assert_eq!(result.videos.len(), 2);
        assert!(result.videos.iter().any(|video| {
            video.relative_path == Path::new("素材/旅行.MOV")
                && video.format == "mov"
                && video.modified_ms.is_some()
        }));
    }

    #[test]
    fn previews_only_video_files_inside_registered_workspaces() {
        let storage = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(storage.path()).unwrap();
        let workspace = root.join("workspace");
        let library = Library::new(root.join("settings"));
        fs::create_dir(&workspace).unwrap();
        let video = workspace.join("clip.mp4");
        let outside = root.join("outside.mp4");
        let notes = workspace.join("notes.txt");
        for path in [&video, &outside, &notes] {
            fs::write(path, b"").unwrap();
        }
        assert!(library.preview_video(&workspace, &video).is_err());
        library
            .update(|preferences| preferences.workspaces.push(workspace.clone()))
            .unwrap();
        assert_eq!(library.preview_video(&workspace, &video).unwrap(), video);
        for path in [&outside, &notes] {
            assert!(library.preview_video(&workspace, path).is_err());
        }
        #[cfg(unix)]
        {
            let link = workspace.join("link.mp4");
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            assert!(library.preview_video(&workspace, &link).is_err());
        }
    }

    #[test]
    fn persists_multiple_workspaces_and_recent_export_directories() {
        let settings = tempfile::tempdir().unwrap();
        let library = Library::new(settings.path().join("app-data"));
        let first = PathBuf::from("/first");
        let second = PathBuf::from("/second");

        assert!(library.read().unwrap().workspaces.is_empty());
        library
            .update(|preferences| {
                preferences.workspaces = vec![first.clone(), second.clone()];
                preferences.default_export_directory = Some(first.clone());
            })
            .unwrap();
        library.remember_export(&first).unwrap();
        library.remember_export(&second).unwrap();
        library.remember_export(&first).unwrap();

        let reopened = Library::new(settings.path().join("app-data"));
        let preferences = reopened.read().unwrap();
        assert_eq!(preferences.workspaces, [first.clone(), second.clone()]);
        assert_eq!(preferences.default_export_directory, Some(first.clone()));
        assert_eq!(preferences.export_history, [first, second]);
    }
}
