# Catalina 桌面还原审计

核查日期：2026-10-09。参考版本：用户镜像中的 macOS Catalina 10.15.7 / 19H2。此文依据当前源码重新核查，不把旧矩阵里的“未实现”直接当作现状。功能与组件测试通过不等于已完成真实 Catalina 的逐像素、逐帧对照。

## 证据范围

| 证据 | 本次实际检查 | 可以支持的结论 |
| --- | --- | --- |
| 镜像静态文件 | `system-version.json`；恢复的 Dock、Dock.prefPane、Volume.menu、Displays.menu 的 plist、NIB、中文字符串及可见 Mach-O 符号表 | 版本、组件、文案、偏好绑定、导入接口的存在 |
| Apple Catalina 帮助 | URL 明确选择 `10.15/mac/10.15`，链接见下文 | 面向用户的操作和选项语义 |
| Apple 当前键盘文档 | Command-O、Command-Down、Delete 符号的含义 | 长期键盘约定的补充依据；不冒充 Catalina 私有实现或运行测量 |
| Realand 源码与组件测试 | 文件动作、事件修饰键、重复键、Launchpad 状态、菜单内容 | 本项目实际逻辑与回归结果 |
| 尚缺的运行证据 | 同分辨率/缩放/主题下的 Catalina 截图、真实输入和逐帧采样 | 精确材质、字体栅格、阴影、命中边界、动画曲线仍不能标为完全一致 |

镜像提取根目录：

```text
$HOME/Documents/Codex/2026-10-06/jia/work/macos-image-inspection-20261008
```

系统组件位于其 `menu-dock-research-20261009/selected/`。原镜像文件没有被改写，也未执行 Apple 二进制。整镜像尾部缺失的既有校验边界见 [镜像研究](macos-menu-dock-research.md)。

## 本轮已纠正的交互

### 桌面文件键盘操作

`clarion-shell/lib/desktop/desktop_view.dart` 现在为 macOS 风格单独匹配完整组合键：

| 操作 | macOS 风格 | 验证 |
| --- | --- | --- |
| 重命名选中项 | Return，兼容小键盘 Enter | 不再误启动；沿用现有真实重命名服务及位置保存 |
| 打开选中项 | Command-O / Command-Down | Command-Down 不先移动选择；普通方向键仍导航 |
| 移到废纸篓 | Command-⌫ | Apple 的 Delete (⌫) 对应 Flutter `backspace`；前向 Delete 是另一按键，不触发文件动作 |
| 新建文件夹 | Shift-Command-N | 无选中项时也能在桌面创建；沿用名称输入与目录服务 |
| 修饰键/重复键 | 精确匹配，仅首次按下发起文件动作 | Option/Shift/Control 组合不误落入其他动作；长按打开不重复启动 |

Material 继续使用原有 Enter 打开、F2 重命名、Delete 回收语义。非当前页面的条目、无选中项、文件服务忙碌时均不发起选中项操作；弹出菜单/对话框期间不透传动作到底层桌面。原有垃圾箱确认对话框和重命名对话框仍是 Realand 行为，本轮没有改成 Finder 的内联编辑或直接回收。

