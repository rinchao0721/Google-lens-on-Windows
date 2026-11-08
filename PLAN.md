# Windows 版 Google Lens 圈选搜索工具：规划文档

> 状态：**技术选型已确定（见第 9 节），剩余问题见第 13 节**。全部确认前不开始编码。
> 项目名：`lenswin`（已确认）。

---

## 1. 目标与范围

### 1.1 MVP 目标
常驻托盘的独立 Windows 程序：
1. 按全局快捷键 → 冻结所有显示器画面。
2. 用户**圈选（自由套索）**或**矩形框选**一个区域。
3. 裁剪 → 上传图床 → 用默认浏览器打开 `搜索引擎模板 + 图片 URL`（默认 Google Lens）。

### 1.2 MVP 同时交付（低成本、验证架构用）
- 截屏：选区 → 复制到剪贴板 / 保存到文件。

### 1.3 MVP 之后的规划内功能（架构必须从第一天预留）
- **托盘设置**：常用选项直接在托盘菜单里切换（见 7.3）
- **图形化桌面设置窗口**（见 7.4，egui）
- **两种分发形式**：zip 便携版 + 安装版（见第 10 节）

### 1.4 远期功能（架构预留，暂不排期）
- 录屏（区域录制、带声音录制）
- OCR 文字识别
- 截图标注编辑
- 点击自动吸附窗口区域

---

## 2. 与原项目（dots-hyprland）的对应关系

| 原项目 | 本项目 |
|---|---|
| Hyprland `hl.bind` 全局快捷键 | `RegisterHotKey`（Win32） |
| `grim -o <屏幕>` 逐屏截图 | `ScreenCapturer` trait（GDI / WGC 实现） |
| Quickshell `ScreencopyView` 冻结画面 + 选区 | `overlay` crate：每显示器一个置顶无边框窗口，显示冻结帧 |
| 圈选：记录拖拽点 → 取外接矩形 + padding | 相同算法，放在 `core::geometry`，纯函数可单测 |
| `magick -crop` | 内存中裁剪 RGBA 缓冲区，不落盘 |
| `curl -F files[]=@` → `jq .files[0].url` | `Uploader` trait；默认 `litterbox` 实现，另有可配置的 `custom` 实现 |
| `xdg-open "<前缀><url>"` | `ShellExecuteW`；引擎改为 `{url}` 模板并做 URL 编码 |
| `SnipAction` 枚举（Copy/Edit/Search/OCR/Record） | `ActionKind` 枚举 + `Action` trait + 注册表 |

改进点：原项目把图片 URL 直接拼接、未编码；临时文件落盘在 `/tmp`。本项目全程内存处理，URL 做百分号编码。

---

## 3. 总体架构

### 3.1 分层

| 层 | crate | 职责 | 允许依赖 |
|---|---|---|---|
| 入口 | `app`（bin） | 托盘、快捷键、命令路由、配置热更新、日志、单实例 | 下面所有 crate |
| 功能 | `actions` | search / copy / save /（ocr）/（record） | upload、overlay、platform、core |
| 功能 | `settings_ui` | 设置窗口，只负责编辑并提交 `Config` | platform、core |
| 能力 | `upload` | 图床抽象 | core |
| 能力 | `overlay` | 选区界面 | core |
| 能力 | `platform` | Win32 / WinRT 封装 | core |
| 基础 | `core` | 纯逻辑，零平台依赖，可单测 | 无 |

`actions` 与 `settings_ui` 互不依赖；`upload`、`overlay`、`platform` 三者互不依赖。

**依赖方向只能自上而下**。`core` 不依赖任何 Windows API；`overlay` 和 `platform` 之间互不依赖时最好，若 overlay 需要显示器信息，则通过 `core` 中定义的数据类型传入。

### 3.2 一次「圈选搜索」的数据流

```
热键 WM_HOTKEY
  → app 根据配置查出 ActionKind::Search
  → platform::capture 抓取所有显示器 → Vec<Frame>
  → overlay::select(frames, SelectionMode::Lasso) → Option<Selection>   （Esc 返回 None）
  → core::image::crop(frame, selection.bounds) → CroppedImage
  → actions::search 在工作线程：编码 PNG → Uploader::upload → 拼 URL → platform::shell::open_url
  → 失败时 platform::notify 弹托盘气泡提示
```

### 3.3 线程模型
- **主线程**：Win32 消息循环（热键、托盘、覆盖层窗口全部在此）。
- **工作线程**：网络上传、PNG 编码、文件写入。使用 `std::thread` + `std::sync::mpsc`，结果通过 `PostMessageW` 回投主线程。
- 项目代码不直接使用 async 运行时。`reqwest::blocking` 内部自带的 tokio 由 reqwest 管理，对项目代码透明，不算违反。
- 每次上传在工作线程内调用 blocking 客户端；`reqwest::blocking::Client` 全局只建一个，复用连接池。

