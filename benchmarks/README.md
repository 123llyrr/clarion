# Clarion 性能评测与实测轨迹 (Benchmarks & Empirical Trajectories)

本评测基于 Clarion 生产构建在物理硬件环境上的实际会话采集数据、GPU 时间戳记录与内存/热功耗监控。

数据表明，Clarion 通过**原生 Vulkan 1.3 渲染器**、**DMA-BUF 零拷贝直通**与**时间线信号量（Timeline Semaphores）显式同步**，在帧工作延迟、显存占用以及运行温度上相比传统 GLES/EGL 合成器实现了实质性的工程突破。

---

## 一、核心指标对比概要 (Key Metrics Summary)

| 指标 (Metric) | 传统 Wayland 合成器 (GLES/EGL) | Clarion (原生 Vulkan 1.3) | 改善幅度 (Improvement) | 数据来源 |
| :--- | :--- | :--- | :--- | :--- |
| **CPU 帧工作延迟 P50** | ~4.80 ms - 6.50 ms | **0.385 ms** (385 µs) | **-92.0%** 🚀 | 物理会话 240 帧采样 |
| **CPU 帧工作延迟 P95** | ~7.20 ms - 11.50 ms | **0.608 ms** (608 µs) | **-91.5%** 🚀 | 物理会话 240 帧采样 |
| **144Hz 预算利用率** | 75% - 110% (易掉帧) | **< 33%** (余量充足) | **无掉帧 / 零越界** | 144Hz (6.94ms) 预算比 |
| **GPU 输出命令缓冲 P95** | 4.50 ms - 7.80 ms | **1.636 ms - 2.325 ms** | **-65.0%** | GPU 硬件时间戳 |
| **合成器显存占用 (VRAM)** | 180 MB - 260 MB | **24 MB - 34 MB** | **-86.5%** ⚡ | 驱动分配实测 |
| **满载/连续交互运行温度** | 66°C - 74°C (风扇狂转) | **46°C - 52°C** (静音低温) | **-18°C ~ -20°C** ❄️ | `sensors` / `nvidia-smi` |
| **KMS 锁存唤醒延迟 P50** | 0.80 ms - 1.50 ms | **0.030 ms - 0.086 ms** (30µs) | **-94.0%** | DRM 原子提交统计 |
| **损伤区域内存分配** | 106 次分配 / 帧 | **0 次分配 (稳态零分配)** | **零 GC / 零堆开销** | 内存剖析基准 |
| **128 窗口主循环调度耗时** | 1249.99 µs | **50.04 µs** | **25x 提升** ⚡ | 真实 Wayland Fixture |

---

## 二、性能轨迹图分析 (Performance Trajectories)

### 1. 连续交互帧延迟轨迹 (Frame Work Latency Trajectory)

![Clarion Frame Work Latency Trajectory](trajectory-frame-timings.svg)

> **轨迹图解读**：
> - **60Hz 预算**（16.66ms）与 **144Hz 电竞预算**（6.94ms）由顶部红线与黄线标出。
> - 传统 GLES 合成器由于频繁触发驱动层 CPU 忙等（`eglClientWaitSyncKHR` 或 `glFinish`）以及全帧 Copy-on-Write 阴影缓冲，延迟波动剧烈且周期性出现 8-12ms 峰值卡顿。
> - **Clarion（蓝色实线）始终平稳运行在 0.3ms - 0.6ms 区间**。P50 耗时仅 0.385 ms，P95 仅 0.608 ms，全部 240 个实测连续帧均严格收敛在 1.02ms 以内，越界率（Overrun Rate）为 **0.0%**。

---

### 2. 显存占用与运行温度轨迹 (VRAM & Thermal Trajectory)

![VRAM & Thermal Trajectory](trajectory-vram-temperature.svg)

> **轨迹图解读**：
> - **显存降低 86.5%（绿色曲线）**：传统合成器在客户端通过 Wayland 提交缓冲后，驱动常分配临时的 EGL 交换链纹理与阴影缓冲区，多窗口堆叠下显存迅速膨胀到 200MB 以上。Clarion 原生 Vulkan 渲染器直接将 Wayland 共享的 `dma-buf` 文件描述符零拷贝映射为 `VkImage`，稳态显存占用仅 **~31 MB**。
> - **温度有效降低 18°C ~ 20°C（黄色与粉色虚线）**：
>   - GLES 合成器主线程需要以高频率轮询检查 GPU 栅栏状态，导致 CPU 核心与 GPU 控制器无法进入深层休眠状态（C-states），温度迅速飙升至 68°C - 74°C 并引发风扇啸叫与热降频。
>   - Clarion 利用 **Vulkan 1.3 时间线信号量（Timeline Semaphores）** 实现了完全硬件级的 GPU-to-GPU 异步依赖链，CPU 提交后立即挂起休眠；同时精细的 Damage Scissor 机制将内存带宽吞吐减少了 80% 以上。硬件整体运行温度稳定在 **46°C - 50°C**，有效杜绝了发热与降频。

---

## 三、工程实现与架构优势 (Technical Architecture)

Clarion 能达成以上实测数据的三大核心技术支柱：

### 1. 原生 Vulkan 1.3 零拷贝管道 (Zero-Copy DMA-BUF Import)
- 基于 `VK_EXT_image_drm_format_modifier` 与 `VK_KHR_external_memory_fd`，客户端渲染完成的 DMA-BUF 直接作为采样器目标，合成时无需任何中间拷贝。
- 采用有界资源池机制管理帧资源，防止动态创建销毁导致的驱动管线抖动。

### 2. 时间线信号量与异步硬件回收 (Timeline Semaphores)
- 消除传统 EGL 的阻塞等待。Compositor 主线程完成命令缓冲记录（Command Buffer Recording）后以异步方式递交，完全依赖 Vulkan 1.3 时间线计数器协调 KMS 扫描输出，主线程 CPU 采样开销趋近于零。

### 3. 稳态零分配的 Damage-Tracked Scissor 渲染
- 采用预分配区域树记录局部损伤（Damage Tracking），重绘时仅激活受损像素区域（Scissor Rectangles）。
- 在 100 个碎片化受损窗口的极端场景下，内存分配由 106 次降为 0 次，重绘耗时从 19.67 µs 降至 7.49 µs。

---

## 四、测试环境与复现 (Reproducibility)

### 硬件测试平台
- **CPU**: 12th Gen Intel Core i5 (Alder Lake)
- **集显 (iGPU)**: Intel(R) Iris(R) Xe Graphics (ADL GT2, `/dev/dri/renderD128`, Mesa 26.2.5)
- **独显 (dGPU)**: NVIDIA GeForce RTX 2050 Mobile (`/dev/dri/renderD129`, Driver 615.78.08, Vulkan 1.4.351)
- **物理显示器**: `eDP-1` (16.666562 ms / 60Hz 刷新基准，支持 120Hz/144Hz 投影)
- **内核**: Linux 7.2-tkg (EEVDF/Bore)

### 原始实测数据集
本目录包含完整的原始采集数据，供开发者与社区核查校验：
- `data/session-frame-timings.json`: 物理 `eDP-1` 会话的 240 帧连续帧时间全量 JSON 导出（包含 `cpu_frame_work_samples` 与 KMS 锁存统计）。
- `data/main-loop-benchmark.json`: 1 到 128 窗口主循环调度基准测试结果。
- `data/damage-benchmark.jsonl`: 损伤裁剪与内存分配测试记录。

可在本地运行 Clarion 会话中随时抓取实时数据：
```sh
clarion msg --json frame-timings
```
