# Video Trans

[简体中文](README.md) · English

Video Trans is a desktop video converter for macOS and Windows, developed by [Bordertrail](https://github.com/Bordertrail). It processes videos on your device and comes with FFmpeg and FFprobe.

## Download and install

Go to [GitHub Releases](https://github.com/Bordertrail/video-trans/releases) and choose the installer under **Assets**. Check the release notes for the features available in each version.

| Platform | Installer | Installation |
| --- | --- | --- |
| macOS 13 or later, Apple silicon | `*-macos-arm64.dmg` | Open the disk image and drag Video Trans into Applications. |
| Windows 10 / 11, x64 | `*-windows-x64-setup.exe` | Run the installer. It downloads WebView2 if needed. |

## Features

- **Formats and resolution** — Import common formats such as MP4, MOV, MKV, WebM, and AVI. Export to MP4, MOV, or WebM. Keep the original resolution or scale down to 1080p, 720p, or 480p while preserving the aspect ratio.
- **File size limit** — Set a maximum export size in MB, including decimal values. Leave the field blank to use automatic bitrate settings. If the limit is too low, raise it or choose a lower resolution.
- **Watermarks** — Add text or image watermarks and adjust their size, opacity, and position. Drag a watermark directly in the preview to position it.
- **Playback and workspaces** — Play source and exported videos in the app. Add multiple folders as workspaces, filter videos by modification date or format, and open a preview before importing.
- **Export folders** — Set a default folder, reuse a recent folder, or remove an entry from the history. Export filenames include a local timestamp by default.

The published v0.2.4 installers predate file size limits and export history removal. These features were added in the local development builds v0.2.5 and v0.2.6, respectively.

## Build from source

Install Node.js 22 and stable Rust. Run these commands from the repository root.

**macOS (Apple silicon)** requires the Xcode Command Line Tools.

```sh
rustup target add aarch64-apple-darwin
npm ci
npm run build:media
npm run build:macos
```

Find the DMG in `src-tauri/target/aarch64-apple-darwin/release/bundle/dmg/`.

**Windows (x64)** requires the Rust MSVC toolchain, the **Desktop development with C++** workload in Visual Studio Build Tools, and WebView2.

```powershell
rustup target add x86_64-pc-windows-msvc
npm ci
npm run build:windows
```

Find the EXE installer in `src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/`.

The project uses Rust, Tauri 2, TypeScript, and FFmpeg. See the [development guide (Chinese)](DEVELOPMENT.md) for build details. The project uses the [MIT License](LICENSE). Installers include third-party license notices.