---

## 4. 目录结构

```
lenswin/
├── Cargo.toml                 # workspace 定义、统一依赖版本、统一 lint
├── rust-toolchain.toml        # 锁定 stable + MSVC 目标
├── rustfmt.toml
├── clippy.toml
├── AGENTS.md                  # 智能体编码规范
├── PLAN.md                    # 本文档
├── assets/
│   ├── app.ico
│   └── app.manifest           # Per-Monitor DPI V2、Common Controls v6
├── crates/
│   ├── core/                  # lenswin-core
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── geometry.rs    # Point / Rect / 坐标换算 / 套索外接矩形
│   │       ├── selection.rs   # Selection / SelectionMode / SelectionShape
│   │       ├── frame.rs       # Frame（RGBA 像素 + 所在虚拟桌面区域）、裁剪
│   │       ├── action.rs      # ActionKind / Action trait / ActionInput / ActionOutcome
│   │       ├── config/
│   │       │   ├── mod.rs     # Config 结构、默认值
│   │       │   └── validate.rs# 配置校验
│   │       └── error.rs
│   ├── platform/              # lenswin-platform（仅 cfg(windows)）
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── capture/
│   │       │   ├── mod.rs     # ScreenCapturer trait
│   │       │   ├── gdi.rs     # BitBlt 实现（MVP）
│   │       │   └── wgc.rs     # Windows Graphics Capture（录屏阶段）
│   │       ├── monitor.rs     # 枚举显示器、DPI
│   │       ├── hotkey.rs      # 快捷键解析与注册
│   │       ├── tray.rs        # 托盘图标与菜单
│   │       ├── clipboard.rs
│   │       ├── shell.rs       # 打开 URL
│   │       ├── notify.rs      # 托盘气泡通知
│   │       ├── paths.rs       # 便携/安装模式判定，配置与日志目录
│   │       ├── autostart.rs   # 开机自启（HKCU\...\Run）
│   │       ├── single_instance.rs
│   │       ├── ipc.rs         # 进程间消息：WM_COPYDATA 收发
│   │       └── handle.rs      # Win32 句柄 RAII 包装
│   ├── overlay/               # lenswin-overlay
│   │   └── src/
│   │       ├── lib.rs         # select(frames, mode) -> Option<Selection>
│   │       ├── window.rs      # 每显示器覆盖层窗口
│   │       ├── input.rs       # 鼠标/键盘状态机
│   │       ├── render.rs      # 冻结帧、遮罩、选框/套索绘制
│   │       └── modes/
│   │           ├── mod.rs     # SelectionTool trait
│   │           ├── rect.rs
│   │           └── lasso.rs
│   ├── upload/                # lenswin-upload
│   │   └── src/
│   │       ├── lib.rs         # Uploader trait、UploadError、按配置构造上传器
│   │       ├── litterbox.rs   # 默认图床，可设过期时间
│   │       └── custom.rs      # 自定义端点：字段名 + 响应解析规则
│   ├── settings_ui/           # lenswin-settings-ui（eframe + egui）
│   │   └── src/
│   │       ├── lib.rs         # open(config) -> 用户提交的新 Config
│   │       ├── pages/         # 常规 / 快捷键 / 搜索与上传 / 截屏 / 关于
│   │       └── hotkey_input.rs# 快捷键录入控件
│   └── actions/               # lenswin-actions
│       └── src/
│           ├── lib.rs         # ActionRegistry
│           ├── search.rs      # 以图搜图
│           ├── copy.rs        # 复制到剪贴板
│           ├── save.rs        # 保存文件
│           ├── ocr.rs         # （后续）
│           └── record.rs      # （后续）
├── app/                       # lenswin（bin）
│   ├── build.rs               # 嵌入图标、manifest、版本信息
│   └── src/
│       ├── main.rs            # 入口：按启动参数分发到托盘模式或设置模式
│       ├── ipc_server.rs      # 托盘进程接收设置进程提交的配置
│       ├── privacy.rs         # 首次上传前的隐私确认
│       ├── dispatcher.rs      # 热键/托盘命令 → Action 执行
│       ├── config_store.rs    # 配置读写、原子保存、变更广播
│       ├── tray_menu.rs       # 根据 Config 生成托盘菜单
│       └── logging.rs
└── packaging/
    ├── build-portable.ps1     # 生成 zip 便携版
    ├── build-installer.ps1    # 调用安装包工具生成安装版
    └── installer/             # 安装包脚本（Inno Setup 脚本 lenswin.iss）
```

