# macOS 视频转码工具开发文档

## 1. 产品范围

首版是一款简体中文的本地桌面应用。用户导入一个视频，选择输出格式、分辨率和可选水印，预览画面后保存成品。一次运行一个转码任务，素材和成品均留在本机。

| 项目 | 首版规格 |
| --- | --- |
| 发布目标 | Apple Silicon（arm64），macOS 13 及以上，通过 DMG 安装。Intel Mac 和 Windows 延后。 |
| 输入 | 支持文件选择和单文件拖拽。首批验证 MP4、MOV、MKV、WebM、AVI、M4V，实际支持由随包 FFmpeg 的解封装和解码能力决定。 |
| 输出 | MP4、WebM、MOV，默认 MP4。允许源文件与成品使用同一种格式，例如给已有 MP4 加水印。 |
| 分辨率 | 保持原分辨率、1080p、720p、480p，默认保持原分辨率。 |
| 水印 | 关闭、文字、图片三选一。水印位置可预设或在预览中拖动，全片显示。 |
| 音频 | 优先使用默认音轨，否则选择第一条。无音轨时输出无声视频。 |

首版输出 8 位 SDR 视频。识别为 HDR 的素材显示明确提示并阻止导出，后续版本再处理 HDR 保留或 SDR 色调映射。字幕、多音轨导出、批量任务、裁剪和剪辑编码预设放在后续版本。

## 2. 用户流程

1. 在主窗口点击「选择视频」或拖入一个本地视频。导入后显示文件名、时长、显示尺寸和文件大小。点击预览区的「源视频」在窗口内播放。文件损坏、缺少视频流或无法解码时显示原因。
2. 选择格式和输出分辨率。界面同时显示实际输出尺寸。修改分辨率或水印时，刷新静帧预览。
3. 如需水印，选择「文字」或「图片」，修改位置、大小和透明度。可在预览区直接拖动水印，或用方向键微调。
4. 点击「开始转换」后打开 macOS 保存对话框。初始目录依次取本次选择的历史目录、默认导出目录和源文件目录。文件名为 `原文件名-converted_年-月-日_时-分-秒.扩展名`。时间戳取设备本地时间，扩展名随所选格式设置。路径已存在时要求明确确认覆盖。
5. 转换中在左侧预览下方显示处理进度和「取消」，暂时锁定本次任务的设置。完成后展示成品路径、文件大小、「打开文件」和「在访达中显示」，同时在左侧预览区显示「成品」。失败或取消后恢复设置，以便重试。

## 3. 技术栈和模块

| 层 | 技术 | 职责 |
| --- | --- | --- |
| 桌面壳 | Tauri 2 | macOS 窗口、拖放、原生文件对话框、打开文件和定位到访达。 |
| 界面 | TypeScript、Vite、HTML/CSS | 单窗口表单、预览、进度和结果。 |
| 应用逻辑 | Rust、Tauri 异步运行时、Serde、tempfile、ab_glyph、image | 媒体模型、参数校验、任务状态、子进程、临时文件和错误映射。 |
| 媒体处理 | 随包 FFmpeg、FFprobe | 读取媒体信息、抽帧、缩放、叠加水印、编码和封装。 |

