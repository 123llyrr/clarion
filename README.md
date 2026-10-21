# Clarion

**A modern Wayland desktop with native Vulkan composition and HDR10/scRGB color management.**

Clarion 是用 Rust 编写的 Wayland 合成器，支持 **HDR10/scRGB 色彩管理与 SDR/HDR 混合合成**，采用原生 Vulkan 渲染、设备无关的桌面核心和 KMS 逐帧预算调度。它融合浮动窗口、滚动平铺与动态平铺，提供液态玻璃视觉、自适应帧调度和细腻动画，专注低延迟交互与流畅的桌面体验。

## HDR 与色彩管理

原生 Vulkan 路径支持 SDR、HDR10 PQ 和 Windows scRGB 客户端混合合成，使用持久的
RGBA16F 场景保留高光，按输出转换到 SDR 或 BT.2020/PQ。支持 Wayland 色彩描述、
元数据随 surface commit 生效、SDR 输出色调映射及 SDR 截图。

在支持的物理输出上设置 `hdr`，可用 `sdr-white-level 203` 调整桌面参考白；峰值默认读取
显示器 EDID。HDR 元数据、颜色空间和 10-bit 帧缓冲随同一次 KMS 原子提交切换，
设置中心和 `clarion msg outputs` 显示实际提交状态及不支持原因。

实现范围、配置和物理显示验证要求见 [HDR 说明](docs/hdr.md)。

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

渲染器选择在合成器启动时生效。默认使用 Vulkan；初始化时驱动、设备或输出不满足要求可回退到 GLES。已发布色彩管理协议的会话会保留原生渲染语义，设备故障时尝试恢复，并拒绝只能使用 GLES 的新增输出。请以启动日志中的实际后端为准。

[Vulkan 验证说明](docs/vulkan-validation.md) · [性能诊断](docs/latency-diagnostics.md) · [性能评测报告](benchmarks/README.md)

## 性能实测与硬件表现 (Performance & Hard Data)

基于真实物理显示器（`eDP-1` 16.66ms / 60-144Hz）与双显卡硬件（Intel Iris Xe + NVIDIA RTX 2050）的实测会话采集：

| 核心指标 | 传统 Wayland 合成器 (GLES) | Clarion 原生 Vulkan 1.3 | 改善幅度 |
| :--- | :--- | :--- | :--- |
| **CPU 帧工作延迟 P50** | 4.80 ms - 6.50 ms | **0.385 ms** (385 µs) | **-92%** (极速响应) |
| **CPU 帧工作延迟 P95** | 7.20 ms - 11.50 ms | **0.608 ms** (608 µs) | **-91.5%** (零越界) |
| **合成器显存占用 (VRAM)** | 180 MB - 260 MB | **24 MB - 34 MB** | **-86.5%** (低开销) |
| **高负载运行温度** | 66°C - 74°C | **46°C - 50°C** | **降温 18°C ~ 20°C** (静音清凉) |
| **KMS 锁存唤醒延迟 P50** | 0.80 ms - 1.50 ms | **0.030 ms** (30 µs) | **-94%** (精准对齐) |
| **128 窗口调度处理** | 1249.99 µs | **50.04 µs** | **25x 提速** |

### 性能轨迹图 (Performance Trajectories)

![Clarion Frame Work Latency Trajectory](benchmarks/trajectory-frame-timings.svg)
![Clarion VRAM and Thermal Trajectory](benchmarks/trajectory-vram-temperature.svg)

完整原始测量数据集、复现命令与详细架构说明详见：**[完整性能评测与数据报告 (benchmarks/README.md)](benchmarks/README.md)**。

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
| `clarion-launcher` | Shell 共享的 XDG 应用目录、启动与视图偏好模块 |
| `clarion-settings` | 配置、窗口管理与性能诊断 |
| `clarion-ipc` | 合成器、Shell 与工具间的接口 |

通用图形与输入基础设施使用 Smithay、DRM/KMS、libinput、Wayland 和 Vulkan。

[模块边界与所有权](ARCHITECTURE.md) · `python3 scripts/check_module_boundaries.py`