---

## 5. 核心抽象（接口草图，仅示意签名）

```rust
// core::selection
pub enum SelectionMode { Rect, Lasso }

pub enum SelectionShape {
    Rect,
    Lasso(Vec<Point>),          // 保留原始轨迹，后续可做蒙版
}

pub struct Selection {
    pub shape: SelectionShape,
    pub bounds: Rect,            // 虚拟桌面物理像素坐标
    pub monitor_index: usize,
}

// core::action
pub enum ActionKind { Search, Copy, Save, Ocr, Record, RecordWithAudio }

pub enum InputKind { Image, Region }          // 截屏类要图像，录屏类要区域

pub enum ActionInput {
    Image(CroppedImage),
    Region(Selection),
}

pub enum ActionOutcome {
    Done,
    Session(Box<dyn RunningSession>),         // 录屏等长任务，可 stop()
}

pub trait Action: Send + Sync {
    fn kind(&self) -> ActionKind;
    fn input_kind(&self) -> InputKind;
    fn preferred_mode(&self) -> SelectionMode;
    fn run(&self, ctx: &ActionContext, input: ActionInput) -> Result<ActionOutcome, ActionError>;
}

// platform::capture
pub trait ScreenCapturer {
    fn capture_all(&self) -> Result<Vec<Frame>, CaptureError>;
}

// upload
pub trait Uploader: Send + Sync {
    fn upload(&self, png: &[u8], file_name: &str) -> Result<Url, UploadError>;
}

// overlay::modes
pub trait SelectionTool {
    fn on_press(&mut self, p: Point);
    fn on_move(&mut self, p: Point);
    fn on_release(&mut self, p: Point) -> Option<Selection>;
    fn draw(&self, canvas: &mut Canvas);
}
```

**圈选算法**（与原项目一致）：记录拖拽轨迹 → 求 min/max x、y → 外扩 `padding + stroke_width / 2` → 与所在显示器边界求交 → 宽或高为 0 时视为无效选区。

---

## 6. 扩展机制

新增一个功能，只需要做这几件事，**不需要改动其它功能的代码**：

| 新功能 | 需要做的事 |
|---|---|
| 新动作（如 OCR） | `actions/src/ocr.rs` 实现 `Action` → 在 `ActionRegistry` 注册 → 配置 `[hotkeys]` 加一项 |
| 录屏 | `platform/capture/wgc.rs` 提供连续帧 → `actions/src/record.rs` 返回 `ActionOutcome::Session` → 托盘显示「停止录制」 |
| 新图床 | `upload/src/<name>.rs` 实现 `Uploader` → 配置 `provider` 加枚举值 |
| 新搜索引擎 | 只改配置 `engine_url_template`，不改代码 |
| 新选区工具（如窗口吸附） | `overlay/src/modes/<name>.rs` 实现 `SelectionTool` |

后续功能技术路线（预研结论，到时再细化）：
- **OCR**：优先用系统自带 `Windows.Media.Ocr`（WinRT），无需 tesseract。
- **录屏**：WGC 抓帧 + Media Foundation `IMFSinkWriter` 编码 H.264/MP4；声音用 WASAPI loopback。
- **标注**：独立窗口，复用 overlay 的渲染模块。

---

## 7. 配置

位置由运行模式决定（见第 10 节）：安装版在 `%APPDATA%\lenswin\config.toml`，便携版在 `<exe 目录>\data\config.toml`。首次启动不存在时写入默认值；解析失败时**不覆盖**原文件，改用默认值运行，并弹通知提示。

```toml
[hotkeys]
search = "Win+Shift+A"
screenshot_copy = "Win+Shift+C"

[search]
engine_url_template = "https://lens.google.com/uploadbyurl?url={url}"
selection_mode = "lasso"          # lasso | rect

[upload]
provider = "litterbox"            # litterbox | custom
timeout_secs = 30
max_bytes = 104857600

[upload.litterbox]
expiry = "1h"                     # 1h | 12h | 24h | 72h

# provider = "custom" 时生效；以下示例等价于原项目的 uguu.se
[upload.custom]
endpoint = "https://uguu.se/upload"
file_field = "files[]"
response = "json"                 # json | text
json_pointer = "/files/0/url"     # response = "json" 时必填，RFC 6901 语法

[selection.lasso]
padding = 10
stroke_width = 4

[screenshot]
save_dir = ""                     # 为空则只复制到剪贴板
```

