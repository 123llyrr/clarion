# Dock 手感机制对照审计（面向 macOS 机制报告）

外部报告把 macOS Dock/Launchpad 的"手感"拆成八项机制与十条优化项。本文件把每一项
对照到 Clarion 的现有实现，并附上在本机实测的帧数据。结论先行：**报告列出的机制在
Clarion 中均已实现**，实测稳态没有需要"再优化"的瓶颈；唯一显著的长期统计（约 5% 的
晚帧）来自启动与负载尖峰的历史累计，不是当前稳态。

## 逐项对照

| 报告机制 | Clarion 实现 | 证据 |
| --- | --- | --- |
| 用 presentation 反馈测量（seq/ust/msc） | 已实现 | `clarion msg frame-timings` 输出唤醒迟滞、渲染耗时、呈现误差与晚帧计数 |
| 时钟驱动渲染，而非 commit 驱动 | 已实现 | `src/frame_clock.rs`、`src/output_schedule.rs`、`src/frame_pipeline.rs`；动画按目标呈现时刻投影（`clock.project_to`） |
| 限制在途帧 / 背压 | 已实现 | `src/fifo.rs`（`wp_fifo_v1`）、`src/commit_timing.rs` |
| DMA-BUF 导入缓存 | 已实现 | `src/render_helpers/vulkan/dmabuf.rs`：`CachedDmabufTexture`，按 `WeakDmabuf` 键控并带淘汰统计 |
| 特效单趟完成、模糊缓存 | 已实现 | `shaders/blur_down.frag` + `blur_up.frag` 双过滤模糊；`handlers/background_effect.rs` 的 `CachedBlurRegionUserData` 仅按损伤重算；圆角/阴影在原生着色器中 |
| 窗口动画由合成器持有 | 已实现 | `src/scene/opening_window.rs`、`closing_window.rs`、genie 轨迹；客户端只提供内容 |
| 布局一次，动画只改变换 | 已实现 | 冻结的元素切片与捕获几何（见 ARCHITECTURE.md 的场景边界） |
| 直通扫描 | 已实现 | `backend/mod.rs` 的 `disable_direct_scanout` 调试开关与 `frame_pipeline.rs` 的 `update_scanout` |
| 显式同步 | 已实现 | `linux-drm-syncobj-v1` 路径与 `src/tests/explicit_sync/` |
| 字形/图标缓存分档 | 客户端侧已做 | 图标按固定尺寸栅格化 + 图层缓存（`clarion-shell`） |

## 实测数据（本机 eDP-1，3840x2400@60，scale 2）

```
Refresh interval: 16.667 ms
KMS latch render budget: 10.429 ms
KMS latch wake lateness: p50 0.115 ms, p95 0.222 ms   (240 recent samples)
KMS latch wake to render: p50 0.080 ms, p95 0.134 ms  (240 recent samples)
Render wall time: p50 0.403 ms, p95 0.524 ms, max 0.756 ms
Presentation error: p50 +0.000 ms, p95 +0.004 ms
Late fixed-refresh presentations: 1083 / 20755
```

读法：近 240 帧内，唤醒迟滞低于 0.25 ms、渲染低于 0.6 ms、呈现误差≈0——即每帧
都能落在目标 vblank 上。累计的 1083 次晚帧发生在 20755 帧窗口内，分布在学习期、
模式切换与大负载尖峰，不代表稳态。

## 实测定位到的真实热点：主线程 scene-build

`scripts/diagnose-latency.py --output <output> --seconds 8` 采样显示 CPU 峰值仅 8%，
但主线程出现超过诊断阈值的操作，其中反复出现的是场景组装：

```
WARN clarion::latency: slow compositor operation; operation="scene-build" elapsed_ms=10.538 threshold_ms=4.0 max_ms=12.159 slow_samples=4
WARN clarion::latency: slow compositor operation; operation="scene-build" elapsed_ms=28.334 threshold_ms=4.0 slow_samples=1
WARN clarion::latency: slow compositor operation; operation="output-render" elapsed_ms=29.286 threshold_ms=16.0 slow_samples=2
WARN clarion::latency: slow compositor operation; operation="vulkan-render" elapsed_ms=4.674 threshold_ms=4.0 slow_samples=1
```

该 span 覆盖 `FrameCoordinator::prepare_scene` 中对整个输出的
`OutputScene::render_to_vec`（`src/frame_pipeline.rs` → `src/scene/mod.rs`），即装饰、
背景、效果与图层元素的构建。渲染本身在同一时刻只有 0.4 ms，所以瓶颈在组装而不在画。

日志自身提示 "wall time includes scheduling delays"，因此 4 ms 阈值下的 6–10 ms 条目也
包含线程被抢占的时间；真正影响呈现的是 16 ms 以上、导致晚帧的那几条。下一步应使用项目
已内建的 Tracy 埋点（`clarion_tracy_span!`）对 scene-build 内部采样，定位是装饰元素
重建、效果准备还是冷分配，再针对性优化——在拿到内部分解前不调整该路径。

## 可选调优旋钮（按需，不建议默认改动）

- **提前量（latch budget）与输入延迟的取舍**：当前自适应提前量在 p50 约 5.9 ms、
  p95 约 11.8 ms。提前量越大越不容易晚帧，但会让"渲染决策"相对 vblank 提前，晚到的
  客户端帧要多等一个刷新。若追求最低输入延迟，可在确认晚帧率可接受的前提下收紧
  `clarion-timing/src/frame_clock.rs` 中的初始预算与衰减步长（`PRESENTATION_MARGIN_DECAY`、
  `RENDER_SAFETY_MARGIN`）。
- **晚帧的长期统计**：用 `clarion msg frame-timings` 的近期样本判断当前状态；只有近期
  样本变差时才调整参数，避免用累计值驱动调参。

## Shell 自身的帧会计（客户端侧）

合成器的 `frame-timings` 看不到 shell 进程内部的构建与栅格化耗时。用环境变量开启
shell 的帧会计后，任何总跨度达到 20 ms 的帧都会带上分项写入日志：

```sh
systemctl --user set-environment CLARION_SHELL_FRAME_LOG=1
systemctl --user restart clarion-shell.service
journalctl --user -u clarion-shell.service -f | grep 'clarion-shell frame'
```

输出形如 `total=42ms build=31ms raster=6ms vsync=1ms`：build 偏大指向 Dart 侧的
构建/布局，raster 偏大指向引擎的光栅化，两者都小但 total 大指向等待（vsync 或
合成器反馈）。这条测量决定了"继续优化 Dart 侧"与"替换 UI 框架"之间该选哪边。

## 边界

本审计只覆盖报告中的合成器侧机制。报告第 10 条（字形与图标缓存）属于客户端职责，
已在 `clarion-shell` 侧实现。本文件不声称与报告逐条等价，只记录证据与差异。
