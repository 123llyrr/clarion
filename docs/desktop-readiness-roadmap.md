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

## 二、能力来源分类（生态代劳 vs 必须自做）

桌面能力大多**不需要自己实现**：只要合成器提供协议、会话提供接线，现成客户端就能用。
下表把每一项分成"生态现成工具 / 已核实的协议前提 / Clarion 需做的最小工作"。

| 领域 | 生态现成工具 | 协议前提（本仓库已核实） | Clarion 需做的最小工作 |
| --- | --- | --- | --- |
| 锁屏 | `hyprlock`、`swaylock`、`gtklock` | `ext-session-lock`、`session_lock_manager` ✓ | **已接线**：`idle { }` 的超时动作会启动 `lock-command`；PAM 由锁屏工具自带 |
| 网络 | NetworkManager + `nm-applet`／`plasma-nm` | D-Bus（已有） | secret agent 与 polkit 代理可用；可选原生面板 |
| 认证 | `polkit-gnome`、`lxpolkit`、`hyprpolkitagent` | D-Bus（已有） | 会话启动其中一个；可选 shell 风格对话框 |
| 空闲 | `swayidle`、`hypridle` | `idle_notify`、`idle-inhibit` ✓ | **已内置**：`idle { timeout; action; lock-command; lock-before-suspend }`，尊重 inhibit；想用外部守护则 `idle { off }` |
| 色温 | `gammastep`、`wlsunset`、`hyprsunset` | gamma ✓ | 启动与设置项；可选内置定时 |
| 蓝牙 | `blueman`、`overskride`、`blueberry` | BlueZ D-Bus | 会话启动 + 托盘项；可选原生面板 |
| 剪贴板历史 | `cliphist` + `wl-clipboard` | `data_control`、`primary_selection` ✓ | 启动 + 选择器（`wofi`/`rofi` 或原生面板） |
| 显示器排列 | `wdisplays`、`nwg-displays`、`kanshi` | `output-management` ✓ | `kanshi` 式配置驱动自动应用最贴合本项目；可选原生 UI |
| 文件管理器 | `dolphin`、`nautilus`、`thunar`、`nemo` | `xdg-decoration`、portal FileChooser | 默认应用与 MIME 关联；桌面视图与外部管理器分工 |
| 输入法 | `fcitx5`（含中文引擎） | `input-method`、`text-input` ✓ | **会话导出 `GTK_IM_MODULE`／`QT_IM_MODULE`／`XMODIFIERS`** + 文档 |
| 可访问性 | `Orca` | `at-spi2`（需验证 Flutter 语义桥） | 验证桥接并写验收；可选放大与高对比 |
| 截图/录屏 | `grim`／`slurp`／`wf-recorder` | `screencopy`、PipeWire ✓ | 已内置 UI；外部工具可用属附带能力 |
| 状态栏/启动器替代 | `waybar`、`fuzzel`、`rofi` | `wlr-layer-shell`、`foreign-toplevel` ✓ | 无需工作；说明可替换 Shell 组件 |
| 登录管理器 | 任意 DM（`greetd` + `gtkgreet`／`tuigreet`） | 会话入口已提供 ✓ | 文档化；可选会话选择器主题 |

### 结论：真正属于 Clarion 的工作只有三类

### 已完成（本轮）

合成器现在自己执行空闲策略，不需要外部空闲守护：

```kdl
idle {
    timeout 300             // 秒；0 表示从不
    action "lock"           // "lock" | "suspend" | "none"
    blank-timeout 600       // 独立于动作；到时通过连接器 DPMS 熄屏，输入即亮
    lock-command "swaylock" // 锁屏界面由客户端提供，PAM 由它自带
    lock-before-suspend
}
```

动作在合成器的一秒 tick 上判定；空闲抑制面与 freedesktop idle-inhibit 生效时不动作，
已有会话锁时不再重复请求；每个空闲周期只动作一次，任何输入重新计时。熄屏走连接器的
`DPMS` 属性（`ConnectorProperties::set_dpms`），只在该属性实际变化时提交原子请求；
面板关闭期间会话仍保持合成，输入到达时立即亮屏并重绘（`pending_dpms_blank` 由拥有后端的
刷新周期消费，因此唤醒不受一秒 tick 限制）。熄屏独立于动作、且在已锁屏时依然生效——
锁屏后屏幕应当熄灭。策略是纯逻辑模块（`src/idle.rs`），不持有时钟与进程，单独有单元测试。

仍未完成的部分（同一节内）：按电源来源分别计时（需要查询 logind 的 `OnBattery`），以及由
logind `PrepareForSleep` 或合盖发起的挂起前锁定（需要订阅该信号；目前只有本策略自己发起的
挂起会先锁屏）。

2. **会话接线（不可省略）**——把上面这些工具"接起来"，让全新安装不是一片空白：
   导出输入法/光标/主题环境变量；随会话启动选定的守护（项目已有 `login_items_service`
   自启通道）；确认 `resources/clarion-portals.conf` 的 ScreenCast／Screenshot／
   FileChooser 路由；按发行版给出**最小依赖包集合**（见附录）。
3. **一致性（产品选择，非功能必需）**——接受"一栈独立 applet"（waybar 风格），还是把它们
   汇入顶栏/控制中心（网络、蓝牙、剪贴板、色温、空闲状态）。这是"像一个桌面"与
   "能用"的区别，也决定后续 UI 工作量。
4. **产品决定（安全相关）**——已按"内置"选型落地：空闲
   策略由合成器执行，锁屏界面由配置的 `lock-command` 提供（推荐 `swaylock`／`hyprlock`／
   `gtklock`，均自带 PAM）。

生态代劳不了的两项工程工作仍然存在：

- **主线程 `scene-build`**：`scripts/diagnose-latency.py --output <output> --seconds 8`
  采样到 6–28 ms（阈值 4 ms），同帧 `output-render` 达 29 ms，而渲染仅 0.4 ms。详见
  [compositor-fluency-audit.md](compositor-fluency-audit.md)。
- **真机矩阵**：Intel/NVIDIA、多屏混合刷新率、真实 VRR 面板的呈现偏差与延迟未系统验证。

## 三、边界

- 本清单只覆盖"能否日用"的缺口，不追求功能数量对齐其他桌面。
- 标注为"缺失"的项基于当前树的检索结果；改状态前应先给出实现证据。
- 第三方依赖（NetworkManager、BlueZ、polkit、at-spi 等）不随本项目分发，需在文档
  中给出最小安装集合。

## 附录：最小依赖包集合（Arch 包名，其他发行版按同名包替换）

```
# 会话与门户
xdg-desktop-portal xdg-desktop-portal-gtk
# 网络与认证
networkmanager polkit polkit-gnome gnome-keyring
# 空闲与锁屏
swayidle hyprlock        # 或 swaylock / gtklock
# 显示与色温
wdisplays kanshi gammastep
# 蓝牙
bluez bluez-utils blueman
# 剪贴板
wl-clipboard cliphist
# 输入法（中文）
fcitx5 fcitx5-chinese-addons fcitx5-gtk fcitx5-qt
# 捕获（可选，替代内置 UI）
grim slurp
# 可访问性（可选）
orca
# 字体（必需：缺中文族会让界面文字变成方块）
noto-fonts-cjk ttf-dejavu
# 图标主题（应用图标由合成器按主题解析）
hicolor-icon-theme adwaita-icon-theme
```
