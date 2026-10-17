# Clarion

**A modern Wayland desktop with a native Vulkan backend.**

Clarion 是用 Rust 写的 Wayland 合成器：Vulkan 原生合成、设备无关的桌面核心、KMS 逐帧预算调度。它融合浮动窗口、滚动平铺与动态平铺，提供液态玻璃视觉、自适应帧调度和细腻动画，专注低延迟交互与流畅的桌面体验。

## 原生 Vulkan 合成

Clarion 在支持的物理主显卡输出上直接使用 Vulkan 完成桌面合成，并通过 DRM/KMS 呈现。

- **原生渲染路径**：基于 `ash`，支持窗口、阴影、裁剪、模糊、液态玻璃及离屏效果。
- **异步完成回收**：GPU 提交、完成通知和资源生命周期分离，有界管理在途工作，减少合成器主线程等待。
- **按输出调度**：根据目标呈现时间、CPU 帧工作和硬件反馈维护独立帧预算；动画按目标时间取样，打断与反向保留运动状态。
- **同步与损伤管理**：支持 DMA-BUF、按硬件能力启用的显式同步、FIFO 与定时提交；效果撤销会处理旧区域与新区域的重绘。
- **可观测性能**：分别记录主线程 CPU、GPU 命令缓冲和硬件呈现间隔，保留异常样本及统计完整性信息。

启用 Vulkan：

```sh
clarion --renderer vulkan
```

也可在 `~/.config/clarion/config.kdl` 顶层设置：

```kdl
renderer "vulkan"
```

渲染器选择在合成器启动时生效。默认使用 Vulkan；驱动、设备或输出不满足要求时自动回退到 GLES，部分兼容与多 GPU 路径也由 GLES 承担。请以启动日志中的实际后端为准。

[Vulkan 验证说明](docs/vulkan-validation.md) · [性能诊断](docs/latency-diagnostics.md)

## 独立桌面核心

`clarion-core` 以纯 Rust 管理桌面状态与数值策略，不依赖 Smithay、Wayland 或 GPU。
窗口与桌面空间身份、焦点、最小化、布局及交互状态由核心管理；动画数学、呈现调度和场景策略各自编译为独立模块。
场景快照与设备提交分离，协议和 GPU 资源由适配层管理。

| 模块 | 职责 |
| --- | --- |
| `clarion-core` | 桌面模型、纯布局策略与交互状态 |
| `clarion-motion` | 曲线、弹簧、动画轨迹与运动时钟 |
| `clarion-timing` | 每输出呈现预测、帧预算与调度状态机 |
| `clarion-scene` / `src/scene` | 纯场景策略、受限场景组装与不可变快照 |
| `src/platform` | 客户端、输入、协议及设备事件适配 |
| `src/render_helpers/vulkan` | 原生 Vulkan 渲染、同步与完成回收 |
| `clarion-shell` | 顶部栏、Dock、启动器与桌面界面 |
| `clarion-launcher` | 独立 Rust/GPUI Tahoe Apps 面板、XDG 应用目录与启动服务 |
| `clarion-settings` | 配置、窗口管理与性能诊断 |
| `clarion-ipc` | 合成器、Shell 与工具间的接口 |

通用图形与输入基础设施使用 Smithay、DRM/KMS、libinput、Wayland 和 Vulkan。

[模块边界与所有权](ARCHITECTURE.md) · `python3 scripts/check_module_boundaries.py`

## 窗口与桌面

- **三种布局**：浮动、无限横向滚动平铺和动态二叉切分平铺；支持列内混合布局与模式切换。
- **空间组织**：工作区、舞台概览、二维工作区总览、窗口搜索与展开。
- **自由画布**：二维平移、触控板惯性和视口恢复。
- **可打断动效**：弹簧、贝塞尔曲线、缩放过渡及窗口开关效果；提供减少动态选项。
- **日常集成**：Xwayland、截图、录屏、全局应用菜单、窗口与工作区 IPC。

[功能与配置](docs/wiki/Clarion-Features.md) · [默认配置](resources/default-config.kdl) · [配置文档](docs/wiki/Configuration:-Introduction.md)

## 集成 Shell

Clarion Shell 提供顶部栏、Dock、应用启动器、控制中心、通知中心、托盘与桌面文件视图。
支持 Material 与玻璃风格界面，通过 layer-shell 和 Clarion IPC 连接合成器。
Flutter 负责客户端界面绘制；桌面合成、背景模糊和液态玻璃由合成器渲染后端处理。

[Shell 构建与使用](clarion-shell/README.md) · [设置中心](clarion-settings/README.md)

