# Catalina Dock 与 Realand 的功能差异

更新：2026-10-09。参考为用户提供的 Catalina 10.15.7 / 19H2 镜像中的真实资源、菜单模板和偏好绑定，以及 Apple 对应版本帮助。Realand 已补齐下表注明的功能；材质、动画和交互参数仍是项目实现，尚未完成运行中的 Catalina 逐帧测量，不能称为完整 1:1 复刻。

## 镜像证据可以复核什么

原镜像位于 `$HOME/项目/macOS Catalina 10.15.7 19H2.dmg`。已恢复并核对选定资源；镜像尾部缺失，资源与 bundle 的 CodeResources 哈希匹配不代表整镜像完整或 Apple 证书信任已经验证。

| 原始证据 | 能确认的内容 | 追溯 |
| --- | --- | --- |
| `Dock.app/Contents/Resources/DockMenus.plist` | 19 种有序菜单模板，包含应用、目录、窗口、Trash、卷等不同类型；原 command 是 Dock 内部编号，不能当作 Realand IPC | SHA-256 `1a4a7bf8af64006dd101fb9d01dc0ea803dbdbccfeaa065c58b29437d42b5314` |
| `Dock.app/Contents/Resources/zh_CN.lproj/DockMenus.strings` | 原中文菜单名：神奇/缩放、Folder/Stack、Fan/Grid/List/Automatic、排序、登录打开等 | SHA-256 `647a5ab0d83c034682b06ab0c8d6850a55e651986de8d8f7e96584d156eece5a` |
| `Dock.prefPane/Contents/Resources/Base.lproj/PrefPane.nib` | `self.size`、`magnificationSize`、`showRecents`、`minimizeIntoApp`、`minimizeEffect` 等独立绑定 | SHA-256 `a8ce59fe7da4b9497b76efaa7f5643d3bb9dd77b5562f3778974f1e1980eab5e` |
| `Dock.app/Contents/Resources/default.plist`、`com.apple.dockfixup.plist` | 有序应用模板、独立 Downloads directory tile、Mission Control 的真实应用身份；不是登录用户偏好 | 原值、哈希见本轮镜像清单 |
| Core.pkg 的 CoreTypes / 应用 ICNS | Realand 使用的 Finder、Downloads、空/满 Trash、Launchpad 等原图来源；第三方 Linux 应用保持自身图标 | 资产来源与转换记录（本地记录：`$HOME/realand/clarion-shell/assets/macos/provenance.json`）、运行资产哈希（本地记录：`$HOME/realand/clarion-shell/assets/macos/SHA256SUMS`） |

完整绝对路径、115 个选定文件的哈希、菜单 flags、44 条 NIB 绑定和 PNG 尺寸保存在镜像核对清单（本地记录：`$HOME/Documents/Codex/2026-10-06/jia/work/dock-detail-completion-20261009/image-evidence.json`）与原始比较报告（本地记录：`$HOME/Documents/Codex/2026-10-06/jia/work/dock-detail-completion-20261009/mirror-comparison.md`）。Dock bundle 的 256px 图与运行资产中从 CoreTypes ICNS 恢复的 1024px 图是不同来源，不应混写为同一文件。

资源和 NIB 不能给出私有动画曲线、时间、模糊方程、首次登录默认值或运行中用户历史。NIB 滑块的 0–1 设计归档不能推出 Realand 的 32–64px 尺寸范围。Mission Control 属于应用区，但不在镜像 default.plist 的默认固定应用列表；Realand 的独立工具按钮是项目选择。

## 已完成与仍有差距

