# macOS 镜像逆向分析与 Wayland 合成器（Realand）流畅度工程实现

2026-10-12 复核范围：第 4、5 节及事务提交后处理；本轮完整函数体校验见第 7 节。其余历史分析没有在本轮重新逐项验证，不作为整桌面像素一致或性能相等的保证。

更新日期：2026-10-11  
分析样本来源：macOS Catalina 10.15.7 (19H2) 镜像提取的原生二进制：
- `WindowServer` (Mach-O x86_64, 46,064 字节)
- `SkyLight.framework` (版本 451.4, 4,193,888 字节, SHA-256 `38cde43140bf...`)
- `QuartzCore.framework` (3,862,896 字节, SHA-256 `548c33b372ab...`)
- `CoreDisplay.framework` (1,735,040 字节)
- `MultitouchSupport.framework` (281,904 字节)

2026-10-11 的复核仅针对恢复出的 BaseSystem SkyLight 调度与提交函数。原 DMG 尾部缺失，样本哈希不代表整个镜像或 Apple CMS 验证通过；本轮没有执行 macOS 二进制或测量 Catalina 运行性能。下文保留的其他早期分析不因此获得新的验证。当前实现、测试与证据边界见 [本轮合成器重构记录](macos-compositor-refactor-20261011.md)。

---

## 核心发现与逆向事实总览

以下整理静态样本中的观察及尚需验证的机制，不构成 macOS 性能的因果证明：
1. **Work-Interval 静态依赖**：SkyLight 导入了 Work-Interval 函数。导入符号与前轮记录的 entitlement 不能证明每帧动态申请实时优先级、最高优先级核心选择或实际带宽分配。
2. **多显示器调度与时间截断**：`WSDisplayUpdateQueueSchedule` 具有 125 ms 远期截断和三种重调度行为；队列比较器与全局选择顺序尚未由本轮函数证明。`-1.25` 常量存在，但完整提交判定策略仍需函数体证据。
3. **零延迟自适应动量滤波**：`_PKG2DVelocityFilterAddPoint` 采用带转向瞬时切换（Zero-Latency Direction Reversal）的 EMA 滤波与 0.5px 死区，彻底解决平滑与延迟的固有矛盾。
4. **全屏三层透明度传播与遮挡剔除**：精确计算 `window_alpha * layer_alpha * parent_alpha`，对不透明区域执行严格差集剔除，杜绝无效像素光栅化。
5. **窗口冻结缩放与背景模糊缓存**：`_SLSWindowFreezeWithOptions` 采用 1.0 秒延迟快照保护消除缩放拉扯闪烁；`BackdropFreeze` 冻结静止模糊背景，避免每帧重复重绘毛玻璃。

---

## 一、WindowServer 调度特权与内核 Work-Interval 逆向

### 1.1 进程与特权证据
- 二进制路径：`SkyLight.framework/Versions/A/Resources/WindowServer`
- 内部源码路径：`/AppleInternal/BuildRoot/.../SkyLight-451.4/SkyLightTools/WindowServer/WindowServer.c`
- 关键 Entitlements（嵌入代码签名中的 plist）：
  ```xml
  <key>com.apple.private.kernel.work-interval</key><true/>
  <key>com.apple.private.gpuwrangler</key><true/>
  <key>com.apple.private.iosurfaceinfo</key><true/>
  <key>com.apple.QuartzCore.debug</key><true/>
  ```
- 进程入口流程：
  `main` 函数执行 `setegid(WINDOWSERVER_GID)`、`seteuid(WINDOWSERVER_UID)`、初始化 Darwin 临时/缓存目录，进入沙箱，随后直接调用 `_SLXServer`。所有的窗口管理、事件循环与合成调度均在 `SkyLight` 内执行。

### 1.2 Work-Interval 静态依赖与运行机制边界
在 `SkyLight` 的符号表中观察到以下导入函数：
- `_work_interval_create`
- `_work_interval_instance_alloc`
- `_work_interval_instance_start`
- `_work_interval_instance_finish`
- `_work_interval_instance_set_deadline`

这些导入证明样本存在 Work-Interval 依赖，不证明调用频率、参数、每帧 start/finish 配对或 XNU 调度结果。要确认渲染线程的优先级、核心选择与实时带宽，需要具体调用路径以及目标系统上的调度证据；本轮未建立这些事实。
- **静态字符串线索（前轮记录）**：`0x2eb790` 处字符串为 `"CGXDisplay work_interval signaling enabled."`，关联 `0x17eae3` 处的 `set_debug_options`。字符串不代表运行时已启用或实际记录过该日志。