校验规则：
- `engine_url_template` 必须包含且仅包含一个 `{url}`，并且是 http/https。
- 数值项有上下限；快捷键字符串可解析且不重复。
- `custom.endpoint` 必须是 http/https；`file_field` 非空；`response = "json"` 时 `json_pointer` 必须以 `/` 开头。
- 上传返回的 URL 不论来自哪个图床，都必须解析成功且为 http/https，否则视为上传失败。

### 7.1 配置有三个修改入口，统一走 ConfigStore

托盘菜单、设置窗口、手动编辑文件，最终都汇总到 `app::config_store`：

```
托盘点选 ─┐
设置进程 ─┼→ ConfigStore::apply(new_config) → 校验 → 原子写盘 → 广播 ConfigChanged
文件变化 ─┘                                              │
                     ┌───────────────────────────────────┤
                     ▼                 ▼                 ▼
               重新注册快捷键     重建 Uploader      重建托盘菜单
```

- **校验失败**：拒绝提交，保持旧配置，告诉用户哪一项不合法。设置窗口里就地标红。
- **原子写盘**：先写同目录临时文件，再 `ReplaceFileW` / `MoveFileExW` 替换，避免写一半断电损坏。
- **快捷键重注册失败**（被别的程序占用）：回滚到旧快捷键，并提示冲突项。
- **手动编辑文件**：托盘菜单提供「重新加载配置」。是否自动监听文件变化，放到实现阶段再定。
- 配置由程序完整写回，**手写在 config.toml 里的注释会丢失**。配置项说明放在设置窗口和文档里。

### 7.2 Config 的结构要求（为 GUI 预留）
- 每个配置项都有默认值、取值范围和校验函数，集中定义在 `core::config`，托盘和设置窗口共用，不各自重复写校验。
- 枚举型选项（选区模式、过期时间、图床类型）用 Rust `enum`，UI 直接从枚举生成选项列表。
- 预置搜索引擎列表（Google Lens、Bing、Yandex、TinEye 等）放在 `core`，用户也可以填自定义模板。

### 7.3 托盘菜单（已确认）

```
lenswin
├── 圈选搜索                Win+Shift+A
├── 截屏到剪贴板            Win+Shift+C
├── ─────────
├── 选区模式           ▸   ● 圈选  ○ 矩形
├── 搜索引擎           ▸   ● Google Lens  ○ Bing  ○ Yandex  ○ TinEye  ○ 自定义
├── 图片保留时间       ▸   ● 1 小时  ○ 12 小时  ○ 24 小时  ○ 72 小时
├── ─────────
├── 开机自启                ☑
├── 设置…
├── 打开配置文件夹
├── 重新加载配置
├── ─────────
├── 关于 lenswin（版本 · 便携版/安装版）
└── 退出
```

托盘只放「高频、单选、切换即生效」的项；需要输入文本的项（自定义端点、快捷键）只在设置窗口里改。

### 7.4 设置窗口（egui / eframe）
- 单实例：已打开时再次点击只是激活窗口。
- 分页：常规（开机自启、通知）/ 快捷键 / 搜索与上传 / 截屏 / 关于。
- 「应用」与「确定」把新配置发给托盘进程，由托盘进程的 `ConfigStore::apply` 处理；「取消」丢弃修改。
- 关闭设置窗口只结束设置进程，托盘进程继续常驻。
- egui 代码只允许出现在 `settings_ui` crate；选区覆盖层不用 egui。

**运行方式（D8-B）**：设置窗口是独立进程，详见 7.5。这样也避开了 winit 一个进程只能创建一次事件循环的限制。

**已知的坑，实现时必须处理：**
- **中文字体**：egui 自带字体不含中文。启动设置窗口时从系统字体目录加载「微软雅黑」（`msyh.ttc`，取集合中的第 0 个字体），找不到时依次回退到其它中文字体，全部失败则提示。字体目录通过 `platform` 获取。
- **Win 键**：egui 的修饰键模型不包含 Windows 键，按键录制拿不到 `Win`。因此快捷键录入采用 D9-A：修饰键复选框 + 主键下拉框。
- **DPI**：eframe 自己处理每显示器 DPI，不需要额外代码；设置窗口的初始尺寸用逻辑像素。
- **渲染后端**：用 `glow`（OpenGL），不用 `wgpu`，以控制体积和启动时间。

### 7.5 设置进程与托盘进程的通信

```
托盘菜单「设置…」
  → 托盘进程以 `--settings` 参数启动自身 exe
  → 设置进程：读取配置（与托盘进程同一套 platform::paths 规则）→ 显示窗口
  → 用户点「应用」
  → 设置进程先用 core::config 校验，不通过就在窗口里标红，不发送
  → 通过后序列化为 TOML，经 WM_COPYDATA 同步发送给托盘进程的隐藏消息窗口
  → 托盘进程：反序列化 → ConfigStore::apply（再校验、注册快捷键、原子写盘、广播）
  → 返回结果码（成功 / 校验失败 / 快捷键冲突 / 写盘失败）
  → 设置进程按结果码提示；失败时保持窗口内容不变，方便用户修改
```

