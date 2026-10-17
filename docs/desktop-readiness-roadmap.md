# 桌面就绪度清单（Desktop readiness）

目标：把 Clarion 从"可用的合成器 + Shell"推进到"能日用的一台桌面"。本清单记录
**已核对的现状证据**、**缺口**、**验收标准**与**依赖**。状态以代码和可运行的验收
为准，不以功能数量或离屏测试单独证明。

核对方式：按协议名与 Rust 类型名双向检索（例如 `idle-inhibit` 与
`IdleNotifierState`、`input-method` 与 `InputMethodManagerState`），并读取对应实现。
"缺失"指在当前树中找不到实现；若某项以未检索到的形式存在，应先补证据再改状态。

## 一、已具备的能力（证据）

| 领域 | 能力 | 证据 |
| --- | --- | --- |
| 后端 | DRM/KMS、嵌套、无头 | `src/backend/`（`disable_direct_scanout` 调试开关、`tty.rs`） |
| 渲染 | Vulkan 原生、GLES 回退、显式同步、直通扫描 | `src/render_helpers/vulkan/`、`frame_pipeline.rs::update_scanout` |
| 帧调度 | 自适应 latch 预算、按呈现时间投影、FIFO、commit-timing | `clarion-timing/src/frame_clock.rs`、`src/fifo.rs`、`src/commit_timing.rs` |
| 输出 | VRR、HDR 位深、分数缩放、gamma、`wlr-output-management` | `clarion msg outputs`（`max_bpc`/`hdr_active`/`vrr_enabled`） |
| 桌面协议 | foreign-toplevel、ext-workspace、layer-shell、gtk-shell | `src/compositor.rs` 的 delegate 列表 |
| 会话 | `ext-session-lock`、idle-notify、idle-inhibit | `IdleNotifierState`、`IdleInhibitManagerState`、`lock_surface_focus` |
| 输入 | text-input、input-method、指针约束、相对指针、快捷键抑制 | `TextInputManagerState`、`InputMethodManagerState` |
| 剪贴板 | data-control、primary-selection | `src/compositor.rs` |
| 捕获 | screencopy、PipeWire 录屏、截图 UI | `src/screencasting/`、`src/ui/screenshot_ui.rs` |
| 兼容 | Xwayland | `src/compositor.rs` |
| Shell | 顶栏、程序坞、启动台、Spotlight、调度中心、控制中心、通知中心、托盘、MPRIS、壁纸、桌面文件视图、全局菜单、电源模式、音频、自启 | `clarion-shell/lib/services/`、`lib/ui/` |
| 设置 | 外观设置 + 配置原文编辑器 | `clarion-shell/lib/ui/mac_shell_settings.dart`、`clarion-settings/`（egui） |
| 会话与打包 | logind/DBus、关机会话目标、portal 配置、会话入口、Nix/RPM、CI 与 release | `src/dbus/freedesktop_login1.rs`、`resources/clarion-shutdown.target`、`flake.nix`、`clarion.spec.rpkg`、`.github/workflows/` |

## 二、缺口

### P0：不补不算能日用

#### 1. 锁屏（UI + PAM）

- 现状：`ext-session-lock` 协议、`lock_surface`、`lock_surface_focus` 已存在；但
  **没有锁屏界面**（`src/ui/` 与 shell 中均无锁屏视图），**没有任何 PAM 调用**。
- 缺口：密码认证、锁屏渲染、失败节流、多输出一致、通知隐私。
- 验收：空闲策略或快捷键触发后进入锁屏；错误密码有节流与提示；正确密码解锁回到
  原焦点；锁屏期间不显示通知内容；`loginctl lock-session` 生效并回报
  `LockedHint=true`。
- 依赖：PAM（`pam`/`pam-sys` 或独立认证进程）、空闲策略（P1-4）、logind 的
  `Lock`/`PrepareForSleep` 信号。

#### 2. 网络

- 现状：shell 服务清单中无网络服务（`lib/services/` 无 network 相关）。
- 缺口：Wi-Fi 扫描/连接/密码与 PIN、以太网状态、已知网络、VPN 基础、无线开关。
- 验收：控制中心可看到并连接 Wi-Fi；连接过程有状态与错误反馈；密码走 secret
  agent 保存；断线自动重连；有线/无线切换不打断会话。
- 依赖：NetworkManager 或 iwd 的 D-Bus 接口、secret agent（与 polkit/keyring 协同）。

#### 3. Polkit 认证代理

- 现状：无。特权操作（挂载、装包、改系统设置）缺少密码对话框，会直接失败。
- 验收：注册到会话总线的 polkit agent；特权操作弹出 shell 风格对话框；取消/失败
  返回明确错误；与锁屏共用认证后端。
- 依赖：polkit、PAM。

### P1：日常体验明显缺

#### 4. 空闲策略（黑屏 / 锁屏 / 挂起）