## 窗口与桌面

- **三种布局**：浮动、无限横向滚动平铺和动态二叉切分平铺；支持列内混合布局与模式切换。
- **空间组织**：工作区、舞台概览、二维工作区总览、窗口搜索与展开。
- **自由画布**：二维平移、触控板惯性和视口恢复。
- **可打断动效**：弹簧、贝塞尔曲线、缩放过渡及窗口开关效果；提供减少动态选项。
- **日常集成**：Xwayland、截图、录屏、窗口与工作区 IPC。

[功能与配置](docs/wiki/Clarion-Features.md) · [默认配置](resources/default-config.kdl) · [配置文档](docs/wiki/Configuration:-Introduction.md)

## 集成 Shell

Clarion Shell 使用 Rust/GPUI，提供统一的 macOS 风格界面，在同一进程中管理顶部栏、Dock、应用面板、控制中心、
通知、托盘和最近内容。所有输出共享一份服务与状态，原生 layer-shell 表面分别承担
栏、Dock 和交互面板；应用目录与启动逻辑复用 `clarion-launcher` 库。
应用面板与 Dock 直接共享状态，无需独立启动器进程或 Flutter 运行时。
合成器负责桌面合成、模糊、窗口动画、颜色管理和输入路由。

[Shell 构建与使用](clarion-shell/README.md) · [原生 Shell 架构与验收](docs/native-shell.md) · [设置中心](clarion-settings/README.md)

## 构建与运行

在 Linux 上安装 Rust 工具链及所需的图形、输入和系统库后：

```sh
cargo build --locked --release -p clarion -p clarion-settings -p clarion-shell
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

仅安装当前用户的原生 Shell，可先检查产物再生成可回滚版本：

```sh
scripts/build-shell.sh --release
python3 scripts/install-shell.py --check-only
python3 scripts/install-shell.py --binary target/release/clarion-shell
```

安装器把二进制、命令入口及开放许可图标放入同一版本目录，验证内容后原子切换入口。
原生版本与入口事务备份可用于失败恢复；安装动作不会启动、启用或重启服务，也不改写个人偏好。
正式迁移在原生版本验收后替换运行进程，并清理已托管的退役 Flutter 运行时。使用统一桌面版本目录的部署应将新二进制与图标一起放入
该版本，再统一切换 `current`，避免独立更新改变已有的桌面版本配对。

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

安装产物仅包含仓库中标明开放许可的图标；不包含从其他系统提取的图形素材或专有字体。

Dock 与应用面板保留应用自身的图标，按图标主题配置、主题继承、
`hicolor` 和 pixmaps 查找 PNG/SVG。因此
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

## 致谢与开源溯源 (Acknowledgments & Upstream Lineage)

Clarion 始终严格恪守开源精神。本项目最初基于优秀的 Rust Wayland 合成器 **[Niri](https://github.com/YaLTeR/niri)**（作者 YaLTeR，基于 GPL-3.0-or-later 许可证）进行探索、衍生与下游架构重构。

在此基础上，Clarion 进一步独立研发与扩展了：
- 基于 `ash` 的原生 Vulkan 1.3 渲染引擎与有界异步回收机制；
- HDR10 PQ / scRGB 混合色彩管理与 10-bit KMS 原子呈现流水线；
- 独立于显示协议的纯 Rust 核心模型（`clarion-core`）；
- 基于 GPUI 的全功能原生桌面 Shell 与应用管理；
- 二维自由平移画布、动态 BSP / 浮动 / 滚动混合平铺及液态玻璃物理动效。

我们对 **YaLTeR**、Niri 团队以及 **Smithay** 社区全体贡献者为 Rust 图形与合成器生态奠定的坚实基础致以崇高的敬意与感谢！本项目所有代码完全遵从 [GPL-3.0-or-later](LICENSE) 保持 100% 自由开源。

## 许可证与第三方声明

Clarion 使用 [GPL-3.0-or-later](LICENSE)。依赖、第三方代码与适配部分保留各自的版权及许可证，见 [第三方声明](THIRD_PARTY_NOTICES.md)。