依据：[Catalina 重命名](https://support.apple.com/en-gb/guide/mac-help/mchlp1144/10.15/mac/10.15)、[Catalina 删除文件](https://support.apple.com/en-gb/guide/mac-help/mchlp1093/10.15/mac/10.15)、[Apple 键盘约定](https://support.apple.com/en-us/102650)。

### Launchpad 重复键

现有 Launchpad 已有长按进入抖动状态、Option/Alt 编辑入口、Esc 退出编辑、文件夹改名、同页与跨页拖动。本轮修复了左右 Alt 的 `KeyRepeatEvent` 被当成新按下、反复开关抖动的问题。仅首次按下切换，重复事件被消费且保持当前状态；未根据静态资源猜测 Option 抬起规则、长按阈值或苹果动画周期。

依据：[Catalina Launchpad 帮助](https://support.apple.com/en-gb/guide/mac-help/mh35840/10.15/mac/10.15)证明长按抖动、文件夹和分页操作；[Apple 卸载帮助](https://support.apple.com/en-us/HT202235)补充 Option 入口。重复事件应不反复切换是本项目可复现的状态错误，不是从镜像得出的时间参数。

## 后续轮次：桌面选中集合与真实批量操作

macOS 风格现支持 Command 点击添加/取消单项、Shift 点击及 Shift 方向键扩展选择、Command-A 选中当前可见页、空白区域拖框选择。按住 Command 或 Shift 拖框会保留原有选择；Esc 或指针取消恢复拖框前的选择。换页/换输出不保留不可见的操作目标，Material 仍为单选。

依据为 [Apple Catalina 选择条目说明](https://support.apple.com/en-ae/guide/mac-help/mchlp1378/10.15/mac/10.15)：Command 多选/取消单项、Shift 相邻范围、拖框和 Command-A 均有明确说明。此说明没有给出自由排列桌面图标的复杂 Shift 锚点规则；Realand 本轮采用当前页按列、行排序的网格范围规则，并不把该工程选择称为 Catalina 自由桌面的精确行为。分页本身也是 Realand 的现有实现。

选中集合现可通过 Command-O / Command-Down 与上下文菜单批量打开，通过一次确认批量送入回收站。未受信任的应用快捷方式在任何项目打开之前集中确认。服务先验证整组选中路径，再按项重验桌面根目录和路径并调用真实 GIO / 应用启动服务；重复路径去重，首个错误停止后续项，错误包含具体项和此前完成数量。部分回收后重新扫描真实目录，保留仍存在的选中项；不重试启动，也没有递归删除回退。

组拖动保持选中图标的相对网格位置，碰到未选中图标时将这些图标放入本组腾出的原位置，沿用既有单项交换规则。目标越界、跨页或缺失成员时整组不保存；Esc/指针取消只恢复视图。该冲突策略不是从 Apple 私有实现恢复的参数。重命名仍仅支持单项；批量重命名、拖入文件夹/跨应用移动、Finder 内联重命名及无需确认回收仍是差距，本轮不添加假动作。

定向验证：桌面视图、服务与布局共 74 项测试通过，覆盖真实隔离目录里的批量路径预检、部分失败、操作中根目录被替换、多选打开、框选/组拖取消、碰撞交换、快捷方式信任与可见页隔离；六个修改文件的定向 analyze 无问题。

## 原生快捷键链路

审计时已发现真实集成冲突：`~/.config/realand/config.kdl` 与仓库默认配置均把 `Mod+O` 绑定到概览、`Mod+Down` 绑定到窗口焦点移动。TTY 默认 Mod 为 Super。`src/input/mod.rs` 的配置快捷键拦截发生在事件发给 Flutter 之前；原桌面没有针对这些键的焦点路由例外，也没有注册全局快捷键 inhibitor。

本轮已保留原生 namespace `realand-desktop`，另通过现有 GTK v1 的 per-surface `application_id` 元数据区分 `io.github.realand.Desktop.MacOS` 与 `io.github.realand.Desktop.Material`。Dart 启动和实时风格切换顺序发送到 runner；runner 创建自己的元数据对象，在映射后重发，避免 GTK 自身映射元数据覆盖。元数据只作用于桌面 base surface，菜单/对话框 namespace `realand-desktop-interaction` 不获此标记。旧合成器没有 GTK 元数据接口时保留原配置拦截，稳定降级。

原生输入仅在逻辑焦点是 LayerShell、实际 seat 焦点也正是该 base surface、namespace 与 macOS 标记都匹配时，向桌面转发 Super-A、Super-O、Super-Down、Super-Backspace、Shift-Super-N 五类精确组合。普通应用、Material、弹窗、额外修饰键和其他 Super 组合保留既有绑定；锁屏请求等待阶段起，以及截图、MRU、退出确认和快捷键覆盖层均优先。释放仍走既有 `suppressed_keys` 配对机制，焦点/风格改变、先松开 Super 都不会泄漏被拦截按下对应的释放。本轮没有修改个人快捷键配置，也没有注册全局快捷键 inhibitor。

这条链路属于 Realand 在 Wayland 上实现 Finder 操作的工程方案，不是从 Apple 镜像中提取的私有路由。GTK helper 的“一次发布”限制依据 [GTK 3.24.43 源码](https://raw.githubusercontent.com/GNOME/gtk/3.24.43/gdk/wayland/gdkwindow-wayland.c)，因此使用可更新的独立协议对象；v1 没有 destructor request，销毁本地 proxy 时不发送错误 opcode。此原生变更需新版合成器启动后生效；真实会话的物理组合键验收仍应与源码/组件测试分开记录。

## 当前差异与后续优先级

| 项目 | 依据 | 当前源码差异 | 可实现的下一步与边界 |
| --- | --- | --- | --- |
| 独立音量菜单 | `Volume.menu` 原字符串含“音量：”“输出设备：”“声音偏好设置…”；[Catalina 音量帮助](https://support.apple.com/en-gb/guide/mac-help/mchl1f98810b/10.15/mac/10.15) | 已改为锚定音量图标的独立菜单，真实音量/静音、输出设备选择和声音偏好设置均接通 | 260px/行高/锚点为工程实现；仍需真实 Catalina 同尺度截图验证滑条绘制、材质和尺寸 |
| 已有桌面细节 | Catalina 重命名帮助、镜像 Finder 资源；本轮隔离文件/原生 seat 回归 | 已细修局部选中、高亮菜单、Control 点按、标签原位改名、失败重试与短屏文字布局；保留用户位置 | 当前旧compositor的菜单改名保留弹窗；原位菜单焦点恢复需重新登录。排列起点、完整菜单、叠放/标签/预览及像素材质仍有差异 |
| 桌面多选后续 | [Catalina 选择条目帮助](https://support.apple.com/en-ae/guide/mac-help/mchlp1378/10.15/mac/10.15) | 本轮已有选中集合、框选、批量打开/确认回收与组拖；Shift 使用 Realand 网格顺序 | 仍需真实 Catalina 验证自由桌面 Shift 细节，补真实批量重命名、文件夹/跨应用拖放；不能把框选高亮等同这些后端能力 |
| Apple 菜单 | [Catalina Apple 菜单](https://support.apple.com/en-gb/guide/mac-help/mchlp1130/10.15/mac/10.15) | macOS 已独立为紧凑 MacSystemMenu，真实关于/偏好/最近应用/会话和电源操作；Material 保留原菜单 | 强制退出仍禁用；最近项仅应用，没有文稿/服务器；关于/确认框的窗口样式、菜单材质和可用商店入口仍有平台差异 |
| 通知中心结构 | [Catalina 通知中心](https://support.apple.com/en-gb/guide/mac-help/mchl2fb1258f/10.15/mac/10.15)；本轮镜像 NotificationCenter 中文字符串与 NIB | macOS 已有独立贴右侧栏、今天/通知双视图、真实日期/月历、按条/天/全部清除及真实通知动作；Material 保留原面板 | 天气、日程、第三方widget编辑尚无provider；DND只表示Realand状态，不冒充全系统静音/计划；材质及动画仍需实机对照 |
| Dock 应用菜单 | 镜像 `DockMenus.plist` 含 `OPEN_AT_LOGIN`、`HIDE`、`HIDE_OTHERS`、`QUIT`、`FORCE_QUIT` | “登录时打开”本轮已实现持久配置与状态勾选；“关闭所有窗口”确实只关闭窗口，隐藏/真正退出/强制退出尚无完整应用生命周期 | Quit/Hide 必须识别真实应用/进程，不把关闭窗口冒充终止后台应用 |
| Launchpad 到 Dock 拖动 | Catalina Launchpad 帮助明确支持 | 当前是各 Flutter view 内部拖放；Dock `DragTarget` 只用于内部重排，未见跨 native view 的 Launchpad 传输桥 | 增加可靠跨 view 拖放协议并以物理输入验证；右键固定不等同该交互 |
| Launchpad 页内布局 | 当前 `MacLaunchpadLayout` 仅保存 version 1 紧凑条目；`_scheduleDragPage` 不越过既有页数 | 长期页内空位和最后一页拖向新页未实现 | 扩展布局模型并迁移旧数据。具体新页阈值与长期空位规则仍需 Catalina 运行参考，镜像默认布局 plist 不足以证明 |
| 桌面预览、整理和标签 | Catalina 桌面/文件帮助有 Quick Look、整理、标签等功能 | 当前桌面菜单仅打开、重命名、回收与新建；未提供预览、排序/叠放与标签 | 优先实现可对接真实文件元数据与预览器的能力；不虚构 Finder 数据库或缩略图缓存 |
| 模糊、图标、动画细节 | 镜像可提供纹理/原图和偏好名称 | 放大、Launchpad 抖动、文件夹开合等参数仍是 Realand 实现 | 为固定参考场景建立同尺度截图与帧时间对照；不能把原资源哈希匹配视为视觉已完全一致 |

此表不是要求一次性把所有项目换成 macOS 文案；优先保持真实动作与可用输入，再对齐布局。桌面热区、原生窗口与第三方应用之间的交互必须纳入测试。

## Catalina 与控制中心的版本边界

这份镜像的 Catalina 没有 Big Sur 引入的控制中心。[Apple Big Sur 发布说明](https://www.apple.com/newsroom/2020/06/apple-introduces-macos-big-sur-with-a-beautiful-new-design/)明确介绍该新功能。Realand 现有控制中心是扩展入口，可以保留，但不能称为这份镜像原生布局。

原 `Displays.menu` 的 bundle 名称是 AirPlay Displays Menu，并非普通亮度滑条。Catalina 亮度主要通过显示器偏好、键盘或 Touch Bar 调节；[Catalina 亮度帮助](https://support.apple.com/en-gb/guide/mac-help/mchlp2704/10.15/mac/10.15)另列电池菜单的一次性节能亮度动作。现有提取没有提供亮度/音量滑条完整绘图尺寸，不能据此声称某个圆角或像素高度是 Apple 原值。

## 本轮登录项修复

Dock 的“登录时打开”已从仅关闭菜单的占位动作改为真正的用户登录项。打开菜单异步读取有效配置并显示勾选，写入期间禁用重复点击；写入失败保留菜单并显示原因。菜单高度计数也补上了原先遗漏的该行。

Linux 后端使用 [XDG Autostart 规范](https://specifications.freedesktop.org/autostart/latest/)：读取用户优先的同名 `.desktop` 文件，在用户 `autostart` 目录原子写入覆盖；关闭系统登录项用 `Hidden=true`，不改系统文件。保留原始 Exec 引号、字段码、本地化名称和 Desktop Action 段，遵循当前桌面可见性及 TryExec。启用登录项不立即启动程序；预览仅保存内存状态。条件登录项无法可靠启用时报告真实错误，不假装成功。

## 本轮卡顿证据与修复边界

- 修复 `ShellController` 反复构造窗口、工作区、Dock 应用与最近应用列表的开销。缓存以桌面 revision 和不可变应用/固定/最近列表为依据，真实状态变化时才重建。
- 修复 Launchpad 每个图标布局反复构造整份应用 Map 的开销。1,000 应用的 widget 流程中，首次目录元素遍历从 105,000 次降至 2,000 次，翻页/文件夹/搜索流程从 704,000 次降至 0；目录替换时立即重建，显示、搜索和启动使用新对象。
- 1,000 应用、50 窗口的合成测试中，120 组已预热的重复读取由约 1,104 ms 降至约 0.111 ms，应用 ID 访问从 12,084,120 次降至 0。这里统计的是内存对象字段访问，不是磁盘读取；该 debug 基准不能换算成实际桌面帧率。
- 当前会话 30 秒基线记录 1,302 次提交、4 次延迟，采样窗口内没有合成器重大缺页，最近渲染最大约 2.36 ms。这没有复现用户描述的偶发整桌面停顿，不能据此认定已消除。
- 新增 scene-build、Vulkan/GLES render、KMS submit、DMA-BUF import 子阶段慢日志，以及 PSI、换页、IO 与事件时间关联。Rust 日志需启动新版合成器后才生效；诊断脚本可立即用于旧版会话。
- 大透明 surface 的小模糊区域已采用保留完整捕获/纹理/采样坐标的 Vulkan scissor 优化，减少各 pass 实际片元计算。完整捕获带宽、纹理内存和 GLES 路径尚未减少；不能据此宣布整桌面偶发停顿已根治。

## 定向验证

- `desktop_view_test.dart`：21 项通过，含真实隔离目录下的改名/创建、确认回收、风格切换、选择保留、修饰键与重复键。
- `desktop_application_test.dart`：3 项通过，验证启动 macOS 标记、preview 不发布、运行时 Material/macOS 来回切换、非风格偏好更新不重复发送。
- `mac_launcher_jiggle_test.dart` + `mac_launcher_panel_test.dart`：28 项通过，含左右 Alt 连续重复以及现有搜索、分页、文件夹和拖动回归。
- Rust 完整 lib test 编译通过，`input::tests` 5 项与 `tests::gtk_shell` 2 项均通过。新增测试覆盖精确组合、焦点/修饰键改变后的按下释放配对，真实 Wayland surface 元数据更新、Material、交互层、seat 与逻辑焦点不一致、MRU/退出确认/快捷键覆盖层以及真实锁屏请求等待阶段；截图/概览/锁屏的逻辑焦点排除也已覆盖。
- `desktop_windows.cc` release C++ 对象编译通过；GTK v1 消息顺序及签名已按仓库协议 XML 独立复核。发布构建与整套回归由本轮主任务汇总。
- 上述四个修改 Dart 文件已执行 `dart format`；定向 `flutter analyze --fatal-infos` 通过。
- 测试显式使用本轮工作目录中的独立 `XDG_RUNTIME_DIR`，避免测试触及正在运行的 shell socket。未重启会话、修改个人快捷键配置或安装系统组件。
- 本节不代表同一轮其他并行修改的完整回归；构建、原生快捷键路由、实际帧时间与物理输入结果以对应最终验证记录为准。

## 本轮最终验证与发布（2026-10-09）

- 最终完整 Flutter 回归 **724 项通过**；静态分析无问题；Shell Linux release 构建通过。
- 原生输入测试 5 项、GTK 私有 Wayland 连接集成测试 2 项通过，涵盖精确修饰键、释放配对、真实 surface 风格标记、焦点隔离、遮罩与待锁屏状态。诊断另有 Rust 5 项和 Python 6 项定向回归通过。
- 合成器最终优化 release 构建与本机配置校验通过，已备份旧安装并准备供下次登录使用。当前合成器进程保持运行，尚未做新版本物理键盘端到端验收。
- 新 Shell 已安装并运行，服务状态 active/running、重启计数 0；运行文件哈希与安装收据一致，命令 socket 可连接。
- 更新后再次采样 30 秒：没有记录到慢操作日志或合成器重大缺页。这是短时观测，不能证明偶发停顿已彻底消除，也没有进行匹配 Catalina 实机的逐像素/逐帧验收。

本机完整日志、基线、发布与备份收据保存在本轮 `work/desktop-fidelity-20261009/` 工作目录。以上发布状态不把表中尚缺的音量独立菜单、多选、通知中心等功能标为已完成。


## 继续还原：独立菜单、桌面集合与区域模糊（2026-10-09 晚间）

### 音量菜单

从镜像 `Volume.menu/Contents/Resources/zh_CN.lproj/MenuItem.strings` 解码确认“音量：”“输出设备：”“声音偏好设置…”；按 [Catalina 音量帮助](https://support.apple.com/en-gb/guide/mac-help/mchl1f98810b/10.15/mac/10.15) 实现菜单栏直接拖动。新菜单复用已经修复的真实 `wpctl` 写入队列，`pactl` JSON 枚举输出并切换默认 sink；切换前先完成旧设备写入，随后读取新设备自己的音量/静音状态。系统声音偏好启动实际 `pavucontrol`。

只在菜单打开时订阅音频事件；拖动或写入期间暂停设备枚举，合并外部音量事件为轻量读取，结束后刷新一次。旧代际回读不覆盖新菜单或新操作，菜单关闭后停止订阅。滑条、设备列表和其他菜单分别选择自己依赖的状态。没有借本次验收改变用户的真实音量或设备选择。另修复 controller 未持有命令服务器时 dispose 删除运行中 socket 的问题。

### 系统菜单

参照 [Catalina Apple 菜单项目](https://support.apple.com/en-gb/guide/mac-help/mchlp1130/10.15/mac/10.15) 调整项目顺序、分隔、13px 文字和26px行，并移除打开菜单时对全桌面的变暗。布局数值是 Realand 工程选择，尚无原生运行截图测量，不能称为 Apple 原值。

“关于本机”异步读取真实发行版、CPU、内存及当前 compositor 版本，不伪造 Mac 型号或 macOS 版本。“最近使用的项目”连接已持久保存的最近应用，支持真实打开和清除；文稿与服务器最近记录仍缺。“系统偏好设置”启动真实 Realand 设置；软件商店仅在确有安装时提供入口。

电源项查询 [systemd-logind 官方接口](https://raw.githubusercontent.com/systemd/systemd/main/man/org.freedesktop.login1.xml) 的 CanSuspend / CanReboot / CanPowerOff；只有 yes/challenge 且条件满足才启用，激活前重查。睡眠先等待 `swaylock --daemonize` 成功锁定，再提交 Suspend；无 locker 或锁定失败均不睡眠。锁屏用真实 swaylock，注销调用本 compositor Quit，重启/关机用真实 logind。注销、重启、关机先确认；demo/preview 不执行系统动作。验证只使用注入后端，没有实际触发这些电源/会话操作。

读取超时5秒，显式交互动作允许60秒授权；超时显示“未收到系统操作结果，请检查当前状态”，不暗示远端动作已被取消，不自动重试。菜单销毁取消未完成读取并关闭所属 bus。模态框获得键盘焦点，Esc 仅关闭当前模态框，外部面板切换会移除该菜单拥有的模态 route；小物理输出可滚动。最近应用子菜单支持方向键进入/返回，保持屏幕边缘可点击。强制退出仍禁用：CloseWindow 不能冒充可靠的应用进程终止。

### Vulkan 区域计算

每次保持原完整 capture、纹理大小、viewport、矩阵和 UV；从最终可见 region 反向计算每个 Dual-Kawase pass 所需采样范围并向外保守扩展，仅缩小 scissor。区域移动、扩大、离散或清空时重新规划，不复用过期的覆盖范围。整个区域路径继续使用原 fast path。

Intel Iris Xe 与 NVIDIA RTX2050 各238帧，区域优化与原 full-pass 输出逐字节一致；包括8种 transform、1/1.25/1.5/2缩放、crop/zoom、圆角/noise、缓存区域移动/扩大/离散/清空，另覆盖1pass offset0与31pass。两 GPU 共8项回归通过，并包含原 Dual-Kawase、framebuffer GLES 对照与真实 Wayland 背景模糊协议生产链；同步验证无诊断。

混合测试的 blur/clear 计算面积从13,666,072降至4,366,144，减少68.1%；1080p、24px条带、3passes/offset2示例从3,402,000降至131,040，减少96.15%。这是 shader/clear 工作面积，不能换算为整桌面帧时间或宣称消除偶发停顿。完整捕获带宽/纹理占用、其他 GLES 工作、系统 IO/换页和第三方应用阻塞仍需分别观测。

严格 Clippy、CPU 反推覆盖和5项原生键盘输入测试通过。新 release 已校验配置并保留上一版本/配置备份，只切换下次登录 launcher；当前 PID1324 保持运行，不中断用户应用。区域模糊及新的原生快捷键路由需重新登录后生效。


### 本批发布与验收记录

- 全套 Flutter 回归：**782/782 通过**；`flutter analyze --no-pub` 在 Shell 目录通过。
- Shell release 已构建、安装并仅重启 Shell 服务；服务 active/running，PID 256638，NRestarts=0，命令 socket 可连接。84个Dart/native/manifest源哈希与构建期间冻结记录一致。
- Dart发布代码 `lib/libapp.so` SHA256：`c6ee063ff39c104bd3fab8551165f9c53d4ace0637642ceb9a1ca06e5da2a5fc`；确认当前进程实际映射这个新库，并与新bundle一致。Runner二进制本批没有变化，不能仅凭runner哈希判断Dart代码更新。
- 新 compositor release SHA256：`d00c7dfa354667a060b917205daa989f70389df813fd9b9f936929a4cd01245b`；配置校验通过，个人配置哈希未变，旧launcher/配置有备份。
- 当前 compositor 仍为旧 PID1324，新区域模糊与 Super-A 原生路由仅在重新登录后启用。此轮没有强制退出当前桌面会话。
- UI预览是项目实际widget的渲染检查；双GPU对照是新旧算法图像一致性验证。两者都不是已完成真实Catalina逐像素/逐帧验收的证据。

安装后30秒采样未捕获慢日志，合成器CPU峰值约单核8%；当前仍是旧合成器，因此该采样不能代表新区域模糊在当前桌面的性能，也不能排除偶发停顿。构建期间的安装前采样已另标 under-build，不当作空闲基线。

## 本轮通知中心：镜像资源与实现边界

新增静态提取目录为 `work/notification-center-20261009/selected/`（2026-10-09 Codex 工作目录）。使用既有只读 Core.pkg 读取器选择 NotificationCenter.app 和 Finder 中文/英文资源；扫描 472,592 条 CPIO 后在已知缺失尾部停止，选取 790 个文件。没有运行 Apple 二进制。NotificationCenter `CFBundleIdentifier=com.apple.notificationcenterui`、build `845.40.3`；其 CodeResources 中616个带SHA256的普通资源全部匹配、无缺项。此校验不等于完整安装包或Apple CMS信任校验。

镜像中文字符串实际包含“今天”“通知”“没有新通知”“勿扰模式”“清除”。`Base.lproj/NCNotificationCenterWindowController.nib` 的内容/头部视图宽344点，分段控件归档frame为300×25；`NCDNDViewController.nib` 标明 NSSwitch，归档frame为64×24，所在行高44。Realand采用344点侧栏（输出更窄则收缩）与对应紧凑标签布局。NIB只能证明归档布局，不能证明运行时最终窗口宽、开关绘制边界、字体栅格、阴影或缩放。

[Apple Catalina 通知中心帮助](https://support.apple.com/en-gb/guide/mac-help/mchl2fb1258f/10.15/mac/10.15)证明今天/通知双视图、默认动作、逐条/按天清除及勿扰时继续收集历史。Realand侧栏贴输出右边，从菜单栏下缘延伸至底部，外侧点击层透明；进入220毫秒、退出180毫秒与easeOutCubic是工程值。退出期间通过独立notification surface状态保留原生展开，未借用Launchpad backdrop，反向重新打开延续当前动画；减少动态效果时立即切换。

今天视图只提供本机本地日期和可切换月份、选日、键盘箭头/PageUp/PageDown操作的月历；按月构造民用日期避免按24小时相加跨DST偏移，午夜更新仍跟随今天的选择。没有虚构天气、账户日程、股票或第三方widget。通知按本地日期分组，行操作携带捕获的通知对象，防止同ID替换后旧按钮操作新通知；仅成功动作关闭原版本面板，晚回调不关闭新面板。勿扰开关绑定现有Realand状态，当前服务没有主动声音/横幅提供者，不能称为系统级静音或Catalina的按时退出勿扰。

组件测试覆盖日历闰年/跨年/周起始日、午夜、标签键盘与重开保留、DND状态、默认/命名动作、旧通知对象、重复及晚回调、逐条/按天/全部清除、180×180小输出、Material保留、原生退出展开/反转且不触发Launchpad背景。实际widget亮/暗截图位于本轮 `previews/`，字体使用项目随包字体；它们是Realand渲染检查，不是Catalina实机截图对照。

## 本轮已有桌面的细修（2026-10-09）

用户明确要求继续修改已经存在的图标、标签、排列和右键菜单。macOS 分支保留原有文件服务、原镜像图标资源、用户保存的位置与多选/组拖能力。本轮把整格蓝框调整为图标区域的暗底和文字标签蓝底，指针使用箭头；右键菜单的悬停行改成全宽方角，补 Control 主键点按入口。选中色、图标48点及行高仍为工程实现，未取得 Catalina 运行截图测量，不能据此称逐像素一致。

参照 [Catalina 重命名帮助](https://support.apple.com/en-gb/guide/mac-help/mchlp1144/10.15/mac/10.15)，单选后 Return 开始标签内原位重命名，Return 提交、Esc 取消，文件默认仅选择最后一段扩展名前的名称，目录与点开头且无其他扩展名的名称全选。真实文件服务验证失败时保留编辑器供修改；编辑期间键盘输入不落入文件打开/回收快捷键。保持完整输入，交由既有文件服务校验，而不静默裁掉空格。

文件扫描可能先发布新路径、下一帧才完成重命名 Future：此间保留旧路径的选中记忆，再迁移到新路径。完成时只将旧路径位置迁移至新路径，合并当时最新布局，避免覆盖其他图标期间收到的位置变更。点击其他图标提交时保留新选中；服务、输出或风格切换使旧编辑回调失效。独立延迟/分帧夹具覆盖这些边界。跨原生应用失焦是否应提交仍待 Catalina 运行参考，未凭推测增加行为。

原生桌面有 base on-demand 与独立 exclusive popup 两个 layer；现有合成器在菜单点击时会清除 base 焦点记忆。新版仅在精确 namespace 为 realand-desktop / realand-desktop-interaction、同 Wayland client、弹层使用 Exclusive 且已存在 base 记忆时保留该记忆，不授予新焦点。弹层依然优先获得键盘，关闭后回到已有桌面焦点，其他客户/namespace/普通应用点击保持原清除行为。GTK 同时恢复 base Flutter child 焦点；这不替代 Wayland seat 修复。

客户端只读 Capabilities 请求探测 desktop-menu-focus-restore：旧 compositor 不支持该请求时，右键菜单重命名继续用原有弹窗，Return 仍为原位编辑。重新登录运行新版后，菜单也启用原位编辑；兼容分支不把当前旧会话中尚未具备的焦点恢复能力标为已启用。

背景菜单目前仍有 Realand 的新建文件/快捷方式扩展，未完成 Catalina 的显示简介、整理/排序、叠放、标签、快速查看或真实批量重命名；网格默认起点、自由排列、图标/文字尺寸与阴影仍需匹配尺度的真实镜像运行参考。本轮没有把现有三项菜单包装为功能已完整还原，也没有重置用户的桌面布局。

### 重复工作与通知生命周期

托盘 SVG 的 `File` 缓存键曾在每次 rebuild 新建。现在保留文件身份，用独立 iconRevision 在 NewIcon/NewAttentionIcon 或真实图标字段变化时失效；标题/状态更新不重读。隔离夹具中的30次元数据更新，将31次SVG编译降为1次、文件读取1次；同路径文件改写后增加revision仍能读出新像素（累计读取2次），不是把更新关掉。110项相关回归通过。

通知服务现在把动作与关闭信号按发生顺序单播给记录的发送方；先同步失效旧对象/计时器，再等待I/O，不让旧动作删除同ID替换的新通知。常驻通知在动作后保留，默认不计时但尊重显式正超时；critical不自动过期，transient到期不留历史。过期/主动关闭/用户清除/容量回收分别发送正确原因。原始动作key保留完整，不把截断字符串发回应用。真实私有dbus-daemon验证了接收者隔离，没有给用户会话发送测试通知。

变更按16毫秒合并；100条通知burst只产生1次服务更新，clear也1次，按天100条快照清除在controller只产生忙开始/结束两次更新。服务与控制器36项定向回归通过，之后纳入完整839项Flutter回归。它们证明减少了这些路径的重复工作，不能据此断言整桌面偶发停顿已经消失。

### 当前外壳发布

2026-10-09 22:01 安装并仅重启 Shell，PID280519，服务active/running、NRestarts=0；命令socket可连接，通知服务GetCapabilities返回body/actions/persistence。进程实际映射新的lib/libapp.so，SHA256为 `2d0813e60cfee681e2b99cfd059aba5997a081029c4e9fb085f2621e7b32b571`，与构建bundle一致并区别于上一版本。85个Dart/native/manifest源哈希构建前后无变更。839项完整Flutter回归通过，静态分析无问题，Linux release构建成功。

当前compositor仍PID1324，未退出用户应用。菜单原位重命名的原生焦点能力与区域模糊尚需重新登录后启用；在此之前保留菜单弹窗兼容路径。新compositor的最终构建、下次登录发布与采样记录另附后续条目。

### 本轮原生版本与性能采样收口

22:11 新 compositor release 构建完成，SHA256 `fd0e56bc242c141d83c674448b4f39f83935e7b33ea669c81a93466be1825766`。11项真实 layer-shell 回归通过（另1项原有微基准忽略），IPC capability wire 回归及严格 all-targets Clippy 通过；5600个冻结输入在构建前后不变，配置校验通过。launcher 已原子切到 `desktop-focus-20261009-221106-369717`，个人配置 SHA256 不变，保留旧版本。当前 PID1324 仍运行旧版本；能力探测在当前会话继续使用菜单重命名兼容路径。

22:11:16–46 的30秒采样捕获到 Shell 短时CPU峰值307.7%（多线程合计、单核100%尺度），新增4822次major fault、约231.8MiB read_bytes，集中于22:11:29–31；系统同段swap-in6.93MiB、swap-out0、IO PSI1.87s。合成器仅新增1次major fault，二者无慢日志。缺少同时刻的交互记录与调用栈，因此不能把峰值归因到具体解码路径、内存泄漏或系统换出，也不能断言偶发停顿已消除。该样本不当作空闲基线。既有托盘缓存、通知合并及下次登录启用的区域模糊各有独立验证，但不能替代整桌面的持续性能验收。


## 配置中心对齐 Catalina 系统偏好设置（2026-10-09 23:17）

本轮修改原生 `realand-settings`，沿用既有 KDL 草稿、无损保存、导入文件校验、异步 IPC、显示预览与回滚后端。旧深色侧栏改为浅灰偏好设置总览、SF／苹方字体、分类图标、前进后退、全部显示和搜索；真实可用的八个面板分组显示，保留 Realand 的窗口布局、更多设置与配置文件入口。红黄绿按钮、标题拖动和右下调整尺寸有真实窗口操作；首次启动只对自身 PID 对应的窗口执行一次居中悬浮，后续不重置用户布局。

从 Catalina 10.15.7／19H2 镜像读取五个原偏好面板图标、NIB 与中文资源；本轮选取727项资源，其中565个带 CodeResources SHA256 的文件全部匹配，无不符。此结果只验证所选资源，不等于安装镜像完整或 CMS 信任校验。主 System Preferences.app 内容未恢复、镜像尾部缺失，尚无原系统运行画面；总览尺寸、工具栏间距、egui 控件绘制仍为工程适配，不能称 AppKit／CoreText 逐像素一致。原应用 PrefApp.icns 不可用，桌面入口使用标准 preferences-system 图标；未拿侧栏图标冒充。图标在 egui Context 中缓存，正常重绘不反复解码上传。

通用依归档采用单页，Realand 高级动效折叠；调度中心左下“触发角…”打开四角示意，仅配置已存在的真实动作；键盘独立显示按键重复与重复前延迟双滑块；触控板使用“光标与点按／滚动缩放／更多手势”三页。显示器使用真实输出，多屏时提供排列图，未增加无真实提供者的亮度、色彩、账户、硬件或手势影片空壳。

键盘重复及触控板速度、轻点与自然滚动读合并后的有效配置；修改继承的触控板组时保留完整原组、注释及未修改数值精度，覆盖冲突保留草稿。切页与保存在本帧输入落入表单后执行；输入法提交与同帧鼠标导航通过延期指针事件保护，搜索候选确认不误切页。页面与模态状态变化后立即请求重绘，避免等待 IPC 定时刷新。

最终221项设置应用测试通过，严格 all-targets Clippy 无警告，优化 release 构建成功；71个冻结构建输入前后不变。包含81种页面／宽度／DPI组合、搜索与历史、草稿、输入法、真实滑块到 KDL、范围外数值保留、触发角弹窗与首次自身窗口几何回归。测试不写用户配置或向当前桌面注入测试 IPC。

23:17 已原子发布设置应用，SHA256 `e273c585008044444d1667dc0da2a951eacf0f4fa232f32e562e35127a7c4468`，旧 launcher 与 desktop 入口有备份。实际新窗口 PID46277／ID20 为700×580居中悬浮；总览截图已保存，进程随后以0退出且无错误输出。个人配置 SHA256 保持 `31ba8304b6921d66767b23f8244761e2e60ee083a1d8d6ecadbdd7b87343b6ed`。本次安装未重启 compositor 或 Shell。

最终检查时，桌面会话已于22:14启动，当前 compositor PID1256 实际执行此前准备的 `desktop-focus-20261009-221106-369717` 版本，Shell PID1321 执行22:01发布版本。因此前文“需下次登录”的状态是当时记录，当前会话已经加载该原生更新；这不代表已经证明整桌面偶发停顿消失。

安装收据、完整日志、源冻结及最终验收记录保存在本轮 `work/settings-macos-20261009/`。可交付截图在本轮 `outputs/realand-settings-home.png`、`realand-settings-keyboard.png`、`realand-settings-trackpad.png`；总览为最终重绘修复版本，键盘和触控板为同一界面源码、该修复前的已安装版本。

## 控制中心、面板位置与壁纸加载（2026-10-10 00:15）

菜单栏控制中心改为 Big Sur 风格紧凑卡片、显示器与声音胶囊滑条、独立静音和真实输出设备展开。Catalina 10.15.7 镜像没有 Big Sur 的整合控制中心；参考 [Apple Big Sur 官方发布画面](https://www.apple.com/newsroom/2020/11/macos-big-sur-is-here/)，不将 Catalina Displays.menu 的 AirPlay 菜单冒充亮度布局证据。当前仍是 Catalina 桌面／系统偏好设置加 Big Sur 控制中心扩展，320逻辑点宽等尺寸与160ms过渡是工程参数，未完成单一原版系统的逐像素验收。无后端的 AirDrop、屏幕镜像、键盘背光和播放中没有添加空壳；勿扰仅抑制通知计数，外观切换仅影响 Shell。

根据用户实机反馈修正位置：控制中心沿菜单栏右边缘保留12逻辑点边距，避免停在时钟左侧；长桌面偏好页改为宽620、最高720逻辑点的居中面板，返回恢复320宽控制中心。跟随菜单栏高度、宽度、输出大小和DPI；窄屏自动收缩，外部点击和Esc关闭，不压暗桌面。当前1920×1200、scale2、栏高32实机控制中心为(1588,38,320,384)，桌面偏好为(650,240,620,720)。这些实机几何不等于 Apple 原版尺寸。

实机检查曾发现只有菜单栏模糊、控制中心背景仍能读出底层文字。新增 setPanelGlassRegion：菜单栏与面板圆角区域取真实并集，不将中间桌面包进整块模糊。圆角先按原矩形计算再裁剪，最后整数区域相同不重复提交；关闭、Material、启动台和窗口销毁清理。Dart 将期望面板几何与 setExpanded 放入同一原生串行队列，修复先上报被丢弃和同位置快速重开不重报；坐标观察置于动画／RepaintBoundary之外，避免留下4px入场偏移或纯位置变化的旧区域。五项回归覆盖慢展开、快关重开、取消、仅平移和动画去重；纯Cairo四组区域测试及生产C++语法检查通过，最终Linux release也编译通过。实机控制中心和桌面偏好的背景已呈模糊，而面板外文字保持清晰。

壁纸加载的确定问题是 macOS 外层选择器不订阅图库状态，WallpaperPanel也没有自身订阅：异步扫描返回后列表／进度／选中／错误不及时刷新。新增局部selector，与壁纸无关的30帧通知不重建缩略图；available从绘制期反复statSync改为启动／刷新／应用时异步检查并缓存。并发扫描合并，后续刷新仍重查mtime／size；同路径文件覆写仍失效。重复选择健康且未修改的当前图片复用现有swaybg，首次／变更文件仍解码原图；迟到启动restore不覆盖新选择，原子持久保存与失败保留旧背景继续通过回归。

图库扫描原来已经异步，缩略图原来已经懒构建、320px解码目标及mtime／size缓存，本轮没有把这些旧实现算作新优化。壁纸专项115项通过，真实界面显示57张目录图片并自动完成异步状态刷新，未手动换图。首次大图／PNG缩略图及swaybg原图解码仍有实际耗时，未测量首次应用的加速比例，也未降低壁纸原图画质。通知测试夹具被本机壁纸可用性异步更新影响时，改为稳定的空壁纸变更流，保留原通知零重复更新断言。

最终866项Flutter回归全部通过，flutter analyze --fatal-infos无问题，Linux优化release构建成功；214个Dart／测试／native／资源／manifest输入构建前后无变动。00:11原子安装并仅重启Shell，新版路径为 `20261010-001105-488509`，PID78858，active/running、NRestarts=0。实际runner SHA256 `60331b979fb0b526123530793e770bae9fc5794bbafc1f73845cde433c10a554`，进程实际映射的libapp.so SHA256 `224bc9416d7fe7bb38b1846b08b65510a9d128abdcfa8e16e34fbb049c680251`，均与发布bundle一致。compositor PID1256及其执行路径不变；安装前后个人KDL、Shell偏好与壁纸选择文件哈希均不变。实机验证只打开／返回／滚动自身面板，未修改音量、亮度、无线或电源模式；结束后关闭面板。

发布后查看控制中心／桌面偏好的30.02秒只读样本中，Shell多线程合计CPU峰值5.99%（单核100%尺度）、重大缺页增量0、RSS367861760→367968256字节、VmSwap0→0，无匹配慢日志。该样本在首次打开壁纸图库之前，不是图库冷解码基准；短时状态稳定也不能证明用户描述的整桌面偶发停顿已经消失，仍缺同时交互轨迹和调用栈的根因闭环。

完整剩余项已按七个领域更新于 [macos-remaining-work.md](macos-remaining-work.md)，剔除已有独立最小化条目、废纸篓清空等过时缺项。此次收据、日志、源冻结和验收JSON位于本轮 `work/control-center-redesign-20261009/`，最终实机截图位于 `outputs/realand-control-center-live.png`、`outputs/realand-desktop-preferences-live.png`、`outputs/realand-wallpaper-gallery-live.png`。


## 2026-10-10：桌面工具、媒体控制与缩略图缓存

本批补齐真实能力，仍不将功能测试或组件截图当作 Catalina/AppKit 的像素一致性证明。

- 桌面右键与快捷键接入真实“显示简介”、文本/图片“快速查看”。属性来自受限直接子项的 lstat；不伪造创建日期。预览可取消，不执行快捷方式、不自动跟随链接；打开继续走原信任链，并保持单次原生捕获。文本最多 256 KiB，图片最多 8 MiB/16 MP、仅首帧最高 1600 px；PDF/影音/Office、AppKit 多窗口/汇总简介仍缺。
- “整理”“整理方式”“排序方式”“整理所选内容”接入真实布局。四项名称/种类/修改日期/大小；一次整理与持续排序分开，选择“无”冻结当前位置后恢复手动拖动。按输出持久、兼容旧位置文件。种类不是完整 UTI，目录大小不递归、不冒充创建/添加日期。镜像 Finder 中文菜单资源只证明名称，不证明排序算法。
- macOS 菜单改为单 PopupRoute 级联，父子均可命中；父级悬停换子项、Right 后立即 Left、Esc/外点单次关闭、屏幕边缘左开与小屏滚动均有回归。Material 原菜单保留。
- 控制中心 MPRIS 使用真实 D-Bus owner/connectionId、实时 Can*、事件订阅，无位置轮询或自动启动播放器；无播放器不显示示例卡。窄屏按钮自动折行；封面、进度拖动、AirPlay/投送仍未提供。宿主播放器未被测试控制，后端测试使用私有 DBusServer。
- 壁纸列表增加持久 320 px PNG 缓存，源路径/大小/mtime 失效，最多两路 NI15/idle IO 单线程生成、128 MiB/512 项预算。未启动离屏任务跳过；已启动任务可继续至完成或 20 秒超时，关闭面板停止自有进程。损坏回源/负缓存避免重试风暴；应用壁纸仍用原图。当前真实列表生成 36 项 ready 缓存，总 PNG 3,939,411 字节；这是落盘证据，不是首次原图应用的加速比例。
- journald MESSAGE 为字节数组时，诊断脚本此前漏掉所有慢调用，现严格解码后去 ANSI；9 项 Python 回归包括真实 124.9/94.6 ms client-dispatch 记录。历史尖峰仍需分段定位；快照 GPU 同步失败永久重试风险已记录，未证明其触发该尖峰、未在本批修改同步资源生命周期。

验证：`flutter analyze --no-pub` 无问题；全量 **954 项 Flutter 测试通过**（上批 866），Linux release 成功，**248 个冻结输入无变化**。桌面快捷键合成器补充 `i/I/y/Y` 的 exact SUPER 放行，3 项 Rust 回归通过，原焦点/身份/锁屏/模态守卫与按键配对保持；旧运行合成器尚未含新规则，需要下次加载新合成器才能验收与个人 Mod+I 绑定的冲突。

Shell 已安装并重启至 `20261010-010117-594540`，PID `103068`，NRestarts=0；runner SHA256 `60331b979fb0b526123530793e770bae9fc5794bbafc1f73845cde433c10a554`，实际映射 libapp SHA256 `614c139ca00372bedd1220c3bb670240b18fcd53fb9cdb3c3f9f619c444a7173`。合成器 PID1256 保持原 release，未重启用户会话。个人 KDL 哈希未变化；偏好、壁纸选择与桌面布局哈希在并发会话操作中发生变化，仅凭哈希不能归因，未回滚当前选择。

生产组件截图使用独立真实文件和字体加载：简介（本地记录：`$HOME/Documents/Codex/2026-10-09/zh/outputs/realand-desktop-info-widget.png`）、快速查看（本地记录：`$HOME/Documents/Codex/2026-10-09/zh/outputs/realand-desktop-quick-look-widget.png`）、级联排序（本地记录：`$HOME/Documents/Codex/2026-10-09/zh/outputs/realand-desktop-sort-widget.png`）。这些是 Flutter widget 截图；当前控制中心另有 native capture。没有把受同时桌面操作影响的临时预览/文件管理器画面当验收截图。

30 秒只读样本采自本批安装前的 Shell78858：1229 次呈现、0 late，合成器 render rolling max1.161ms，无两进程大缺页或系统换页；它只是空闲基线，不能验证本批缓存优化、完整输入到显示时延或排除偶发停顿。

完整日志、源冻结、真实缓存和映射验证：`$HOME/Documents/Codex/2026-10-09/zh/work/macos-desktop-tools-20261010/`。当前剩余项以 [持续维护清单](macos-remaining-work.md) 为准，旧章节中简介、整理排序与 MPRIS 缺失的结论已被本批替代。

合成器快捷键发布补充：release 构建成功（3m29s），已原子安装到 `$HOME/.local/lib/realand/releases/desktop-tools-20261010-011245-231470`，SHA256 `e820e7424a9e4f96e236e085b50860957efac52de0c1c631309b89fac2ecd9de`；旧启动链接已备份。`/proc/1256/exe` 仍指向旧 release，未重启；下一次会话加载新版后需补物理键盘验收。