- 现状：idle-notify 与 idle-inhibit 协议已有；`resources/default-config.kdl` 中
  **没有任何空闲时长或动作的配置项**。
- 验收：配置可表达"空闲 N 秒后黑屏/锁屏/挂起"，分交流电与电池；全屏视频等
  inhibitor 被尊重；唤醒后按策略决定是否要求解锁；含"演示模式"总开关。
- 依赖：P0-1（锁屏）、logind。

#### 5. 夜间模式（色温）

- 现状：gamma 协议已实现（wlsunset/gammastep 可用），但无内置定时与设置项。
- 验收：手动/按日落时间调整色温，过渡平滑（不闪），逐输出生效，配置持久化，设置
  面板可开关与调色温。

#### 6. 蓝牙

- 现状：无。
- 验收：开关、扫描、配对（含 PIN 确认）、已配对设备管理、电量显示（若设备提供）。
- 依赖：BlueZ D-Bus、配对代理（agent）。

#### 7. 显示器排列界面

- 现状：`wlr-output-management` 协议已有（可用 `wlr-randr`），但无可交互 UI。
- 验收：拖拽排列、主输出选择、分辨率/刷新率、缩放、旋转、VRR/HDR 开关；变更可
  自动回退（保持可见输出在线）；结果写入配置并可恢复。

#### 8. 剪贴板历史

- 现状：data-control/primary-selection 协议已有；无随会话提供的管理器。
- 验收：历史面板（文本 + 图片）、置顶/删除/清空、粘贴到任意客户端、随会话启动；
  密码管理器等敏感来源至少支持"忽略标记"的尽力而为策略。

### P2：打磨与生态

#### 9. 输入法会话接线

- 现状：compositor 支持 input-method 与候选窗定位（`text_input_rectangle`）；但会话
  脚本未导出 `GTK_IM_MODULE`/`QT_IM_MODULE`/`XMODIFIERS`，也没有输入法指示器。
- 验收：会话按存在的输入法框架导出正确变量；顶栏或控制中心显示当前输入法；候选窗
  位置跟随光标；文档给出 fcitx5/ibus 的最小安装步骤。

#### 10. 可访问性

- 现状：无屏幕阅读器集成、放大镜、高对比度与可访问性设置。
- 验收：Orca 可启动并朗读 shell 与 GTK/Qt 应用的关键控件（需确认 Flutter 语义树到
  AT-SPI 的桥接是否可用）；键盘可完整导航 shell；提供放大与高对比选项。
- 依赖：at-spi2、Orca；Flutter 侧语义导出需先验证。

#### 11. 文件管理器

- 现状：仅桌面内嵌文件视图（复制/重命名/属性/打开方式/搜索等交互不成体系）。
- 验收：可独立开窗；复制/剪切/粘贴/重命名/删除到废纸篓/属性/打开方式/搜索/排序/
  多选/拖放；与 portal FileChooser 协同；大目录滚动流畅。

#### 12. 设置覆盖面

- 现状：外观与配置原文编辑已有。
- 验收：网络、显示、电源、蓝牙、快捷键、默认应用（MIME 关联）面板可用，且与配置
  文件双向一致（改配置不丢 UI 设置）。

#### 13. 更新机制

- 现状：仅打包（Nix/RPM）与 release 工作流。
- 验收：版本化渠道、更新可用性提示（可选自动检查）、升级说明；不阻塞登录。

### 工程侧

#### 14. 主线程 `scene-build`

- 证据：`scripts/diagnose-latency.py --output <output> --seconds 8` 采样到
  `scene-build` 反复 6–28 ms（阈值 4 ms），同帧 `output-render` 达 29 ms，而渲染本身
  仅 0.4 ms。详见 [compositor-fluency-audit.md](compositor-fluency-audit.md)。
- 验收：Tracy 采样给出内部子阶段分解；修复后 95 分位 `scene-build` 低于 4 ms，且
  `clarion msg frame-timings` 的近期晚帧样本不增加。

#### 15. 真机矩阵

- 现状：Intel/NVIDIA、多屏混合刷新率、真实 VRR 面板的呈现偏差与延迟未系统验证
  （项目文档亦如此标注）。
- 验收：每种组合记录 `frame-timings` 的 P50/P95 与晚帧率，形成可复现的验收表。

#### 16. 打包面

- 现状：Nix、Fedora RPM 已覆盖。
- 验收：按需增加 AUR/DEB/Flatpak；每个渠道有版本化产物与安装校验步骤。

## 三、边界

- 本清单只覆盖"能否日用"的缺口，不追求功能数量对齐其他桌面。
- 标注为"缺失"的项基于当前树的检索结果；改状态前应先给出实现证据。
- 第三方依赖（NetworkManager、BlueZ、polkit、at-spi 等）不随本项目分发，需在文档
  中给出最小安装集合。