| 项目 | Catalina 可证实的行为 | Realand 当前实现与边界 |
| --- | --- | --- |
| 外壳与独立配置 | Dock 有独立偏好 | Material/macOS 可切换，分别保留栏位置和 macOS 偏好；切换不重启合成器 |
| 图标及字体 | 原 ICNS、Apple 标识及系统字体可追溯 | 对应系统角色使用恢复的原图和 SFNS/PingFang；不是用 macOS 图标覆盖所有第三方应用 |
| 大小、放大、三位置 | 基础大小与放大独立；底/左/右 | 已持久化并同步原生绘制、输入和保留区。尺寸、倍率、放大函数是项目参数 |
| 应用、最近、其他分区 | 应用与文件/文件夹分别位于两条分隔线的不同侧 | 固定/运行应用、独立最近分区、Downloads/Trash 分区；Mission Control 已回到应用区 |
| 最近应用 | 最多三个尚未包含在 Dock 的最近应用 | 真实启动/焦点历史、去重、有界持久化、独立开关；当前运行应用已在主应用区，所以不重复显示在最近区。排除 running 是去重布局策略，公开帮助未规定最近区只能显示已退出应用 |
| 运行指示点、启动弹跳 | 应用打开时有指示点，启动可弹跳 | 指示点来自真实窗口，启动与聚焦分开；尚缺无窗口应用的独立生命周期、请求注意弹跳和真实通知徽章 |
| 下载位置与数据 | Downloads 是独立目录叠放 | 解析 XDG DOWNLOAD、读取真实一层目录、监听变化及删除/重建恢复；限制枚举及并行 stat，超额提示打开文件夹查看全部 |
| 关闭态 Folder / Stack | 图标显示与展开布局为不同设置 | 原 Downloads 图标或从已枚举项目中按真实排序取前几项叠放；独立持久化，空目录保留文件夹图标。文件项目使用系统类型图标，尚非 Finder Quick Look 内容缩略图 |
| 展开 Fan / Grid / List / Automatic | 目录菜单含四种独立布局选项 | 显式 Fan 在底部足够空间时对大量目录仍生效：按高度显示有界文件图标与打开目录的终点；自动模式才按数量选择 Fan/Grid，侧边及过窄/过矮回退 Grid。自由图标和名称胶囊沿弯曲路径展开、反向收起；键盘只导航实际显示的文件与“显示更多”。最多八文件、曲线和 320/220 ms 是 Realand 参数；详见[扇形说明](macos-downloads-fan.md) |
| 排序 | 名称、种类、修改/创建/添加日期为独立选项 | 名称、种类和真实 mtime；当前 Linux 后端没有可靠创建/添加日期，相关菜单禁用，旧不可用配置按名称呈现。没有将 POSIX ctime 或首次扫描时间冒充这些日期 |
| 下载快捷菜单 | 原目录模板有排序、显示、布局、Options 与打开目录 | 叠放面板提供三组设置与“在文件管理器中打开”；右键目前打开同一叠放面板，尚非完整的原分层目录快捷菜单 |
| 应用快捷菜单 | `fileapp` / `process` 模板按状态提供命令 | 紧凑图标锚定菜单，真实窗口选择、最小化/恢复、在程序坞中保留/从程序坞中移除（已对齐 Catalina zh_CN 原串）、显示安装入口所在目录、关闭所有窗口；边缘和窄屏约束、键盘及过期锚点保护已接入 |
| 退出、隐藏、登录打开 | 与窗口关闭和 pin 操作不同 | 未实现完整 Quit、Hide/Hide Others、Force Quit、Open at Login、工作区分配。当前明确写“关闭所有窗口”，不假称已经终止后台应用 |
| 应用自定义菜单、最近文稿 | 应用可以导出独立 Dock NSMenu；SHOW_RECENTS 是应用最近项目 | 尚未接 Linux `.desktop` 自定义动作及独立应用 Dock 菜单/最近文稿。全局主菜单、托盘菜单、最近应用开关都不能替代它 |
| 自动隐藏 | 指针到对应边缘显示 | 已支持边缘揭示及菜单/Downloads 打开时保持可见；缺少完整拖动锁定和隐藏状态的最小化终点。隐藏/揭示未稳定期间仍清理图标目标 |
| Genie / Scale | 原偏好与模板有两种最小化效果 | compositor 已有方向明确的 Genie 和等比 Scale，支持反向连续交接、隐私遮挡和减少动态；曲线及 500/350ms 是项目实现。核心更新在下次登录使用新进程后生效，详见 [效果说明](macos-minimize-effects.md) |
| 最小化到应用 / 独立窗口项 | `minimizeIntoApp` 与 `minwindow` 是独立能力 | 当前最小化到应用图标，真实状态可恢复；未实现独立窗口缩略图区域，也未增加无效的模式开关 |
| Trash | OPEN 与 EMPTY_TRASH 分开，原图有空/满状态 | GIO 查询真实虚拟 Trash，监听本地变化、轮询其他卷，切换原始空/满图标并真实打开；已实现废纸篓快捷菜单（“打开”与“清倒废纸篓”，空态禁用，有内容时调用 `gio trash --empty` 真实清倒） |
| 拖放、分隔线操作 | 拖动重排、拖出只移除引用、分隔线拖动尺寸和右键设置 | 已有固定应用长按排序；分隔线鼠标光标变化（resizeUpDown / resizeLeftRight）、实时拖拽动态调节 Dock 大小（32.0–64.0px）以及分隔线快捷菜单（启用/关闭隐藏、启用/关闭放大、置于屏幕上的位置、最小化效果、程序坞偏好设置…）均已实现。未完成任意文件/文件夹拖入、拖出移除、文件拖到应用打开 |
| 材质、提示、响应细节 | 存在 light/dark/AX 背景、阴影、箭头、label 资源 | 原生模糊与圆角玻璃区域已接通；Dock 背景、弹跳、放大、隐藏、菜单/扇状参数仍需运行参考测量。原纹理存在不证明九宫格或 Blur 参数已还原 |
| 多输出与输入 | 每屏行为需实际验证 | 原生输出变更与锚点 epoch、逻辑坐标、透明区域穿透已有保护；跨输出最小化目标的 owner/条件清理仍缺失，其他输出的无效注册可能清除正确目标 |
| 分数缩放与尺寸同步 | 客户端收到的逻辑输出尺寸应与合成器状态一致 | 本轮原生测试发现 IPC 截断与 xdg-output 四舍五入不一致，现已统一协议整数尺寸；Winit 调整大小也重新计算缩放、方向和位置，保留物理模式尺寸。浮点布局尺寸保持原计算方式 |