- **托盘进程是配置文件的唯一写入者**。设置进程从不写盘，所以不会出现两个进程同时写配置。
- 用 `WM_COPYDATA` + `SendMessageTimeoutW`：同步调用，返回值就是结果码，不需要另建回传通道；超时（如 5 秒）视为托盘进程无响应并提示。
- 托盘进程的隐藏消息窗口用固定窗口类名，设置进程通过 `FindWindowW` 定位。找不到时提示「主程序未运行」，不直接写配置。
- 托盘进程对收到的数据**一律不信任**：校验 `dwData` 消息类型和数据长度（上限 1 MiB），解析失败或校验失败都返回错误码，不 panic。
- 单实例：托盘进程用互斥量 `Local\lenswin`，设置进程用 `Local\lenswin-settings`。设置进程重复启动时，激活已有窗口后退出。
- 不带参数再次启动 exe 时，如果托盘进程已在运行，就通知它弹出托盘气泡「lenswin 已在运行」，然后退出。
- 消息类型和结果码定义在 `core`，Win32 收发封装在 `platform::ipc`。

### 7.6 首次上传前的隐私确认
- 第一次执行搜索时，在上传**之前**弹出确认框（Win32 `TaskDialogIndirect`）：说明截图会上传到哪个图床、是否公开、多久后过期，按钮为「继续」和「取消」。
- 用户点「继续」后，把确认过的图床主机名记入配置（`privacy.acknowledged_host`），之后不再弹。
- 切换图床或修改自定义端点的主机名后，重新弹一次。
- 用户点「取消」：本次不上传，丢弃截图。

---

## 8. 依赖清单（**每一项都需要你批准**）

| crate | 用途 | 备注 |
|---|---|---|
| `windows` | Win32 / WinRT 全部调用 | 微软官方 |
| `serde` + `toml` | 配置读写 | |
| `thiserror` | 库 crate 错误类型 | |
| `anyhow` | 仅 `app` 顶层错误汇总 | |
| `png` | 编码上传用的 PNG | 比 `image` 轻得多 |
| `reqwest` | multipart 上传 | 只开 `blocking`、`multipart`、`rustls-tls` 特性，关闭默认特性（不用 native-tls） |
| `serde_json` | 解析图床返回，`Value::pointer` 实现 JSON 指针 | |
| `log` + `simplelog` | 写日志文件 | GUI 程序无控制台 |
| `embed-resource`（构建依赖） | 嵌入图标、manifest | |
| `eframe`（含 `egui`） | 设置窗口 | 关闭默认特性，只开 `glow`、`default_fonts`、`accesskit`（屏幕阅读器支持）；只能被 `settings_ui` 使用 |

非 crate 的构建工具：**Inno Setup 6**（只在打包机上需要，用于生成安装版）。

托盘、快捷键、剪贴板、单实例**都直接调用 `windows` crate**，不引入 `tray-icon`、`global-hotkey`、`arboard` 之类的封装库。

---

## 9. 技术选型

### 9.0 决策结果

| 编号 | 选定 | 结论 |
|---|---|---|
| D1 | A | 选区覆盖层用纯 Win32 + Direct2D |
| D2 | A | MVP 用 GDI BitBlt，录屏阶段再加 WGC，通过 `ScreenCapturer` 切换 |
| D3 | B | HTTP 用 `reqwest` blocking |
| D4 | B + C | 默认 `litterbox`（过期时间可配）；同时提供自定义端点 |
| D5 | A | Cargo workspace 多 crate |
| D6 | B | 设置窗口用 egui（eframe），选区覆盖层仍为纯 Win32 |
| D7 | A | 安装包用 Inno Setup；安装时由用户选择「当前用户」或「所有用户」 |
| D8 | B | 设置窗口作为独立进程运行（`lenswin.exe --settings`） |
| D9 | A | 快捷键录入用修饰键复选框 + 主键下拉框 |

以下为当时的方案对比，保留备查。

### D1 选区覆盖层的 UI 技术
| 方案 | 说明 | 优点 | 缺点 |
|---|---|---|---|
| **A. 纯 Win32 + Direct2D**（推荐） | 用 `windows` crate 创建窗口，Direct2D 绘制 | 最原生；每显示器 DPI 精准；零额外 GUI 依赖；启动快 | 代码量大，`unsafe` 较多 |
| B. winit + softbuffer + tiny-skia | 跨平台窗口库 + CPU 绘制 | 写起来简单，以后可移植 | 多显示器置顶全屏和混合 DPI 有坑；托盘和热键仍要用 `windows`；依赖多 |
| C. egui / eframe | 即时模式 GUI | 开发快，以后做设置界面方便 | 全屏透明覆盖层不是它的强项；体积和启动延迟较大 |