应用入口默认配置为独立的 Rust/GPUI **Tahoe Apps** 浮动面板，初始尺寸
980×680，使用固定类别、Suggested、名称排序及图标/列表视图。Shell 保留背景玻璃、
Dock 和焦点交接，通过 `$XDG_RUNTIME_DIR/clarion-launcher.sock` 连接独立进程。
GPUI 0.2.2 已 vendor；Phase 0 和完整生产入口的隔离原生验收已通过，覆盖真实中文输入、
Dock 桥接、全屏覆盖与焦点恢复、启动失败和旧入口回退。物理多屏、硬件手势和实际发行包构建
仍需验证；当前会话不会因源码或安装文件更新而自动切换。
实现、构建与分阶段验收见 [Tahoe Apps 说明](docs/launcher-apps.md)，
最初的隔离可行性证据见 [GPUI 启动器验证](docs/launcher-phase0.md)。

在 Shell 的 `preferences.json` 中设置 `"use_external_launcher": false` 可选择保留的
Flutter 启动器。新面板的排序模式、视图和尺寸单独保存到同一配置目录的 `apps-view.json`，
不会读取或改写旧的 `launchpad-layout.json` 文件夹布局。已有 `realand-shell` 配置继续按迁移规则读取。

## 构建与运行

在 Linux 上安装 Rust 工具链及所需的图形、输入和系统库后：

```sh
cargo build --locked --release -p clarion -p clarion-settings -p clarion-launcher
```

已有图形会话中可嵌套测试；在可用 TTY 上可启动独立物理会话：

```sh
./target/release/clarion --renderer vulkan
```

配置入口为 `~/.config/clarion/config.kdl`，可先检查配置：

```sh
./target/release/clarion validate
```

安装登录会话入口：

```sh
sudo bash scripts/install-session.sh
systemctl --user daemon-reload
```

保存工作后，在下次登录时选择 **Clarion**。安装入口不会自动重启当前桌面。

安装 Flutter Shell 时，把同一次构建的 Apps 二进制放入同一版本目录：

```sh
scripts/build-shell.sh --release
python3 scripts/install-shell.py \
  --bundle clarion-shell/build/linux/x64/release/bundle \
  --launcher-binary target/release/clarion-launcher
```

架构不同的机器请替换 bundle 的 `x64` 路径。安装器会先验证两个产物，保留旧版本与备份，
再原子更新入口；它不会重启 Shell 或启用服务。运行中的 Shell 始终优先使用自身版本目录中的
`clarion-launcher`，新安装版本不会混入旧进程。诊断时可用绝对可执行路径
`CLARION_LAUNCHER_BIN` 显式覆盖。

### 运行时依赖（字体与图标主题）

仓库不分发字体，界面只使用开源字体族：Shell 与系统偏好设置都优先通过 fontconfig 解析
Noto Sans / Noto Sans CJK SC，随后回退到 Source Han Sans、Sarasa、文泉驿、Inter、DejaVu、
Liberation 等开放许可字体。**系统若没有任何中文字体，中文会显示成方块**，请安装其一：

```sh
# Arch
sudo pacman -S noto-fonts-cjk ttf-dejavu
# Debian/Ubuntu
sudo apt install fonts-noto-cjk fonts-dejavu-core
```

SF、PingFang 与微软雅黑等专有字体不会被请求或加载；`scripts/import-shell-assets.sh` 只导入
图形素材，不再导入字体。

Dock 与启动器优先保留应用自身的图标；GPUI Apps 按 GTK 图标主题配置、主题继承、
`hicolor` 和 pixmaps 查找 PNG/SVG，Shell 的图标仍由 GTK 解析。因此
无图标主题的最小系统会显示通用占位图标：

```sh
sudo pacman -S hicolor-icon-theme adwaita-icon-theme   # 或 papirus-icon-theme
```

已有 Realand 配置、Shell 偏好和旧命令通过兼容入口继续使用，迁移规则见 [名称与配置迁移](docs/clarion-migration.md)。
依赖、嵌套运行和登录管理器说明见 [Getting Started](docs/wiki/Getting-Started.md)。
从合成器到完整桌面的能力现状与缺口见 [桌面就绪度清单](docs/desktop-readiness-roadmap.md)。

## 开发与验证

```sh
bash scripts/check-project.sh
bash scripts/check-vulkan.sh
clarion msg frame-timings
```

软件回归、GPU 验证和真实显示器的帧时间验收分别执行。硬件测试需要相应 render node 和验证层；单机测量不能代表所有显卡、显示器或应用负载。

## 许可证与第三方声明

Clarion 使用 [GPL-3.0-or-later](LICENSE)。依赖、第三方代码与适配部分保留各自的版权及许可证，见 [第三方声明](THIRD_PARTY_NOTICES.md)。