更多模板如 `volume`、`smartfolder`、`url`、`file`、`folder`、`spacer`、`docklet`、`continuity`、`screensharingstack`、Finder 专属重启仍未实现完整对应后端；模板存在不代表本轮功能已经具备。`SHOW_RECENTS`（应用最近项目）与 `self.showRecents`（Dock 最近应用）尤其不能合并。

公开行为依据：[Catalina Dock 使用帮助](https://support.apple.com/en-gb/guide/mac-help/dock-mh35859/10.15/mac/10.15)、[Catalina Dock 偏好](https://support.apple.com/en-gb/guide/mac-help/mchlp1119/10.15/mac/10.15)、[AppKit applicationDockMenu:](https://developer.apple.com/documentation/appkit/nsapplicationdelegate/applicationdockmenu(_:)?language=objc)。这些支持操作与选项语义，不提供苹果私有插值参数。

## 使用与后续实现入口

在控制中心的 macOS 外观配置中调整最近应用、下载图标和展开视图；下载面板的选项菜单提供排序。隐藏 Downloads 只隐藏入口并保留设置。两套外壳共同遵守“减少动态”，最小化/恢复和键盘选择产生的自动滚动可直接完成。

生产接口分别位于 [DockFolderService](../clarion-shell/lib/services/dock_folder_service.dart)、[Downloads 面板与关闭态图标](../clarion-shell/lib/ui/mac_downloads_stack_panel.dart)、[Dock 布局](../clarion-shell/lib/ui/mac_dock_view.dart)、[应用菜单](../clarion-shell/lib/ui/mac_dock_app_menu.dart)、[锚点几何](../clarion-shell/lib/ui/mac_dock_panel_geometry.dart)、[控制器与历史](../clarion-shell/lib/state/shell_controller.dart)、[Trash 状态](../clarion-shell/lib/services/trash_state_service.dart)。偏好保存于 `preferences.json`，新增 enum 未知值回退，旧配置可读。

下一步先解决目标 owner/输出 epoch 和隐藏终点，再增加无剪贴板/截图界面副作用的窗口缩略图协议。独立窗口 tile 必须在 MinimizeWindow 前准备真实目标，销毁、锁屏、输出迁移及隐私变化时作废；普通 ScreenshotWindow 动作不能冒充该通道。之后才适合提供“最小化到应用图标/独立窗口”选择，并继续补齐真实应用生命周期与按能力生成的菜单。

对应回归在 `dock_details_test.dart`、`dock_folder_service_test.dart`、`mac_downloads_stack_panel_test.dart` 和 macOS 偏好测试；源码实现、自动测试与真实运行像素验收是不同层次。最终构建/回归结果以本轮主任务验证记录为准，多输出物理命中和 Catalina 逐帧一致性尚不能由组件测试替代。

当前原生检查采用 `mac_shell_configuration_native_test.dart` 与 `mac_dock_details_native_test.dart`；早期 `dual_shell_native_test.dart` 的旧 windowState 字段和固定 84px 断言仍需迁移，不能据其名称宣称当前全部功能已覆盖。


## 放大误触修复（2026-10-09）

Catalina 官方 Dock 偏好说明将“指针位于图标上时放大”和“到屏幕边缘显示隐藏的 Dock”分开描述。镜像 Dock 符号表导入 CGSAddTrackingRegion 等跟踪接口，偏好 NIB 分别绑定 magnification、magnificationSize 和 autoHide；这些证据不提供 Apple 的准确触发矩形或延迟。

修复前，Realand 把最大放大及弹跳包络提前作为输入区域。默认 48 px 图标的背景最内侧距屏幕边缘 64 px，输入区却覆盖 100 px，额外 36 px 的透明带会抢走窗口鼠标操作并触发放大。现在保持原绘制画布，静止时只接收可见 Dock 范围；每帧布局后根据实际图标变换和滚动裁剪更新输入区域，放大后仍可点击，离开后随动画收缩。Flutter 预览也用真实图标与背景区域做命中检查，避免透明画布拦截底层控件。原生输入区使用背景与各可见图标矩形的并集，避免单个图标放大后，把相邻图标上方空白也包含进去；该实现不声称复刻了 Apple 的私有区域算法。

新增回归验证底部、左侧、右侧 Dock 的透明区穿透、图标放大后边缘点击、退出复位和自动隐藏边缘唤出；另保留面板关闭后必须真实移动才重新放大的保护。
