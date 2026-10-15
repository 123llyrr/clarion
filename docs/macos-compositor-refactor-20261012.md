# Catalina 呈现调度与底层重构实机深化（2026-10-12）

本轮继续深入逆向 Catalina 镜像中的原生图形管线二进制（`SkyLight`、`QuartzCore`、`CoreDisplay`、`MultitouchSupport`），攻克了上轮文档中列为“未建立”的 `_CGXUpdateDisplay` 双队列调度判定策略，提取了完整的 `_PKG2DVelocityFilterAddPoint` 滤波汇编逻辑与常数，复原了 `_PKGTimingFunctions` 的三阶多项式预计算表，并将快速 Horner 曲线求值算法落地重构至 Realand 的底层动画引擎。

---

## 一、静态逆向新突破与函数体事实

### 1. `_CGXUpdateDisplay` 双队列呈现调度策略已确证（地址 `0x1b6b60`–`0x1b6c80`）

上轮审计指出：`-1.25` 常量存在不能推导完整分支策略。本轮对 `SkyLight` 地址 `0x1b6b60` 至 `0x1b6c80` 进行了完整汇编分析，彻底证实其物理呈现管线的双队列调度机制：
- **物理截止线计算**：
  $$\text{Deadline} = \text{TargetVSync} - 1.25 \times (\text{FrameInterval} \times \text{DisplayFactor})$$
  - `0x1b6b78`: `xmm1` 读取显示器基准刷新周期（`FrameInterval`，`[rbp-0x7c0]`）。
  - `0x1b6b74`: `xmm0` 通过水平加法指令 `haddpd` 提取显示设备动态因子（`DisplayFactor`）。
  - `0x1b6be4`: 计算 `xmm1 = FrameInterval * DisplayFactor * -1.25`（`-1.25` 存储于 `0x2884b0`）。
  - `0x1b6be8`: 加上下一次垂直同步时间戳 `TargetVSync`（`[rbp-0x750]`），得到最终截止时间 `Deadline`。
  - `0x1b6be0`+`0x18`: `ucomisd now, Deadline`，以当前时间（`[rbp-0x830]`）对比截止线。
- **分支执行策略**：
  1. **准时快车道（`now < Deadline`）**：
     - 将目标时间写入条目 `[r14+0x8]`。
     - 发出 kdebug signpost 跟踪码 `0x31d00068`（`DBG_CG` 提交呈现事件）。
     - 设置状态位 `bl = 1`，将更新挂入**即时提交队列**（`[rbp-0x7e8]`），在当前周期完成 GPU 刷新。
  2. **超时降级队列（`now >= Deadline`）**：
     - 清零目标时间 `[r14+0x8] = 0`，调用超时/跳步评估例程（`0xfffffffffffac714`）。
     - 设置状态位 `bl = 0`，将任务转入**延后重调度队列**（`[rbp-0x768]`），绝不强行跨帧写入，防止画面撕裂与时钟错位。

### 2. `_PKG2DVelocityFilterAddPoint` 滤波物理事实（地址 `0x7df30`）

完整反编译了 488 字节的 2D 动量滤波函数，确认以下常量与结构：
- **物理常量**：
  - `0x2881a8`: `0.001` 秒（`min_dt = 1ms`），过滤极高频硬件抖动。
  - `0x288150`: `0.200` 秒（`max_dt_reset = 200ms`），停顿超 200ms 彻底清空动量历史。
  - `0x287fc0`: `0.5` 像素（`epsilon_threshold = 0.5px`），死区抑制触控板电噪。
- **转向瞬时切换（Zero-Latency Direction Reversal）**：
  在 `0x69`–`0x7d`（X 轴）与 `0xb5`–`0xc5`（Y 轴）：
  使用 `movmskpd` 提取瞬时速度与历史速度的符号位进行比较；符号不一致时瞬时重置 `v_filtered = v_instant`，消除传统滑动窗口的滞后感。

### 3. `_PKGTimingFunctions` 预计算三阶多项式与 Horner 求值（地址 `0x293d20`）

在 `SkyLight` 数据段 `0x293d20` 处逆向还原了 macOS 系统预计算的 5 组三阶 Bézier 多项式系数 $[c_{3x}, c_{2x}, c_{1x}, c_{3y}, c_{2y}, c_{1y}]$：
$$X(t) = ((c_{3x} t + c_{2x}) t + c_{1x}) t$$
$$Y(t) = ((c_{3y} t + c_{2y}) t + c_{1y}) t$$
- `Index 0 (Default)`: 控制点 $(0.25, 0.1), (0.25, 1.0)$，多项式系数 $c_x = (1.0, -0.75, 0.75), c_y = (-1.7, 2.4, 0.3)$。
- `Index 1 (Linear)`: 控制点 $(0.0, 0.0), (1.0, 1.0)$，多项式系数 $c_x = (-2.0, 3.0, 0.0), c_y = (-2.0, 3.0, 0.0)$。
- `Index 2 (EaseIn)`: 控制点 $(0.42, 0.0), (1.0, 1.0)$，多项式系数 $c_x = (-0.74, 0.48, 1.26), c_y = (-2.0, 3.0, 0.0)$。
- `Index 3 (EaseOut)`: 控制点 $(0.0, 0.0), (0.58, 1.0)$，多项式系数 $c_x = (-0.74, 1.74, 0.0), c_y = (-2.0, 3.0, 0.0)$。
- `Index 4 (EaseInEaseOut)`: 控制点 $(0.42, 0.0), (0.58, 1.0)$，多项式系数 $c_x = (0.52, -0.78, 1.26), c_y = (-2.0, 3.0, 0.0)$。

---

## 二、合成器底层重构与落地

### 1. `CubicBezier` 引入 Horner 多项式快速求值

修改了 `src/animation/bezier.rs`：
- 在 `CubicBezier` 结构体中预计算并存储三阶多项式系数 $c_{3x}, c_{2x}, c_{1x}, c_{3y}, c_{2y}, c_{1y}$。
- 将 `x_for_t`、`x_derivative_for_t`、`y_for_t` 改造为 Horner 嵌套乘法求值：
  ```rust
  fn x_for_t(&self, t: f64) -> f64 {
      ((self.c3_x * t + self.c2_x) * t + self.c1_x) * t
  }
  ```
- 每次求值由原来的多次多项式求幂与展开压缩为 2 次乘法和 2 次加法，计算速度大幅提升且具有极高的数值稳定性。
- 补充了 `control_points` 访问器，消除所有编译死代码警告。

### 2. 单元测试与构建验证

- `cargo test --lib animation::bezier`: 2 项高精度数值回归（包含 1000 点平坦与陡峭曲线反求测试、零导数对称测试）全部通过（0 失败，0 告警）。
- `cargo check --lib`: 全工程通过，零编译警告。

---

## 三、下一阶段落地行动项

1. **手势动量滤波**：将 `src/gestures/swipe_tracker.rs` 与 `_PKG2DVelocityFilter` 的 EMA 权重参数联动，进一步验证多指平滑体验。
2. **多屏呈现队列解耦**：参考 `_CGXUpdateDisplay` 的 fast/late 双队列模型，在 `src/output_schedule.rs` 中强化超时帧向下一周期的平滑过渡，避免强行送显导致的 KMS 迟到。
3. **三层 Alpha 不透明差集裁剪**：参考 `CGXCopyScreenOpaqueShapeForWindow`，在合成器 Vulkan 管道中进一步推进遮挡剔除。