建议前端位于 `src/`，Rust 位于 `src-tauri/src/`。Rust 按 `media`（探测与格式）、`export_plan`（尺寸与水印参数）、`preview`（抽帧）、`job`（进程与状态）、`output`（成品文件提交）划分。前端通过 Tauri command 调用 Rust，使用 Tauri Channel 接收进度。FFmpeg 和 FFprobe 通过 Tauri 的 `externalBin` 一起打包，Rust 侧统一解析已打包的二进制路径并启动进程。参数按数组传入，文件路径作为独立参数处理。前端只调用业务命令。 [Tauri Sidecar 文档](https://v2.tauri.app/develop/sidecar/)

### 接口约定

| 命令 | 输入 | 返回 |
| --- | --- | --- |
| `probe_video` | 用户选取的文件路径 | `mediaId`、文件名、字节数、时长、原始尺寸、显示尺寸、音轨和导出可用性。 |
| `get_export_plan` | `mediaId`、格式、分辨率、水印设置 | 实际输出宽高、各档位状态和字段校验结果。 |
| `render_preview` | 同一组导出设置、`requestId` | JPEG 静帧、可选的透明 PNG 水印及其在成品画面中的像素矩形。 |
| `start_export` | 导出设置、目标路径、覆盖确认、进度 Channel | `jobId`。后续通过 Channel 返回状态、进度、成品信息或错误。 |
| `cancel_export` | `jobId` | 取消请求是否受理，最终状态以 Channel 为准。 |

预设位置用枚举值，自定义位置用水印可移动范围内的归一化坐标。预览和导出复用同一套 Rust 水印图层与坐标计算。导出开始时固定设置快照，所有 UI 更新通过 `requestId` 或 `jobId` 对应到当前请求。

## 4. 媒体处理

### 4.1 探测和流选择

- 调用 `ffprobe -v error -show_format -show_streams -of json <input>`。保存流索引、尺寸、像素宽高比（SAR）、旋转信息、色彩传输特性、帧率参考值和时长。
- 优先选标记为默认的视频流，否则选第一条有效视频流，跳过封面图。音轨采用相同的默认优先规则。导出时用索引显式映射音视频。
- 时长优先取所选视频流的有效时长，再取容器时长。缺失时界面显示「时长未知」，转换使用不定进度。
- 依据 SAR 和旋转元数据计算显示宽高。对常见 90°、180°、270° 元数据，用 FFmpeg 自动旋转后再缩放。输出以已旋转的实际画面为准，测试时检查成品旋转信息是否会造成二次旋转。对超出首版支持的显示变换提示无法处理。
- 将 PQ（`smpte2084`）、HLG（`arib-std-b67`）或可识别的 Dolby Vision 流视为 HDR。普通 SDR 素材保留适用的色彩标记。

### 4.2 输出格式预设

| 格式 | 视频 | 音频 | 封装 |
| --- | --- | --- | --- |
| MP4 | `h264_videotoolbox`，`yuv420p` | `aac`，最多双声道，160 kb/s | `-f mp4 -movflags +faststart` |
| WebM | `libvpx-vp9`，`yuv420p`，`-crf 32 -b:v 0 -deadline good -cpu-used 4` | `libopus`，最多双声道，128 kb/s | `-f webm` |
| MOV | 与 MP4 相同的 H.264 预设 | 与 MP4 相同 | `-f mov -movflags +faststart` |

H.264 的初始目标码率为 `6 × (输出像素数 / 2073600) × (参考帧率 / 30) Mbps`，限制在 1–45 Mbps。参考帧率缺失时使用 30，仅用于估算码率。用 `-fps_mode passthrough` 保留时间戳节奏。保留单声道，三声道及以上下混为双声道，缺音轨时跳过音频参数。编码质量和码率预设在首轮样片测试时确认。 [FFmpeg 命令文档](https://ffmpeg.org/ffmpeg.html) [VideoToolbox 编码器源码](https://github.com/FFmpeg/FFmpeg/blob/master/libavcodec/videotoolboxenc.c)

以 1920×1080、30 fps、视频流索引为 0、音频流索引为 1 的横屏素材为例，转换为 720p MP4 的参考命令如下。应用根据探测结果填入流索引、尺寸和码率，并把输出指向任务临时目录。

```sh
ffmpeg -hide_banner -nostdin -nostats -progress pipe:1 \
  -i input.mov -map 0:0 -map 0:1 \
  -vf 'scale=1280:720,setsar=1' -fps_mode passthrough \
  -c:v h264_videotoolbox -pix_fmt yuv420p -b:v 2667k \
  -c:a aac -b:a 160k -movflags +faststart -f mp4 output.mp4
```

### 4.3 分辨率计算

| 档位 | 横屏边界 | 竖屏边界 |
| --- | --- | --- |
| 1080p | 1920 × 1080 | 1080 × 1920 |
| 720p | 1280 × 720 | 720 × 1280 |
| 480p | 854 × 480 | 480 × 854 |

横竖屏按显示尺寸判断。方形视频使用对应档位的短边作为边长上限。以显示尺寸 `W × H` 和档位边界 `Bw × Bh` 计算 `s = min(1, Bw/W, Bh/H)`，将两边各自向下取整至偶数。若 `s = 1`，该降分辨率档位置灰。等比缩放不裁切，偶数取整最多引入约两像素误差。对于小到无法得到有效编码尺寸的源文件，提示尺寸不支持。

「保持原分辨率」沿用旋转后的显示尺寸，非方形像素规范化为方形像素，奇数边最多补 1 像素。Rust 返回最终成品尺寸，FFmpeg 按同一个尺寸计划执行 `scale`、必要的 `pad` 和 `setsar=1`。例如 1920×1080 → 1280×720，1080×1920 → 720×1280，2560×1080 → 1280×540。 [FFmpeg 滤镜文档](https://www.ffmpeg.org/ffmpeg-filters.html)

### 4.4 水印

| 项目 | 规则 |
| --- | --- |
| 文字 | 单行、去除首尾空白、1–80 个 Unicode 字符。默认白色、100 px。字号按最终画面的像素计，支持 12–200 px。 |
| 图片 | 本地 PNG 或 JPEG。保留 PNG 的透明度，按原比例缩放。默认宽度为画面宽度的 20%，可调范围为 5%–60%。 |
| 透明度 | 10%–100%，默认 70%。图片的最终透明度为原始 alpha 与此设置相乘。 |
| 位置 | 左上、右上、左下、右下、居中。默认右下。四角边距取输出短边的 2%，最少 4 px。 |

先旋转和缩放视频，再按成品坐标叠加水印。文字由 Rust 使用 `ab_glyph` 和随包 Noto Sans CJK SC 字体渲染成 RGBA 图层，渲染前根据实际字形边界检查文字尺寸。图片由 Rust 使用 `image` 校验格式、纠正 EXIF 方向、缩放并调整 alpha。两种水印都保存为任务目录内的透明 PNG，由 FFmpeg 使用同一套 `overlay` 滤镜合成。若水印超出画面，提示减小字号或图片宽度。 [FFmpeg 滤镜文档](https://www.ffmpeg.org/ffmpeg-filters.html) [Noto CJK 字体项目](https://github.com/notofonts/noto-cjk)

### 4.5 预览

默认抽取视频 10% 处的画面，取样时间最多 5 秒，时长未知时取首个可解码画面。抽帧失败时尝试首个可解码画面。将成品尺寸的底图等比缩至长边最多 960 px，返回 JPEG。WebView 按成品坐标把透明 PNG 显示在底图上。拖动只更新图层位置，无需重新抽帧。以位置、比例和可见性作为预览验收标准。

修改设置后等待约 250 ms 再启动新预览。取消过时的预览进程，界面仅接收最新 `requestId` 的画面。预览失败时提示重试，导出按钮以导出参数校验结果为准。

### 4.6 界面内播放

左侧预览区统一切换「源视频」「水印预览」「成品」，下方显示当前文件的信息。源视频和成品共用内嵌播放器，可暂停、拖动进度和调整音量。通过「水印预览」返回可编辑的静帧，播放源视频时显示原画面和原音轨。

Rust 只在播放时启动监听 `127.0.0.1` 的临时 HTTP 服务，用随机地址授权当前文件，按字节范围传输供播放器快进和回退。系统无法直接解码时，用 FFmpeg 生成最多 720p 的临时 H.264/AAC MP4，完成后播放。切换预览会取消仍在准备的任务。最近的兼容副本在应用内复用，退出时清理。

## 5. 进度、取消和文件写入

### 5.1 任务生命周期

任务按「准备 → 转换 → 写入成品 → 完成」流转。「准备」和「转换」可取消或失败，「写入成品」可完成或失败。Rust 用同一把状态锁管理任务和取消请求。进入「写入成品」前在锁内检查取消状态，进入后禁用取消，避免出现界面显示已取消但成品已经替换的情况。

FFmpeg 启动时传入 `-nostdin -nostats -progress pipe:1`。异步并行读取 stdout 的进度字段和 stderr 的诊断信息，避免管道堵塞。以 `out_time_us / 预计时长（微秒）` 计算单调递增的百分比，在成品提交前最多显示 99%。时长未知时显示不定进度。完成以 FFmpeg 退出成功和成品提交成功为准。 [FFmpeg 命令文档](https://ffmpeg.org/ffmpeg.html)

取消时终止子进程，等待回收后清理任务文件。转换中退出应用时先让用户确认是否取消，确认后等待清理再关闭。stderr 保留有长度限制的诊断文本，界面使用中文错误提示。

### 5.2 保存规则

1. 开始前校验源文件、目标目录和扩展名。目标与源文件相同，或通过符号链接、硬链接指向同一文件时，拒绝导出。
2. 在目标目录建立该任务专用的临时目录，成品先写在其中。这样正式提交时与目标处于同一文件系统。输出封装由格式预设显式指定。
3. FFmpeg 成功退出后检查临时成品存在且非空，再进入提交状态。目标原先不存在时使用平台的排他文件移动，确保同名文件在转码期间出现时也会报冲突。用户确认覆盖时，在提交前重新检查目标身份和修改信息，目标有变更则提示重新选择或确认。目标未变更时使用同文件系统重命名替换。
4. 成功、失败或取消均清理临时文件。失败和取消保留源文件及已有成品。只有提交成功后，界面才显示 100% 和成品入口。

建议定义稳定的错误码，至少覆盖 `InvalidInput`、`UnsupportedVideo`、`InvalidWatermark`、`OutputConflict`、`PermissionDenied`、`DiskFull`、`EncoderFailed`、`MissingBinary`。运行时优先使用明确的文件系统错误和子进程退出结果。无法细分的编码失败显示通用提示，并保留本地诊断信息。

## 6. macOS 构建与发布

1. 开发机安装 Xcode 工具链、Rust stable 的 `aarch64-apple-darwin` target 和 Node.js。提交 Rust 与前端的锁文件，并记录构建使用的版本。
2. 固定 FFmpeg 和 FFprobe 的源码版本、构建参数及产物校验值。面向 macOS 13 构建 arm64 二进制，启用 VideoToolbox、libvpx、libopus 和 zlib。随包验收 `h264_videotoolbox`、`aac`、`libvpx-vp9`、`libopus`、`scale`、`overlay` 可用。将第三方媒体依赖静态集成到 sidecar，或在应用包内完整携带动态库。检查 `otool -L`，确保干净机器可运行。 [FFmpeg 外部依赖说明](https://ffmpeg.org/general.html) [FFmpeg 滤镜文档](https://www.ffmpeg.org/ffmpeg-filters.html)
3. 按 Tauri target triple 命名构建输入 `ffmpeg-aarch64-apple-darwin` 和 `ffprobe-aarch64-apple-darwin`，登记在 `bundle.externalBin`。将字体和许可证材料作为资源打包。用 `npm run build:macos` 生成应用包和 DMG，构建配置中声明最低系统版本 13.0。 [Tauri Sidecar 文档](https://v2.tauri.app/develop/sidecar/) [Tauri DMG 文档](https://v2.tauri.app/distribute/dmg/)
4. 对外发布前，验证主应用、sidecar 和随包动态库的签名，使用 Developer ID 签名并提交 Apple 公证，附上公证票据后在干净的 Apple Silicon Mac 上安装验证。保留 FFmpeg 和各依赖的许可文本、构建配置和对应源码交付材料。按最终构建复核第三方再分发要求。 [Tauri 签名文档](https://v2.tauri.app/distribute/sign/macos/) [Apple 公证文档](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution) [FFmpeg 许可说明](https://ffmpeg.org/legal.html)

### 本地运行和打包

在项目根目录执行以下命令。媒体构建脚本下载固定版本的 FFmpeg、libvpx、Opus、pkgconf 和字体，在工作区内构建 sidecar。

```sh
npm ci
npm run build:media
npm run tauri -- dev
npm run build:macos
```

开发版也支持通过 `VIDEO_TRANS_FFMPEG`、`VIDEO_TRANS_FFPROBE` 和 `VIDEO_TRANS_FONT` 传入本地绝对路径。媒体集成测试使用这三个变量，可用 `cargo test --manifest-path src-tauri/Cargo.toml media_pipeline -- --ignored` 运行。测试 H.264 时需要运行环境允许访问 macOS 的 VideoToolbox 服务。

### Windows 10 和 11（x64）

在 Windows x64 上安装 Node.js、Rust MSVC 工具链、Visual Studio 的「使用 C++ 的桌面开发」组件和 WebView2。安装包会在 WebView2 缺失时下载安装它。 [Tauri Windows 环境与安装包](https://v2.tauri.app/distribute/windows-installer/)

```powershell
rustup target add x86_64-pc-windows-msvc
npm ci
npm run build:windows
```

安装文件位于 `src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/`。也可以在 GitHub Actions 中手动运行 `Windows x64`，通过后从该次运行下载 `video-trans-windows-x64`。安装包包含 FFmpeg、FFprobe 和中文水印字体。MP4 与 MOV 使用 OpenH264 软件编码，WebM 使用 VP9 和 Opus。

## 7. 实施顺序和验收

| 阶段 | 完成标准 |
| --- | --- |
| 1. 验证媒体链路 | Rust 可调用随包二进制探测视频，产出三种格式，通过 FFprobe 校验实际编码、尺寸和音轨。先用短样片验证 VideoToolbox 和中文水印渲染可用。 |
| 2. 完成核心交互 | Tauri 主窗口支持导入、选择格式和分辨率、保存路径、进度、取消和结果入口。 |
| 3. 接入水印 | 文字、图片和静帧预览复用导出处理逻辑，通过横屏、竖屏和透明图片验收。 |
| 4. 验证安装包 | 在干净的目标 Mac 上安装 DMG，验证最低支持系统、Finder 启动和三种格式的完整导出流程。 |

| 验收用例 | 通过条件 |
| --- | --- |
| 有声 MOV 分别转 MP4、WebM、MOV | FFprobe 确认封装、编码和音轨符合预设，播放器中音画同步。 |
| 无声视频、封面图和多音轨视频 | 无声视频可输出，封面不会被选作视频流，多音轨按默认优先规则保留一条。 |
| 横屏、旋转竖屏、超宽和低分辨率 | 三种画面按规则缩至 720p 边界，成品尺寸与界面一致，原片已小于边界时相应档位置灰。 |
| 中文文字及透明 PNG | 中文和常见标点正常显示，透明区域保留，水印位置、比例和透明度符合预览。 |
| 中文、空格和引号文件路径 | 能导入和导出，文件名、扩展名与保存对话框一致。 |
| 取消、不可写目录、损坏文件 | 显示对应状态或提示，源文件和已有成品内容保持完整，任务临时文件已清理。 |
| 同名文件和源文件保护 | 未确认时禁止覆盖，确认后仅在转换成功时替换。转码期间出现的同名文件被保留，直接或间接指向源文件的输出路径被拒绝。 |
| 应用安装 | 未安装 Homebrew 或 FFmpeg 的目标 Mac 上可从 Finder 启动，完成探测、预览和导出。 |
