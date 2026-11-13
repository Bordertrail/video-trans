# 视频转码 · Video Trans

简体中文 · [English](README.en.md)

Video Trans 是一款面向 macOS 和 Windows 的本地视频转码工具，由 [Bordertrail](https://github.com/Bordertrail) 开发。视频在设备上处理，安装包已包含 FFmpeg 和 FFprobe。

![视频转码简体中文界面，显示视频预览和输出设置](docs/screenshots/video-trans-zh.png)

点击右上角的「中文」或「EN」即可切换界面语言。软件会记住上次选择。

## 下载安装

前往 [GitHub Releases](https://github.com/Bordertrail/video-trans/releases)，在版本的 **Assets** 中选择对应的安装包。各版本包含的功能以发布说明为准。

| 系统 | 安装包 | 安装方法 |
| --- | --- | --- |
| macOS 13 及以上，Apple 芯片 | `*-macos-arm64.dmg` | 打开镜像，将「视频转码」拖入「应用程序」。 |
| Windows 10 / 11，x64 | `*-windows-x64-setup.exe` | 运行安装程序。缺少 WebView2 时，安装程序会联网下载。 |

## 功能

- **格式与分辨率**：导入 MP4、MOV、MKV、WebM、AVI 等常见格式，导出为 MP4、MOV 或 WebM。可保持原分辨率，也可等比降至 1080p、720p 或 480p。
- **目标大小**：可填写导出文件的大小上限，也支持小数。留空时使用自动码率。目标太小时，可以调高上限或降低分辨率。
- **水印**：添加文字或图片水印，调整大小、透明度和位置。可在预览画面上直接拖动水印。
- **预览与工作空间**：在应用内播放源视频和成品。可添加多个文件夹作为工作空间，按修改日期或格式筛选视频，并在导入前打开小窗预览。
- **导出目录**：设置默认目录，从历史记录中选用目录，或移除历史记录。导出文件名默认带本地时间戳。
- **常用预设、压缩预览与截取**：保存并复用输出设置，对比最多 5 秒的压缩样片和源画面，或在播放器时间轴上拖动把手选定片段。可添加多个片段，按源视频的时间顺序拼接为一个文件。启用截取后，压缩样片取自第一个片段。

GitHub 上已发布的 v0.2.4 安装包不包含目标大小上限或历史目录移除功能。这两项功能分别在 v0.2.5 和 v0.2.6 的本地开发版中加入。

## 从源码构建

需要 Node.js 22 和 Rust stable。在仓库根目录运行下列命令。

**macOS（Apple 芯片）**需要 Xcode 命令行工具。

```sh
rustup target add aarch64-apple-darwin
npm ci
npm run build:media
npm run build:macos
```

DMG 输出到 `src-tauri/target/aarch64-apple-darwin/release/bundle/dmg/`。

**Windows（x64）**需要 Rust MSVC 工具链、Visual Studio Build Tools 的「使用 C++ 的桌面开发」工作负载和 WebView2。

```powershell
rustup target add x86_64-pc-windows-msvc
npm ci
npm run build:windows
```

EXE 安装包输出到 `src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/`。

项目使用 Rust、Tauri 2、TypeScript 和 FFmpeg。构建细节见[开发文档](DEVELOPMENT.md)。项目采用 [MIT 许可证](LICENSE)，安装包内附有第三方组件的许可说明。