### D2 截屏后端
| 方案 | 说明 | 优点 | 缺点 |
|---|---|---|---|
| **A. MVP 用 GDI BitBlt，录屏阶段再加 WGC**（推荐） | 通过 `ScreenCapturer` trait 切换 | MVP 简单可靠；架构不锁死 | HDR 显示器上颜色可能偏灰 |
| B. 直接用 Windows Graphics Capture | 现代捕获 API | 支持 HDR；截屏和录屏共用一套代码 | 要求 Win10 1903+；异步首帧，代码复杂；Win11 以下会有黄色边框 |
| C. DXGI Desktop Duplication | 桌面复制 API | 性能最高 | 需要处理访问丢失、屏幕旋转、桌面切换，复杂度最高 |

### D3 HTTP 客户端
| 方案 | 说明 | 优点 | 缺点 |
|---|---|---|---|
| **A. `ureq`**（推荐） | 同步、轻量、rustls | 依赖少，和「工作线程 + 同步调用」模型契合 | 功能比 reqwest 少（够用） |
| B. `reqwest`（blocking） | 生态最全 | 文档多 | 间接引入 tokio/hyper，体积大 |
| C. WinHTTP（`windows` crate） | 系统自带 | 零第三方 HTTP 依赖，走系统代理 | 手写 multipart，代码冗长 |

### D4 默认图床
| 方案 | 说明 | 优点 | 缺点 |
|---|---|---|---|
| A. `uguu.se` | 与原项目一致 | 无需账号，3 小时自动过期 | 截图会被临时公开；第三方可用性不可控 |
| B. `litterbox.catbox.moe` | 可选 1h/12h/24h/72h 过期 | 过期时间可控 | 同样公开；同样不可控 |
| C. 自定义端点 | 用户填 endpoint 和返回 URL 的 JSON 路径 | 可用自建图床，隐私最好 | 需要用户自备服务 |

无论选哪个，`Uploader` trait 都保留，后续可以加其它实现。

### D5 工程组织
| 方案 | 说明 | 优点 | 缺点 |
|---|---|---|---|
| **A. Cargo workspace 多 crate**（推荐，即第 4 节结构） | 按层拆分 crate | 依赖方向由编译器强制；`core` 可在任意平台单测；增量编译快 | 初期文件多一些 |
| B. 单 crate 多模块 | 一个 crate 内分模块 | 起步最快 | 分层只靠自觉，后期容易互相引用 |

### D6 设置窗口技术
| 方案 | 说明 | 优点 | 缺点 |
|---|---|---|---|
| **A. 原生 Win32 对话框 + Common Controls v6**（推荐） | Tab、Edit、ComboBox、CheckBox，快捷键录入直接用系统 `msctls_hotkey32` 控件 | 与 D1 技术栈一致；零新增依赖；体积和启动最小；系统原生无障碍支持 | 布局要手写，代码量大；外观偏传统，深色模式支持有限 |
| B. 设置窗口单独用 egui | 只有设置窗口用 egui，覆盖层仍是 Win32 | 开发快；外观和深色模式容易做好 | 新增依赖和第二套渲染栈；exe 增加数 MB；需要独立线程跑它的事件循环 |
| C. WebView2 + 内嵌 HTML | 用 `webview2-com` 承载本地 HTML 页面 | 界面最现代、最灵活 | 依赖 WebView2 运行时（Win11 自带，Win10 多数已装）；内存占用高；要写 JS↔Rust 消息桥 |

### D7 安装包工具
| 方案 | 说明 | 优点 | 缺点 |
|---|---|---|---|
| **A. Inno Setup**（推荐） | 生成 setup.exe | 脚本简单；内置中文界面；支持免管理员的当前用户安装；`AppMutex` 能检测正在运行的实例 | 打包机需要装 Inno Setup；不产出 MSI |
| B. WiX Toolset（MSI） | 用 `cargo-wix` 生成 MSI | 企业环境和组策略友好 | XML 繁琐；当前用户级 MSI 不好做 |
| C. NSIS | 生成 setup.exe | 安装包最小；可定制性强 | 脚本语言老旧难维护 |

安装范围：**由用户在安装时选择**（见 10.3）。