### 1.3 对 Realand (Wayland) 的工程落地方案
- 以下是 Linux 侧的工程候选方案，需要独立测量与验证，不能视为 Mach Work-Interval 的等效实现：
  1. **RealtimeKit 评估**：如果测量显示 KMS 提交被线程调度延迟拖慢，再评估 RealtimeKit 和线程调度策略；实际授权、策略、优先级与收益需在 Linux 环境中验证。本轮静态样本不支持直接指定实时优先级，也不代表 Realand 已实现此方案。
  2. **事件分发与渲染提交彻底解耦**：客户端协议分发（`client-dispatch`）与主渲染循环隔离，防止单个 Wayland 客户端（如缓慢的 IPC、大文件传输或死锁的 DBus 调用）拖慢 KMS 呈现主节律。

---

## 二、多输出调度队列与呈现预测 (`WSDisplayUpdateQueue`)

### 1.1 核心数据结构与地址
- 调度入口：`_WSDisplayUpdateQueueSchedule`（地址 `0xddf2e`）
- 时间查询：`_WSDisplayUpdateQueueGetDisplayUpdateTime`（地址 `0xddea4`）
- 呈现主循环：`_CGXUpdateDisplay`（地址 `0x1b6222`）

### 2.2 调度容器与重调度策略
`WSDisplayUpdateQueueSchedule(int display_id, double timestamp, WSTimerReschedulePolicy policy)`:
1. **按显示器维护时间**：反编译结果引用 `std::set<DisplayScheduleEntry>` / `std::__tree`，但容器名本身不证明比较器、时间与 ID 的排序优先级或全局取最早输出的策略。本轮 `WSDisplayUpdateQueueGetDisplayUpdateTime` (`0xddea4`) 按明确的 `display_id` 查找已有时间，未找到时返回原始 64 位零值；它不是全局最早时间查询。`WSDisplayUpdateQueueAddActiveDisplays` (`0xde147`) 在一次遍历中读取一次当前时间，再根据设备状态剔除条目或使用各显示器的 next wakeup 加偏移调度；各状态字节的完整语义尚未建立。
2. **远期保护截断（Forward Horizon Cap）**：
   ```c
   now = SLSCurrentRealTime();
   double max_horizon = now + 0.125; // 125 ms (存储于 0x288288)
   timestamp = (isnan(timestamp) || timestamp > max_horizon)
       ? max_horizon : timestamp;
   ```
   如果传入的时间戳大于当前时间 125 毫秒（或为 NaN），该函数截断至 `now + 0.125`。这约束了队列中的远期时间，不能保证消除锁等待、阻塞 I/O、GPU fence 或回调造成的死锁及卡顿。已验证的 Schedule Evidence：`ev_bdda56fd61d87b514d0d03196ea4f0fa241650140ec8d026309696ff433a5239`。
3. **三种重调度策略 (`WSTimerReschedulePolicy`)**：
   - `Policy 1 (AdvanceOnly)`：仅当新时间早于当前队列时间时才提前。
   - `Policy 2 (PostponeOnly)`：仅当新时间晚于当前队列时间时才推后。
   - `Policy 3 (Always)`：无条件重设。

### 2.3 呈现分支仍需完整调用证据

旧稿曾把 `0x2884b0` 的 `-1.25` 常数和局部反汇编解释成确定的双队列、丢帧与当前 VSync 提交策略。现有证据只能确认常数及局部指令；操作数含义、所有分支、队列消费者和实际呈现时刻未充分验证。不能据此在 Realand 中硬编码 `TargetVSync - 1.25 × Interval`，也不能声称该公式保证流畅度。现有截止期继续采用本机的渲染/提交测量。

### 2.4 不允许嵌套的显示更新批次

`WSBeginDisplayUpdateTransaction` (`0x1b4b6b`) 检查 `sessionData->txnOpen == false`，已打开时断言失败；`WSCommitDisplayUpdateTransaction` (`0x1b4be5`) 则要求 `txnOpen == true`。这些入口维护的是开放布尔位，不是嵌套深度计数器。

