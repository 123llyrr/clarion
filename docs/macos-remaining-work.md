# macOS 桌面复刻：剩余工作

更新日期：2026-10-11。本清单依据当前源码与历史验收记录整理；本轮新增证据、三项修复和验证范围见 [Catalina 全域复核](macos-catalina-audit-20261011.md)。下文旧会话验收时间是历史记录，本轮没有重启桌面。

**当前目标是 Catalina 桌面与系统偏好设置，加上 Big Sur 风格控制中心；这是混合版本，尚不能称为单一 macOS 版本的 1:1。** 剩余事项应区分 **缺失**（当前没有真实实现）、**待对齐**（已经实现，尚缺匹配尺度的 Catalina 实机验证）和 **平台差异**（需要 Linux 对应能力，不能用空按钮或假数据冒充）。

## 参考与运行边界

- 参考镜像为 Catalina 10.15.7／19H2。选取资源的 CodeResources SHA256 校验不等于完整镜像或 Apple CMS 信任校验；镜像尾部缺失，尚无原系统运行画面。NIB、图标和字符串能证明资源/结构，不能证明最终材质、动画曲线和物理命中边界。
- **Catalina 没有 Big Sur 引入的控制中心。** 本轮按 [Apple Big Sur 官方发布画面](https://www.apple.com/newsroom/2020/11/macos-big-sur-is-here/)实现紧凑模块，Catalina `Displays.menu` 是 AirPlay 菜单，不能作为其亮度滑条尺寸证据。布局尺寸和动画时长仍是工程值。
- 最新会话检查已确认：会话于 22:14 重新启动，已加载 `desktop-focus-20261009-221106-369717` 合成器；“还要下次登录才能加载”的早期记录已经过时。加载成功仍不等于所有物理输入和性能场景验收完成。
- 当前材料缺少同分辨率、缩放、主题、语言下的 Catalina 截图与逐帧输入记录。像素、字体栅格、阴影、模糊、节奏的“完全一致”均未证实。

主要依据：[最新完整审计](macos-desktop-fidelity-audit.md)、[Dock 对照](macos-dock-detail-comparison.md)、[Launchpad 对照](macos-launchpad-interactions.md)、[镜像研究](macos-menu-dock-research.md)。旧表与最新实现冲突时，以当前源码和晚于该表的验证记录为准。

## 分域清单

### 1. 桌面与 Finder 语义

已具备：真实文件扫描/打开/创建/回收、快捷方式信任检查、多选/框选/组拖、批量打开与确认回收、标签原位改名及失败重试、Control 点按、桌面精确组合键路由；显示简介、文本/图片快速查看、手工整理与持续排序已接入真实文件元数据。

- **缺失：** 完整 Finder 目录窗口；当前打开目录交给 Linux 文件管理器。已有按文件类别等分组的桌面叠放源码与回归，标签、批量重命名仍未实现。简介目前是可切换所选项目的单个弹窗，不是 AppKit 多窗口/汇总简介；快速查看支持受限 UTF-8 文本和首帧图片，PDF、影音、Office 等预览提供者尚缺。排序含名称、种类、修改日期和大小，不冒充可靠的创建/添加日期或完整 UTI 类型排序。
- **缺失：** 跨应用系统剪贴板、拖入文件夹与跨应用移动/复制链路；桌面服务已提供内部文件复制/粘贴并通过回归，现有组拖只改变桌面布局，不能当作文件拖放后端。
- **会话加载待验：** `⌘I`/`⌘Y` 已补合成器精确转交并通过 3 项 Rust 回归；当前运行的旧合成器尚未含此规则，右键入口和空格预览可用。新版已安装供下次登录加载，之后需验收与已有 Mod 绑定的冲突。
- **待对齐：** 当前分页网格与 Catalina 自由桌面不同；网格起点、冲突换位、Shift 范围、图标/标签尺寸、阴影及失焦改名提交规则需实机对照。回收前确认也是现有产品行为差异。
- **平台差异：** Finder 元数据、标签与预览需要真实 Linux 存储/预览提供者；不能仅补相同名称的菜单。

依据：[desktop_view.dart](../realand-shell/lib/desktop/desktop_view.dart)、[desktop_service.dart](../realand-shell/lib/desktop/desktop_service.dart)。

### 2. Dock、Launchpad 与窗口

已具备：三边 Dock、真实放大/自动隐藏、精确透明区输入、图标重排/分隔拖动调大小、最近应用、真实 XDG 登录项、真实 Downloads/Trash与清倒废纸篓、Genie/Scale；Launchpad 搜索、分页、拖动、文件夹、长按/Alt 抖动和持久布局。

- **缺失：** 真正应用级 Hide/Hide Others/Quit、应用固定工作区语义；当前 Hide 由逐窗最小化模拟，标为“退出”的动作仍只关闭窗口。Force Quit 已有进程操作及入口，本轮修复操作期间的按钮禁用与恢复，回归不实际终止用户进程。独立最小化条目和“最小化到应用图标”切换已有实现，但条目内容仍是绘制的标题／图标示意，未接真实客户窗口纹理。
- **缺失：** Launchpad → Dock 跨 native view 拖放、任意文件/文件夹拖入 Dock、拖出移除、文件拖到应用打开。右键固定不能替代这些输入链路。
- **缺失：** Launchpad 持久页内空位、拖到新空页、独立文件夹反向关闭动画；当前布局模型是 version 1 紧凑条目，拖页不越过已有页数。
- **缺失：** Shell 调度中心的实时客户窗口内容。`_SpreadWindowCard` 当前绘制应用图标/标题和装饰标题栏，未接窗口纹理；真实切工作区、聚焦和跨工作区移动已有实现。不要将其注释里的“live preview”当成功能证据，也不要与合成器原生概览混为一谈。
- **待对齐：** Dock 放大传播、弹跳、自动隐藏延迟、Stack/Fan 动效、Launchpad 翻页/惯性/文件夹阈值、Genie/Scale 曲线均仍需逐帧参考。Downloads 存在 256 项扫描、Fan 8 项等工程上限，完整层级/预览体验尚不同。
- **平台差异：** 应用卸载、进程生命周期、`.desktop` 扩展动作、最近文稿及其他 Dock tile 类型需要各自后端，不能靠 macOS 外观自动获得。

依据：[mac_dock_app_menu.dart](../realand-shell/lib/ui/mac_dock_app_menu.dart)、[mac_dock_view.dart](../realand-shell/lib/ui/mac_dock_view.dart)、[mac_launcher_panel.dart](../realand-shell/lib/ui/mac_launcher_panel.dart)、[mac_launchpad_layout.dart](../realand-shell/lib/mac_launchpad_layout.dart)、[调度中心卡片](../realand-shell/lib/ui/mac_mission_control_panel.dart#L674)。

### 3. 菜单栏、系统菜单与 Spotlight

已具备：GTK/canonical DBus 导出的真实应用菜单、独立音量菜单及输出设备切换、真实系统信息/最近应用/会话电源操作、紧凑系统菜单。

- **缺失：** 最近项目只有应用，没有文稿/服务器；系统菜单已有可用强制退出入口并通过 widget 回归，不再列为禁用。桌面激活时也未提供 Finder 式的完整菜单组及对应操作。
- **缺失：** 当前菜单栏没有完整独立 Wi-Fi、蓝牙、AirPlay/显示器菜单及其设备/网络选择流程，亦未实现完整 Menu Extra 管理/重排体验。
- **缺失：** Spotlight 当前搜索应用、设置入口和简单算式，未提供文件名/内容索引、文稿、邮件、联系人等系统搜索源。
- **待对齐：** 菜单栏高度、图标间距、状态图标、菜单行高/阴影、键盘子菜单、紧凑音量滑条与对齐锚点仍需 Catalina 截图和输入对照。
- **平台差异：** 全局应用菜单只对实际导出菜单的 Linux 应用可用；非导出应用没有可读取的完整菜单。电源/锁屏和声音偏好由 logind、swaylock、pavucontrol 等真实组件提供，界面与 AppKit 不同。

依据：[mac_shell_view.dart](../realand-shell/lib/ui/mac_shell_view.dart)、[mac_system_menu.dart](../realand-shell/lib/ui/mac_system_menu.dart)、[mac_spotlight_panel.dart](../realand-shell/lib/ui/mac_spotlight_panel.dart)、[global_app_menu_service.dart](../realand-shell/lib/services/global_app_menu_service.dart)。

### 4. 控制中心与通知

已具备：音量/亮度滑条实时订阅、合并写入和过期回读保护、静音、真实无线开关、事件订阅的 MPRIS 媒体卡片；通知中心右侧栏、今天/通知切换、月历、分组清除、真实动作及可靠通知生命周期。

- **已改、仍待实机对齐：** 控制中心主视图已改紧凑连接／勿扰／外观卡片与显示器／声音胶囊滑块，沿菜单栏右边缘展开；长外观表单移入更宽的居中桌面偏好子页。面板圆角区域接入原生背景模糊，声音展开接既有真实输出菜单。仍缺原版完整模块、各模块展开层级、拖入菜单栏与自定义排列；不能把当前适配布局称为原版逐像素一致。
- **缺失：** AirDrop、屏幕镜像、键盘背光、Night Shift／True Tone 等真实后端；主面板暂不显示这些空壳。MPRIS 现已接入真实播放器发现、状态、切换及上一首/播放暂停/下一首，按播放器实际能力启用；仍无封面、进度拖动、AirPlay/音频投送。
- **缺失：** Wi-Fi 扫描/选择/密码加入、蓝牙发现/配对/设备选择 UI；当前主要是开关与状态。亮度后端是 `brightnessctl` backlight，不是通用外接屏逐屏 DDC 控制。
- **缺失：** 通知主动横幅/声音提供者、Today 天气/账户日程/股票/第三方组件编辑；当前月历不是已连接账户的日程组件。DND 只是 Realand 状态，尚非全系统/按时退出勿扰策略。
- **待对齐：** 通知侧栏宽度、底色/模糊、文字、清除动作、进退动画仍缺真实 Catalina 同尺度对照。已经过真实 widget 检查，不应再列为“没有通知中心”。

依据：[quick_settings_panel.dart](../realand-shell/lib/ui/quick_settings_panel.dart)、[system_service.dart](../realand-shell/lib/services/system_service.dart)、[mac_notification_center.dart](../realand-shell/lib/ui/mac_notification_center.dart)、[notification_service.dart](../realand-shell/lib/services/notification_service.dart)。

### 5. 系统偏好设置

已具备：原图标分类总览、搜索/历史/全部显示、八个真实入口（通用、窗口布局、调度中心、显示器、键盘、触控板、更多设置、配置文件）、草稿保护与真实 KDL 保存、双键盘滑块、触控板三页、触发角和真实输出排列/回退；已有真实 700×580 悬浮窗口验收。

- **缺失：** Catalina 其余系统偏好领域并没有对应完整面板与服务；目前的八个真实入口不能视为整套 macOS 偏好设置。尤其账户、声音完整偏好、网络、色彩管理/校准等需按真实 Linux 能力补齐。
- **待对齐：** 主 System Preferences.app/NIB/PrefApp.icns 未恢复；应用图标暂用标准 `preferences-system`。工具栏/控件/间距、原生窗口行为、主题及无障碍状态未完成 AppKit 实机对照。设置当前主要按浅色样式实现。
- **平台差异：** Linux 键盘/libinput/触控板能力与 Apple 硬件不同；只配置已有参数，不能把不存在的手势、影片或设备信息做成可用选项。

依据：[preferences_home.rs](../realand-settings/src/preferences_home.rs)、[app.rs](../realand-settings/src/app.rs)、[设置主题](../realand-settings/src/theme.rs)。

### 6. 字体、材质与动画

已具备：镜像字体/多组原图标、字体加载与图标纹理缓存、原生背景模糊、减少动态效果、菜单与侧栏独立过渡。

本轮新增：窗口移动/缩放交互弹簧、255 ms 开窗参考配置、连续 Genie 收口及拖放释放动量交接。
这些调校与镜像中确认的事实分开记录；REA 复核纠正了将 Sheet 动画及数值搜索常量当作
Genie 和统一时长的误读，详见 [动画方案实施记录](macos-animation-scheduling.md)。

- **待对齐：** SF/PingFang 文件相同不代表 Flutter/Skia/egui 与 CoreText 栅格相同；字重、基线、字距、fallback、亚像素边界仍需截图差异分析。
- **待对齐：** 玻璃饱和度、模糊半径、透明度、圆角、阴影、深浅主题及高对比度状态缺统一实机标尺；现有尺寸/时长/缓动多是工程值，不能宣称 Apple 原值。
- **平台差异：** AppKit vibrancy、系统强调色传播、原生应用窗口装饰不是共享同一个绘制栈；需明确可保证的一致范围。

### 7. 输入、性能与多屏

已具备：滑条更新路径修复、派生数据/Launchpad 目录缓存、托盘图标修订缓存、通知合并、原生输入区去重与精确命中、多屏逻辑尺寸同步，以及 Vulkan 区域模糊计算优化。壁纸面板已补异步状态订阅、移除绘制期同步文件检查，并复用同一未修改图片的健康背景进程；可见壁纸缩略图增加持久缓存、版本失效、最多两路低优先级生成、取消与预算清理。缓存依赖可选的 vipsthumbnail，缺失时回退既有 320 px 原图预览。

- **待测量：** 壁纸首次应用仍由 swaybg 解码原图并提交各输出 buffer；本轮未降低画质，尚无真实首次解码耗时或加速比例。状态刷新修复与重复同图复用不能替代这项测量。

- **未收口：** 用户描述的偶发整桌面停顿尚无已定位且闭环的根因。22:11 的短样本出现 Shell 多线程 CPU 307.7%、4822 次重大缺页和约 231.8 MiB 读取，但缺同时交互记录与调用栈；不能把峰值/Swap 本身当根因，也不能宣布卡顿消失。
- **诊断已修、仍待定位：** journald 字节数组日志现能正确解析；历史 `client-dispatch` 有 124.9/94.6 ms 尖峰。30 秒无负载样本无迟到呈现或大缺页，不等于已消除偶发停顿。原生 Vulkan 调整大小快照已移除重复 CPU 等待，由同队列顺序、屏障与完成资源所有者保护复制；GLES 快照同步失败的永久重试及设备失效恢复仍待改造，尚未证明它触发了这些历史尖峰。
- **待验证：** 新会话持续交互帧时延、长时间 RSS/Swap 趋势、应用冷启动/大目录/图片通知/菜单切换压力，以及物理鼠标/触控板/IME/快捷键。当前优化并未减少全部背景捕获带宽/纹理内存，GLES 路径也不能等同 Vulkan 收益。
- **实现风险待针对性复现：** Dock 最小化目标登记尚无输出所有者令牌/条件清除；跨输出旧登记清除有效目标的风险在旧对照文档中仍待收口，不能直接当作当前已观察故障。
- **待验证：** 混合 DPI、分数缩放、热插拔、旋转、跨屏窗口/弹窗/Dock、不同 GPU 与高刷新率的物理验收矩阵。已有逻辑尺寸和边界测试，不能重复列为“完全不支持多屏”。

依据：[性能记录](macos-desktop-fidelity-audit.md#L209)、[Shell controller](../realand-shell/lib/state/shell_controller.dart)、[Dock 对照风险](macos-dock-detail-comparison.md)。

## 建议收口顺序

1. 为当前“Catalina 桌面 + Big Sur 控制中心扩展”保存同尺度截图、录屏与输入轨迹；如目标改为单一系统版本，需要统一所有组件的版本基线。
2. 延续当前用户重点：紧凑控制中心真实操作与亮度/音量全链路验收；同时捕获一次有交互上下文和调用栈的实际停顿。
3. 补高频且可验证的真实能力：跨应用文件剪贴板/拖放及更多格式预览，跨 view Launchpad→Dock，真实窗口预览；桌面内部复制/粘贴已有实现，不要重复开发。
4. 再补应用生命周期、通知横幅与 provider、多屏所有权风险；最后按固定基线逐项精修像素与动画，并明确硬件/平台例外。

**不可继续照抄的旧缺项：** “登录时打开未做”“Launchpad 没有抖动”“桌面没有多选/原位改名”“音量仍打开整个快速设置”“没有今天/通知侧栏”“设置中心还只是旧侧栏”“原生更新尚未加载”“桌面没有简介/文本图片快速查看/整理排序”“控制中心没有真实媒体控制”均已被后续实现或最新运行记录推翻。