### D8 设置窗口的运行方式
| 方案 | 说明 | 优点 | 缺点 |
|---|---|---|---|
| **A. 进程内常驻 UI 线程**（推荐） | 第一次打开时创建专用线程，之后每次打开都在这个线程上重新运行 eframe（eframe 会复用同一个事件循环） | 直接调用 `ConfigStore::apply`，快捷键冲突等错误能当场显示在窗口里；只有一个进程 | 关窗后事件循环仍留在线程里，有少量常驻内存；UI 线程 panic 必须捕获，不能拖垮托盘 |
| B. 独立进程 `lenswin.exe --settings` | 设置窗口是同一个 exe 的另一种启动模式，保存后通知主进程重新加载 | 完全隔离，崩溃不影响托盘；关窗后内存全部释放 | 要做进程间通信；快捷键冲突要等主进程应用后才能回传，错误提示链路更长 |

### D9 快捷键录入方式
| 方案 | 说明 | 优点 | 缺点 |
|---|---|---|---|
| **A. 修饰键复选框 + 主键下拉框**（推荐） | Ctrl / Alt / Shift / Win 四个复选框，加一个主键下拉框 | 实现简单可靠，绕开 egui 拿不到 Win 键的问题 | 操作比「按一下就录好」慢 |
| B. 按键录制 | 点击输入框后直接按组合键 | 体验最好 | 需要在设置窗口里另装低级键盘钩子才能拿到 Win 键，实现复杂 |
| C. 录制 + 复选框修正 | 先录制，再用复选框补上没录到的修饰键 | 兼顾体验和可靠 | 界面元素多，两套交互都要做 |

---

## 10. 分发：便携版与安装版

### 10.1 同一个 exe，运行时判定模式
两种分发用**同一个编译产物**，不做条件编译。启动时由 `platform::paths` 判定：exe 同目录存在 `portable` 标记文件即为便携版，否则为安装版。