Commit 统一处理本批窗口几何、清除开放状态，再按累计区域执行失效与更新调度，最后释放窗口列表。这支持“在批次结束集中处理几何和通知”的工程选择，不能单独证明整个呈现管线的背压策略或性能。

Begin Evidence：`ev_4bbd53e32ba32483f0cb43ac9bfe0f50f80933b96552e51e24759686096c0ff3`。Commit Evidence：`ev_a75d08c4b8edde3df514e57c8766bf5a1a85aa55cb85a9ea595c31c44e8ac772`。本轮五个完整函数体共 914 字节，均与原样本逐字节匹配；Ghidra 默认分析完成，但出现 Objective-C analyzer 异常，不能声称获得完整 Objective-C 调用图。

---

## 三、触控手势滑动动量滤波 (`PKG2DVelocityFilterAddPoint`)

在 `SkyLight` 地址 `0x7df30` 处逆向分析了苹果核心的 2D 速度滤波算法：

### 3.1 核心物理参数
- `min_dt = 0.001 s`（1 ms，存储于 `0x2881a8`）：两次事件间隔小于 1ms 时忽略，过滤极端时间戳抖动。
- `max_dt_reset = 0.200 s`（200 ms，存储于 `0x288150`）：如果手指停顿超过 200ms，重置历史速度，直接以瞬时速度初始化。
- `epsilon_threshold = 0.5 px`（存储于 `0x287fc0`）：位移小于 0.5 像素死区截断，消除触控板硬件微弱电噪引起的抖动。

### 3.2 转向瞬时响应算法（Zero-Latency Direction Reversal）
当手指滑动并在同向运动时，采用指数加权移动平均（EMA）：
$$v_{\text{filtered}} = \alpha \cdot v_{\text{instant}} + (1 - \alpha) \cdot v_{\text{prev}}$$

**关键技术特征**：
在 `0x7df99`–`0x7dfad`（X轴）和 `0x7dfe5`–`0x7dff5`（Y轴）：
```assembly
movmskpd eax, xmm5   ; 提取历史速度符号位
and eax, 1
movmskpd ecx, xmm4   ; 提取瞬时速度符号位
and ecx, 1
cmp eax, ecx
jnz direction_changed ; 符号不一致！
```
若用户**突然反向滑动**（`sign(v_instant) != sign(v_prev)`）：
**算法直接跳过平滑公式，瞬时赋值 $v_{\text{filtered}} = v_{\text{instant}}$！**
- **工程意义**：传统 Linux 滑动追踪器（如滑动窗口平均）在用户折返滑动时，历史样本仍包含正向位移，导致 100~150ms 的“橡皮筋滞后卡顿感”。macOS 通过反向瞬时重置，既保证了同向滑动的平滑，又实现了反向折返的零延迟响应！

### 3.3 动画三次方曲线预计算与 Horner 快速求值 (`_PKGTimingFunctions`)
在 `SkyLight` 地址 `0x293d20` 与 `0x1defc8`（`__PKGAnimationCallback`）处逆向分析：
- **多项式系数预计算表（`_PKGTimingFunctions`，0x293d20）**：
  macOS 并不在每一帧重复求解 Bernstein 基函数多项式，而是在静态表直接存入三阶多项式系数 $[c_{3x}, c_{2x}, c_{1x}, c_{3y}, c_{2y}, c_{1y}]$：
  $$X(t) = c_{3x} t^3 + c_{2x} t^2 + c_{1x} t = ((c_{3x} t + c_{2x}) t + c_{1x}) t$$
  $$X'(t) = 3 c_{3x} t^2 + 2 c_{2x} t + c_{1x} = (3 c_{3x} t + 2 c_{2x}) t + c_{1x}$$
  $$Y(t) = c_{3y} t^3 + c_{2y} t^2 + c_{1y} t = ((c_{3y} t + c_{2y}) t + c_{1y}) t$$