| 项 | zip 便携版 | 安装版 |
|---|---|---|
| 模式判定 | exe 旁有 `portable` 文件（zip 自带） | 没有标记文件 |
| 配置 | `<exe 目录>\data\config.toml` | `%APPDATA%\lenswin\config.toml` |
| 日志 | `<exe 目录>\data\logs\` | `%LOCALAPPDATA%\lenswin\logs\` |
| 开机自启 | 写 `HKCU\...\Run`，记录当前 exe 绝对路径；启动时发现路径变了，提示是否更新 | 写 `HKCU\...\Run`；卸载时由安装器删除 |
| 开始菜单 / 桌面快捷方式 | 无 | 有（桌面快捷方式可选） |
| 升级 | 覆盖解压，`data\` 保留 | 运行新安装包覆盖安装；通过 `AppMutex` 提示先退出程序 |
| 卸载 | 删除文件夹（先在托盘里关掉开机自启） | 系统「应用」里卸载，询问是否保留配置 |

- 便携版目录不可写时（例如被放进 `Program Files`）：弹通知说明原因，本次改用安装版路径运行，不静默失败。
- 两种模式共用同一个单实例互斥量名称：同一用户下不能同时运行便携版和安装版。
- 「关于」页和托盘「关于」显示当前模式和版本号。
- 代码中**禁止**硬编码任何目录，一律通过 `platform::paths` 获取。

### 10.2 打包流程
- 版本号唯一来源：根 `Cargo.toml` 的 `[workspace.package] version`；`build.rs` 写入 exe 版本信息，打包脚本从这里读取。
- `packaging/build-portable.ps1`：`cargo build --release` → 组装 `lenswin.exe`、`portable`、`README.txt`、`LICENSE` → 生成 `lenswin-<版本>-portable-x64.zip`。只用 PowerShell 自带的 `Compress-Archive`，不引入额外工具。
- `packaging/build-installer.ps1`：调用 Inno Setup 命令行编译器 `ISCC.exe`，生成 `lenswin-<版本>-setup-x64.exe`。
- 代码签名暂不做，放到后续（见第 12 节风险）。

### 10.3 安装版：安装范围由用户选择
Inno Setup 设置 `PrivilegesRequired=lowest` 和 `PrivilegesRequiredOverridesAllowed=dialog`，安装开始时弹窗让用户选：

| 项 | 仅当前用户 | 所有用户 |
|---|---|---|
| 是否需要管理员 | 否 | 是（UAC 提示） |
| 安装目录（`{autopf}`） | `%LOCALAPPDATA%\Programs\lenswin` | `C:\Program Files\lenswin` |
| 开始菜单（`{autoprograms}`） | 当前用户 | 所有用户 |
| 卸载入口 | `HKCU` | `HKLM` |
| 配置与日志 | 各用户自己的 `%APPDATA%` / `%LOCALAPPDATA%` | 同左（配置永远按用户隔离） |
| 开机自启 | 在托盘里开启，写 `HKCU\...\Run` | 同左（每个用户自己决定） |

- 卸载时询问是否删除配置：只能删除**执行卸载的那个用户**的配置；其他用户的配置不动，在卸载提示里说明。
- 升级时安装器沿用上次选择的范围和目录（Inno Setup 自动记录），不再重复询问。
- 单实例互斥量用当前会话内的 `Local\` 命名空间；安装脚本的 `AppMutex` 填同一个名字，用来检测程序是否在运行。
- 安装目录里永远不会出现 `portable` 标记文件；安装脚本在安装时主动删除同名残留文件，防止误判为便携版。

---

## 11. 里程碑

| 阶段 | 内容 | 完成标准 |
|---|---|---|
| M0 环境 | 安装 Rust stable（MSVC）；建 workspace 骨架、lint 配置 | `cargo build`、`cargo clippy -D warnings` 通过 |
| M1 core | 几何、选区、裁剪、配置结构及校验 | 单元测试覆盖套索外接矩形、越界裁剪、配置非法值 |
| M2 platform | 显示器/DPI、GDI 截屏、热键、基础托盘、剪贴板、打开 URL、单实例、**便携/安装路径判定** | 托盘常驻，热键能触发日志；两种模式的路径都正确 |
| M3 overlay | 冻结画面、矩形和套索两种工具、Esc 取消、多显示器 | 在混合 DPI 双屏上手动验证选区坐标正确 |
| M4 actions | 搜索（上传 + 打开 Lens）、复制、保存 | **MVP 可用**：端到端走通；断网、图床报错有提示且不崩溃 |
| M5 配置中心 + 托盘设置 | `ConfigStore`、原子保存、热更新、完整托盘菜单、开机自启 | 托盘切换的选项立即生效且重启后保留；快捷键冲突能回滚 |
| M6 设置窗口 | 用 egui 实现全部设置页；中文字体、DPI、单窗口复用 | 所有配置项都能在窗口里修改并校验 |
| M7 分发 | 图标、版本信息、便携版 zip、安装版 | 两种包在干净的 Win10、Win11 上安装、升级、卸载都正常 |
| 后续 | OCR → 录屏 → 标注 → 窗口吸附 | 按需另行规划 |

---

## 12. 风险

| 风险 | 影响 | 应对 |
|---|---|---|
| Google 调整 `uploadbyurl` 入口 | 搜索失效 | 引擎是配置项，可改用其它引擎；必要时再研究直传 |
| 图床不可用或限流 | 搜索失效 | `Uploader` 可替换；错误要明确提示 |
| 混合 DPI / 负坐标多屏 | 选区错位 | 统一使用物理像素；M3 专项测试 |
| 快捷键被占用 | 注册失败 | 启动时提示具体冲突的快捷键；设置里修改失败时回滚 |
| 未签名 exe + 截屏 + 全局热键 | 杀毒软件误报、SmartScreen 拦截安装包 | 发布说明里写清楚；后续考虑代码签名 |
| 便携版被移动目录 | 开机自启指向旧路径 | 启动时检测并提示更新 |
| 程序写回配置 | 用户手写的注释丢失 | 在文档和设置窗口里说明配置项含义 |

---

## 13. 决策记录

### 13.1 已确认
- 技术选型 D1–D9 见 9.0。
- 托盘菜单按 7.3 实现。
- MVP 做到 M4；托盘设置、设置窗口、两种分发放在 M5–M7。
- 便携版用 exe 旁的 `portable` 标记文件判定。
- 安装版的安装范围由用户在安装时选择。
- 新增依赖 `eframe` 已批准（glow 后端，特性见第 8 节）。
- 首次上传前弹隐私确认（见 7.6）。
- 项目名和 exe 名：`lenswin`。
- 默认快捷键：搜索 `Win+Shift+A`，截屏复制 `Win+Shift+C`。

### 13.2 按理解记录、如有误请指出
- 「a，a，b，b+c，a」按顺序对应 D1–D5。
- D9 回复「可以」，按推荐方案 A（修饰键复选框 + 主键下拉框）记录。

### 13.3 构建环境（已就绪，2026-11-08）
- Rust：rustup 1.29.3，默认工具链 `stable-x86_64-pc-windows-msvc`（rustc 1.99.1），含 clippy、rustfmt。
- MSVC：在现有 Visual Studio Community 2026（`F:\Microsoft Visual Studio`）上补装了 `VC.Tools.x86.x64`（MSVC 14.51）；Windows SDK 10.0.26100 / 10.0.28000 已存在。
- Inno Setup 6 到 M7 才需要，现在不装。