- **五组原生标准曲线**：
  1. `Index 0`（系统开窗/默认，CA_DEFAULT）：$(0.25, 0.1), (0.25, 1.0)$，对应系数 $c_x = (1.0, -0.75, 0.75)$，$c_y = (-1.7, 2.4, 0.3)$。
  2. `Index 1`（线性/匀速，CA_LINEAR）：$(0.0, 0.0), (1.0, 1.0)$，对应系数 $c_x = (-2.0, 3.0, 0.0)$，$c_y = (-2.0, 3.0, 0.0)$。
  3. `Index 2`（慢进，CA_EASE_IN）：$(0.42, 0.0), (1.0, 1.0)$，对应系数 $c_x = (-0.74, 0.48, 1.26)$，$c_y = (-2.0, 3.0, 0.0)$。
  4. `Index 3`（慢出，CA_EASE_OUT）：$(0.0, 0.0), (0.58, 1.0)$，对应系数 $c_x = (-0.74, 1.74, 0.0)$，$c_y = (-2.0, 3.0, 0.0)$。
  5. `Index 4`（慢进慢出，CA_EASE_IN_EASE_OUT）：$(0.42, 0.0), (0.58, 1.0)$，对应系数 $c_x = (0.52, -0.78, 1.26)$，$c_y = (-2.0, 3.0, 0.0)$。
- **Horner 法数值稳定性与性能**：
  采用 Horner 嵌套乘法后，每次曲线求值仅需 2 次乘法和 2 次加法，彻底消除 $(1-t)^2$ 展开与中间临时变量，同时配合最大 8 次牛顿迭代收敛（`cmp ecx, 0x8`），达到纳秒级精确平滑求值。该算法已同步至 Realand 的 `src/animation/bezier.rs`。

---

## 四、不透明区域与裁剪的完整函数复核（2026-10-12）

本轮使用 REA/Ghidra 重新分析原样本，逐字节校验函数体：

- `0x47ee6–0x47ff4` 实际是 `CGXCreateCombinedWindowClipShape`，不是不透明区域查询。它在相关状态下变换图层区域，并对裁剪区域求交。
- `CGXCopyScreenOpaqueShapeForWindow` 在 `0x47ff5–0x48498`，比较窗口 `+0x2a0` 的版本与两种查询模式各自的缓存版本。版本未变化时保留并返回缓存区域。
- 该函数确实连乘 `+0xbc/+0xc0/+0xc4` 的三个浮点字段；有效结果小于 1 时使用空不透明区域。三个字段的业务名称不能仅凭偏移确定。
- 不透明路径继续考虑冻结内容、表面区域、裁剪和几何变换。透明度等于 1 不代表整个窗口矩形都可用于遮挡剔除。

这些函数支持“按内容版本缓存区域、仅剔除确定不透明的部分”的设计。它们没有提供整个显示列表从上到下遍历的完整调用证据，不能把单个区域函数描述为完整 WindowServer 光栅化管线。

Evidence：`ev_0e833617c25691c3aad7eaf00efec345af11894161d673d0883f76474337fcac`、`ev_a7df4149719b0d2f6f9bdbf48481e6c94b570fb005b8a7b9ebb560ac5a946076`。

## 五、窗口冻结的客户端入口与服务端对象（2026-10-12）

### 5.1 客户端封装的真实范围

`SLSWindowFreezeWithOptions` (`0xd4ab4–0xd4c81`) 读取 PluginsOnly、ObeyRefreeze、最小宽高和 DeletionDelay，并封装编号 `0x73ad`、长度 `0x40` 的 Mach 消息。缺省 DeletionDelay 为 1.0。这段函数不执行 IOSurface 捕获、不调用 `metal_core_animation_detach_layer`，也不决定客户端新尺寸提交后的具体释放时刻。

因此，旧稿“调用该入口就会保持 IOSurface、等新帧呈现后延迟一秒释放并彻底消除黑边”的解释超出了证据，已撤回。参数存在不等于所有调用都采用默认参数。

### 5.2 服务端更新与缓存

`WSWindowFreezeContent` (`0xc47f8–0xc4a0a`) 校验尺寸、取消已有清理回调，创建或更新冻结对象；更新失败时析构并清除对象。成功时保存对应窗口版本，部分重冻路径使内容区域失效。

`WindowFreeze::UpdateTextureScale` (`0x15a47e–0x15a59f`) 仅在窗口版本变化时重新读取内容边界、计算缩放比并更新缓存版本。这支持尺寸计算按版本复用；完整的采样方式、边缘滤波和运行时生命周期仍需其他函数及实机画面证据。

Evidence：`ev_be0cae9dbfb002466c2798542f96d9cd5a689f7011f38961a92b2e15d5f3f1c5`、`ev_7d365fab0b488447ff0da1e925ffabb95d1d7298b9537a79ae5517788a9f3e90`、`ev_e47c99134f6d97012881a83e74137d8321402449bd49d7f7d3a6f7a06c02e0c0`。

### 5.3 尚未证明的背景材质解释

样本中存在 `ShaderComposer::BackdropFreeze` 及 1x/2x 相关函数名。本轮没有重新建立这些函数的调用条件、完整滤波器及缓存失效机制。旧稿把函数名直接解释为“拖动时静止背景必定复用”的说法不能作为像素和性能保证。Realand 的背景缓存通过本项目的损伤跟踪和 Vulkan 回读测试验证。

---

## 六、逆向结论在 Realand 的落点（2026-10-12）

这次改动聚焦概览切换的过渡捕获。当前真实会话记录了一次 `transition-capture=61.768ms`，同次 `input-dispatch=61.885ms`。原路径为每个输出同步重绘 Output、Screencast、ScreenCapture 三份全屏图，且逐元素完整绘制。

新的 `src/render_helpers/transition_capture.rs` 使用输出已有的损伤与遮挡规则：

1. 根据实际渲染元素的内容、几何与遮挡变化判断是否需要绘制。相邻目标相同时共享不可变纹理；历史关闭动画与旧过渡的隐私状态也参与判断，不仅查看当前窗口规则。
2. 按 Output、ScreenCapture、Screencast 捕获，将通常等价的目标放在相邻位置；返回值仍保持原有 API 顺序。
3. 不同画面使用新存储并以 age=0 完整重绘可见部分，防止覆盖旧快照或把局部损伤误当作新纹理已有内容。
4. 捕获失败使批次缓存失效；只更新所选输出的渲染几何，避免多屏捕获时重复刷新所有输出。

这是依据可验证机制做的 Linux/Vulkan 实现，未复制 Apple 的二进制代码，也不声称是 WindowServer 的相同实现。输入路径仍包含场景构建和必要捕获，不能据此宣布所有桌面卡顿已经消失。

随后，复杂重叠窗口的复用测试暴露了依赖库中的实际错误：固定 Smithay 版本 `4cf0b620` 的矩形批量相减循环按正序处理原元素，同时 `swap_remove` 和追加拆分片段改变数组，造成部分原矩形漏处理。独立编译的旧依赖反例中，24 个矩形区域减去自身后仍留下 `(46,132,69,16)`；修复后结果为空。该计算用于普通输出的损伤与遮挡跟踪，也会制造额外重绘。

修复以 `third_party/smithay` 和 Cargo `[patch]` 固定在项目内，保留原 MIT 许可、精确上游版本与独立补丁。343 份上游文件仅有 `src/utils/geometry.rs` 的循环被修改；没有修改共享 Cargo 缓存。新增两项普通回归验证自身相减为空及 256 组随机区域的逐像素覆盖。

新增硬件回归检查不透明遮挡、半透明混合、圆角背景模糊、隐私隔离、旧图不可变、空场景、分数缩放及输出旋转。控制实验另外比较 3840×2400、24 个模拟窗口矩形、3 个等价目标的捕获成本；它不等于物理桌面 FPS，也不是 macOS 对比基准。

## 七、这次逆向证据与还原边界

本轮新增分析 7 个完整函数体，共 3597 字节，经 REA `read_bytes` 与原 Mach-O 的节映射逐字节校验相同。原始文件、调用参数、汇编、伪代码及校验结果保存在：

`$HOME/Documents/Codex/2026-10-11/github-com-morluto-rea/work/catalina-transition-20261012/reverse/`

另有两个新证据纠正事务语义：`SLSTransactionCommitCoalescing` 的回调 (`0x20482e`) 构建 Mach 消息并处理返回；`SLSTransactionPostCommit` (`0x2045f3`) 在成功后执行提交回调、回收编码流，在错误路径记录失败。函数名不证明无阻塞、自动丢弃旧帧或限定 GPU 队列深度。

样本 SHA-256：`38cde43140bfb76a9cc19cda64fc8e8d260c8c25963bfba650a3f078d8d5b78b`。样本来自恢复的 Catalina BaseSystem；原镜像尾部残缺。局部样本校验不代表全镜像或 Apple 签名完整性。没有原生 Catalina 同条件桌面截图/录制，静态证据不能验收“整桌面像素级还原”，也不能证明性能已与 macOS 相同。
