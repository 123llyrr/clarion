# Clarion Shell

Clarion 的 Flutter 桌面外壳，提供可切换的 Material 与 macOS 风格界面，集成顶栏、Dock、
启动器、控制中心、通知与桌面操作。Linux runner 使用 GTK3 与 Wayland layer-shell，
通过 Clarion IPC 读取窗口、工作区和输出状态。

## Vulkan 合成与液态玻璃

Clarion 合成器的原生 Vulkan 后端负责桌面合成、背景模糊、液态玻璃与窗口动效。
Shell 提交表面、可见区域和交互请求，合成器统一处理效果、损伤与呈现调度；
Flutter 的界面绘制与合成器的 Vulkan 后端分别管理。

在 `~/.config/clarion/config.kdl` 顶层选择后端，并在下次登录 Clarion 时启用：

```kdl
renderer "vulkan"
```

GLES 兼容路径仍可选用。面板模糊和液态玻璃还需相应的图层规则；区域更新、清理和
协议说明见 [`原生 Shell 集成`](../docs/native-shell.md)。

## 构建

需要官方 Flutter stable SDK，以及 `clang`、`cmake`、`ninja`、`pkg-config`、
GTK3 开发文件、`gtk-layer-shell-0 >= 0.6` 和 `wayland-client >= 1.20`。
本次验证使用独立安装的
Flutter **3.47.6** / Dart **3.13.5**：

```bash
cd clarion
scripts/build-shell.sh --release
```

需要 Flutter 3.47.6 或更新版本（Dart 3.13+），以支持桌面名称提示的边界定位。
构建工具默认寻找 `~/.cache/clarion/toolchains/flutter`。可单次指定 SDK，
不需要改全局 `PATH`：

```bash
CLARION_FLUTTER_SDK=/path/to/flutter scripts/build-shell.sh --debug
```

`pubspec.lock` 固定 Dart 依赖，已有锁文件时构建会检查锁文件。产物是可搬移的
完整 bundle，例如 `clarion-shell/build/linux/x64/release/bundle/`；分发时必须
保留其中的可执行文件、`lib/` 和 `data/`，不能只复制可执行文件。

SDK 下载来源与校验信息以[官方发行目录](https://storage.googleapis.com/flutter_infra_release/releases/releases_linux.json)
为准；[官方 Linux 构建文档](https://docs.flutter.dev/platform-integration/linux/building)
说明了系统工具依赖。

## 运行与预览

```bash
# 普通 1200×800 窗口；不会占据桌面的顶部预留区。
scripts/run-shell.sh --demo

# 对实际 IPC 进行预览，通知服务与托盘 watcher 保持关闭。
scripts/run-shell.sh --preview

# macOS 风格的普通窗口预览，不保存这次样式覆盖。
scripts/run-shell.sh --preview --shell-style=macos

# Wayland layer-shell 模式，选择 GTK 第 0 个输出。
scripts/run-shell.sh --monitor=0 --output-name=eDP-1
```

`--debug` / `--profile` 作为脚本第一个参数选择相应 bundle。`--monitor=N` 和
`--monitor N` 都支持零起始的 GTK 输出索引；`--output-name=NAME` 应传入合成器
IPC 中的真实输出名称，没有名称时 Dart 使用当前焦点输出。启动脚本保留参数
边界，不将程序参数拼成 shell 命令，也不停止当前桌面外壳。

普通预览兼容 X11/Wayland。正式桌面模式要求支持 layer-shell 的 Wayland
合成器。

## 两套外壳

Material 模式在“控制中心 → 外观 → 界面风格”选择 **Material / macOS**；macOS 模式在“控制中心 → 桌面偏好设置… → 外观 → 界面风格”选择，成功切换后自动保存。
旧配置默认 Material；Material 的顶部/底部栏位置单独保留，切回来时恢复。
macOS 模式默认使用 22 px 顶部菜单栏，菜单栏高度可配置；Dock 可位于底部、左侧或右侧。
Dock 桌面预留为配置图标尺寸加 16 px（默认 48 + 16 = 64 px），自动隐藏时为零；
桌面文件、分页及名称提示跟随对应边缘避让。切换只更新外壳表面，不重启合成器、应用或工作区。

全局菜单读取前台应用真实导出的 GTK GMenu/GAction 或 canonical D-Bus 菜单。
合成器通过 GTK 和 KDE/Qt Wayland 协议把菜单地址绑定到具体窗口，不把窗口 ID
猜作 X11 ID。macOS 模式先绘制界面，同时准备菜单注册器；会话应用自启动、切换样式及启动应用前均准备注册器。
部分 Qt 应用切回 Material 后仍隐藏本地菜单；栏上的“应用菜单”按钮保留这些
已导出的菜单，Material 不再声明全局菜单主机。

传统 GTK 3 应用可使用单独安装的 appmenu GTK 模块。
`scripts/install-gtk-appmenu.py --source <staged-install>` 以用户权限备份、安装模块、
私有解析库、许可证、完整源码归档和 schema；不修改个人 dconf。
完整安装位于 `~/.local/lib/clarion-appmenu/current` 时，`clarion --session` 为新应用
添加模块环境并在会话退出时恢复原值。更多构建、来源及兼容说明见
[`docs/native-shell.md`](../docs/native-shell.md)。

更新合成器后需要下次重新登录才能启用新菜单协议；已有应用可能缓存了无注册器
的状态，GTK 模块也只能在新进程里加载。应用未导出菜单或主动移除了菜单时，
外壳保留系统菜单，不创建无法执行的“文件／编辑”，也不将托盘菜单当作应用菜单。

两条表面用同一个 Flutter engine 与 `ShellController`，共用窗口、通知、托盘、
电源和设备服务。Dock 点击已运行应用时聚焦现有窗口；紧凑右键菜单靠近真实图标，
支持窗口选择、最小化/恢复、固定/移除、显示应用入口所在目录及“关闭所有窗口”。
后者是普通窗口关闭，不声称终止应用后台进程。菜单位置适配三条边及窄屏，输出变化会使迟到锚点失效。
“控制中心 → 桌面偏好设置… → 外观 → macOS → 最小化窗口时使用”可选“神奇效果／缩放效果”。
神奇效果沿 Dock 所在边收口，缩放效果保留窗口比例；减少动态时立即完成。
效果和方向由合成器执行，更新合成器后在下次登录生效。Dock 右键窗口条目可最小化或恢复。
启动器里右键已安装应用也可直接固定到 Dock，无需先启动。默认固定项来自实际安装
应用，明确取消全部固定项会保持为空。无法匹配的运行窗口仍可聚焦，但不能固定为
无法重新启动的项目。

应用、最多三个最近应用、Downloads/Trash 是独立分区；Mission Control 位于应用区。
最近历史由真实启动/焦点更新并保存，排除已在固定/运行区展示的项目；关闭最近应用开关保留历史。
Downloads 解析 XDG 目录并监听真实文件变化，关闭态可选文件夹或内容叠放，展开可选自动、扇状、网格或列表。
自动模式在底部少量项目、空间足够时使用扇形；明确选择扇形时，大目录显示可容纳的文件和“更多”入口。
扇形沿真实 Dock 图标位置和当前尺寸展开，侧边或空间不足时安全使用网格；键盘、完整名称提示、打开文件与原目录均可用。
排序支持名称、种类及真实修改日期；缺少可靠添加/创建日期时相应选项禁用，不以 ctime 代替。
废纸篓根据 GIO 真实空/满状态切换原图，菜单可真实清倒废纸篓；卷弹出和完整应用退出等能力尚未补齐。
完整来源、已实现与缺项见 [`Dock 差异矩阵`](../docs/macos-dock-detail-comparison.md)。
启动台的分页、搜索返回、文件夹内外拖放与取消规则见 [`Catalina 交互对照`](../docs/macos-launchpad-interactions.md)。

Dock 透明绘制厚度按配置图标尺寸、最大放大倍率与反馈空间计算，最低 120 px；
不因指针移动调整原生表面尺寸。屏幕较窄时，基础图标从配置值临时缩小到最低 24 px，保留用户设置；
应用更多时沿 Dock 方向平滑滚动，减少动态时立即完成。
输入范围跟随静止 Dock 与每帧实际可见图标；放大、弹跳尚未占用的透明绘制空间
以及提示区域穿透，不提前拦截窗口边角。放大后的图标仍可点击；离开后范围随动画收回。主面板打开时 Dock 让出输入；Dock 菜单/下载叠放使自动隐藏 Dock 保持可见。
Dock 未能产生首帧时恢复 Material 并
显示失败原因；偏好等待原生样式与 Dock 首帧确认后再保存。

macOS 模式**不再随仓库分发任何 Apple 素材**。此前的 Apple 标识、系统图标与
SFNS / PingFang 字体全部移出：Clarion 以 GPL-3.0-or-later 分发，而 Apple 的许可
不允许再分发，GPL 也无法授予这项权利。移除清单与逐文件 SHA-256 见
[`resources/licenses/removed-apple-assets.sha256`](../resources/licenses/removed-apple-assets.sha256)。

在你自己拥有 macOS 镜像的机器上，可用
[`scripts/import-macos-assets.sh`](../scripts/import-macos-assets.sh) 从本地提取结果
恢复同一套素材（脚本逐文件校验哈希并重新启用 pubspec 声明；导入文件被 git 忽略）。
仓库里保留的只有提取记录 `provenance.json`——它是审计元数据，不是再分发授权。

Dock、启动器的应用格子、搜索结果、文件夹缩略图和拖动反馈共用 `MacAppIcon` 与
`MacAppIcons`。按稳定 desktop ID／图标名识别文件管理器、终端、编辑器、计算器、日历、
图片／文档查看器、截图、字体查看器和系统监视器，导入素材后使用对应的镜像原图；
未导入时回退到内置矢量／Material 图标。名称与启动动作保留真实 Linux 应用，
Firefox、Chrome、微信、QQ 等第三方品牌使用自己的图标。

“控制中心 → 外观 → macOS → 应用使用 macOS 系统图标”可切回应用原生图标。
旧配置默认启用这项外观选择，无需手工改写个人配置；Material 的图标保持独立。
图标按屏幕 DPR 和 Dock 最大放大尺寸选择、解码，悬停复用纹理；缺失或损坏的应用图标
回退到内置矢量图标。移除前的资源来源、原始路径与逐项校验值保留在
[assets/macos/provenance.json](assets/macos/provenance.json)，作为审计记录。
没有执行 macOS 应用。
Dock 和菜单动效按公开行为实现，尚未用 Catalina 实机录像完成逐帧一致性验收。

## Linux 窗口接口

`clarion.shell/window` MethodChannel 提供：

| 方法 | 参数 | 行为 |
| --- | --- | --- |
| `setExpanded` | `bool` | 顶栏与覆盖整个目标输出的面板之间切换 |
| `setShellStyle` | `material` / `macos` | 更新主栏布局，按需创建并复用同引擎 Dock |
| `setBarPosition` | `top` / `bottom` | 保存 Material 栏位置；macOS 主栏始终在顶部 |
| `setDockWidth` | 有限逻辑像素宽度 | 居中 Dock 并限制到目标输出 |
| `setDockInputRegion` | `{x,y,width,height,regions?}` | `regions` 为各可见矩形的列表，原生取并集并裁切；省略时兼容单矩形，展开面板时输入为空 |
| `setPanelGlassRegion` | `{x,y,width,height,radius}` | macOS 展开后更新独立面板圆角模糊区，零宽或零高清除；与菜单栏取真实并集，不模糊二者之间的桌面 |
| `setTooltipVisible` | `bool` | 为悬停提示临时提供 128 px 透明绘制区；面板优先，输入仍限于 40 px 顶栏 |
| `outputName` | 无 | 返回明确传入的输出名称，未指定时为 `null` |
| `surfaceOrigin` | 无 | 返回当前顶栏所在输出的逻辑原点，供托盘动作换算屏幕坐标 |
| `appIcon` | 图标名称，或 `{name, themePath, size, pixelSize?}` | 返回 GTK 图标文件路径；可选 `pixelSize` 为 16–512 的物理像素整数；自定义目录只影响这次查找，找不到时为 `null` |
| `windowState` | 无 | 只读返回 runner 的窗口尺寸、模式和键盘状态，供集成验收 |

`windowState` 还返回 `shellStyle`、`mainViewId`、`dockViewId`、`dockVisible` 和
Dock 首帧、宽度、输入、预留区信息；`dock.inputRegions` 是实际应用到 GDK 的裁切后区域列表。`panelGlassRegion` 是面板请求，`mainBlurRegions` 是主表面实际提交的裁切后模糊矩形列表。`surfaceOrigin` 可传 `main` / `dock`。
主栏悬停提示区域按样式为 128 / 110 px，输入仍只占 40 / 22 px。

顶栏高 40 个逻辑像素，栏内图标、工作区按钮使用紧凑尺寸；展开面板仍保留标准控件尺寸。
键盘模式为 `NONE`。展开时切换到 `OVERLAY` 层与
`EXCLUSIVE` 键盘模式，让全输出模态面板在首次点击时立即取得键盘焦点；关闭后
恢复 `NONE`。固定 `exclusive_zone=40`，仅预留顶栏高度。始终锚定
上、左、右三边，展开按输出逻辑高度请求尺寸，避免四边锚定使正数预留区失效。
输出尺寸变化会更新展开高度，目标输出拔除时选择仍有效的输出。

Flutter view 使用透明背景；runner 清除不透明 region。折叠顶栏的左右上角各
留出 8×8 逻辑像素的输入空隙，使合成器角落热区仍可达。预览/展开时使用完整
输入区域。controller 清理 channel、信号和弱引用，关闭窗口后请求返回错误，
不会继续访问已销毁的 GTK 对象。

顶栏悬停提示绘制在栏下方，透明余区保持鼠标穿透、无键盘焦点，桌面预留区仍为 40 px。
离开图标、点击、切换面板或输出变化会清除提示；预览保持普通窗口尺寸。

`appIcon` 可能返回 SVG 或位图路径，UI 应按格式加载并在不支持时回退为
Material 图标。

## 已实现的组件

- 顶栏：工作区稳定 ID、窗口标题、居中时钟、电池、音量、截图、系统托盘与通知入口。
  极窄输出使用可横向滚动的按钮条，保留所有入口及紧凑点击区域。
- 应用启动器：经典 Launchpad 式整屏分页网格，72 px 图标、两行应用名、居中搜索、
  页码、滚轮/拖动翻页、方向键、PageUp/PageDown、Enter 和 Esc。
  选择跟随稳定应用 ID；每次打开重新读取应用目录，失效启动命令被保守过滤。
  打开/关闭使用可逆缩放与淡入淡出，关闭完成后释放原生表面与键盘；减少动态时直接完成。
- 控制中心：真实音量、亮度、Wi-Fi、蓝牙状态与操作，忙状态及错误反馈。
  电源模式来自已有 power-profiles-daemon，按设备实际支持项显示省电、均衡和性能，
  跟随系统属性变化、授权结果及性能限制原因；服务不存在时显示不可用状态。
- 截图：顶栏入口提供区域、当前屏幕、当前窗口以及鼠标指针选项，使用合成器原有截图功能。
  截图前等待外壳退场与焦点释放，沿用个人截图路径及剪贴板设置；迟到请求或重新打开面板会取消准备。
  已取消请求的迟到错误不会重新打开截图面板，当前请求失败时可直接重试。
- 壁纸选择器：从 `~/图片/Wallpapers` 加载静态图片，提供缩略图、文件名搜索、
  刷新和当前选中标记；选择后填充所有输出，并在下次登录时恢复。
- 通知中心：标准 D-Bus 通知服务、替换、关闭、动作和有界历史；不会抢占已有通知服务。
- 系统托盘：StatusNotifier / AppIndicator 宿主，在顶栏显示图标；左键激活、中键次要激活、
  右键打开 DBusMenu。窄屏或图标较多时提供全部托盘项的展开入口。
  菜单使用现有展开覆盖面板，支持分隔线、禁用项、复选/单选状态和子菜单。
  不会替换已有 `StatusNotifierWatcher`，已有 watcher 时改为注册为 host。
  外部 watcher 重启时重新注册；应用退出后移除项目，菜单与图标跟随 D-Bus 更新。
- 外观：Material 3 语义色和标准控件、深浅主题、减少动态。偏好独立保存于
  `~/.config/clarion-shell/preferences.json`，不改合成器个人 KDL。

当前“勿扰”抑制顶栏通知计数提醒并保留历史；本版尚不提供通知横幅和提示音。
`--preview`、`--demo` 和 `--no-tray` 不声明托盘 watcher，与通知服务的隔离方式相同。
纯 X11/XEmbed 托盘项暂未接入；本模块接入应用导出的 SNI/AppIndicator。
macOS 控制中心已接入 MPRIS 媒体控制；锁屏动作仍调用外部锁屏提供者，Shell 自绘锁屏与集中多输出管理尚未提供。
详细职责和 Denial 来源见 [`docs/native-shell.md`](../docs/native-shell.md)。

壁纸由外壳管理自己的 `swaybg` 子进程，需要系统安装 `swaybg`。
当前目录为 `$HOME/图片/Wallpapers`，支持 PNG、JPG/JPEG、WebP 等静态格式。
选择单独保存于 `~/.config/clarion-shell/wallpaper.json`，不改个人 KDL 或外观偏好。
新壁纸加载并提交后才替换旧的自有壁纸进程；失败时保留上一张并给出错误反馈。
目录、加载进度、选中项与错误由壁纸面板局部订阅，异步结果无需重开面板即可显示。
可用性在启动、刷新和应用时异步检查，绘制阶段不访问文件系统；并发扫描共享一次遍历，后续刷新仍检查文件大小和修改时间。
重复选择未变化的当前壁纸时复用健康背景进程；同路径文件覆写仍重新解码，迟到的启动恢复不覆盖新选择。首次原图解码耗时仍需实测。
可选 `/usr/bin/vipsthumbnail` 为可见格子生成 320 px 静态 PNG，写入
`$XDG_CACHE_HOME/clarion-shell/wallpaper-thumbnails-v1`（默认 `~/.cache`）。缓存以原路径、大小和修改时间失效，最多 512 项/128 MiB；最多两路低优先级、单线程生成，未启动的离屏请求跳过，已启动的离屏解码最多继续至 20 秒超时；关闭面板取消自有进程，损坏时回源一次。缺少提供者时保留原有缩小解码路径。缓存只用于列表预览，应用壁纸始终传原图给 `swaybg`。
`--preview` 不自动恢复壁纸，但手动选择会应用到桌面；`--demo` 不应用或保存。

## macOS 控制中心

菜单栏控制中心采用 Big Sur 风格的紧凑模块：Wi-Fi／蓝牙、勿扰、桌面显示外观、亮度和声音。Catalina 10.15 镜像没有这个整合界面；其余桌面与系统偏好设置继续沿用 Catalina 参考。320逻辑点宽、圆角和过渡时间是工程参数，尚无对应原版运行画面的逐像素验收。

浮层右侧对齐菜单栏右边缘并保留 12 逻辑点边距，跟随菜单栏高度、居中宽度和输出尺寸，点击外部或 Esc 关闭，不压暗桌面。桌面偏好子页改用最高 720、宽 620 逻辑点的居中面板，窄屏自动收缩；返回时恢复紧凑控制中心。独立圆角区域使用原生背景模糊，关闭和样式切换时清理。深浅主题均有胶囊滑条，键盘与无障碍操作继续使用 Flutter Slider；实时状态独立订阅，连续拖动不重建连接卡片或偏好入口。硬件不可用时禁用相应操作，输出设备切换期间暂停音量编辑；声音标题箭头打开既有真实输出设备菜单。

主视图的“桌面偏好设置…”进入可返回的子页，保留电源模式、减少动态、界面风格、栏位置、Dock 与壁纸设置；“系统偏好设置…”打开真实 Clarion 设置应用。Wi-Fi 行只显示电源状态，不把可能来自以太网的连接名称当作 SSID。勿扰仍只抑制通知计数，显示外观只切换 Shell 主题；事件驱动的 MPRIS 卡片仅在存在真实播放器时显示，支持选择播放器、上一首、播放/暂停和下一首；按实时 `Can*` 能力禁用不可用按钮，命令绑定当前 D-Bus owner，防止退出后误控替代进程。无定时位置轮询、不自动启动播放器，默认 demo/测试不连接宿主总线。没有后端的 AirDrop、屏幕镜像、键盘背光等模块不显示空壳。完整未完成项见 [剩余工作清单](../docs/macos-remaining-work.md)。

## 安装

```bash
python3 scripts/install-shell.py --bundle clarion-shell/build/linux/x64/release/bundle
~/.local/bin/clarion-shell --preview
```

安装复制完整 bundle 到独立版本目录，备份已有启动器，并安装应用菜单中的
“Clarion 桌面外壳预览”入口和 user service。安装不改变服务已有的启用状态，
也不会启动服务、停止 Noctalia 或重启 Clarion。首次切换外壳前先用预览验证。
该服务仅在 `XDG_CURRENT_DESKTOP=clarion` 的登录会话启动，避免影响其他桌面。
服务排在 `graphical-session.target` 之后、`xdg-desktop-autostart.target` 之前，
避免 GTK 注册期间同步等待 Portal、Portal 又等待会话就绪的启动环。
就绪通知等待首帧及有界菜单准备；`CLARION_SHELL_STARTUP_TRACE=1` 可记录原生与 Dock 首帧耗时。

## 测试

```bash
cd clarion-shell
~/.cache/clarion/toolchains/flutter/bin/flutter analyze --fatal-infos
~/.cache/clarion/toolchains/flutter/bin/flutter test test
```

`integration_test/native_shell_test.dart` 需要 Linux layer-shell 与真实 Clarion IPC。
在专用嵌套 Clarion 的 `WAYLAND_DISPLAY` 和 `CLARION_SOCKET` 下运行，禁止对当前
主桌面执行其输出缩放、虚拟键盘和截图步骤。该测试检查首次搜索焦点、关闭后释放
键盘、概览动作、40 px 预留区，以及 1 / 1.25 / 1.5 缩放下的展开尺寸。

`integration_test/wallpaper_native_test.dart` 在独立嵌套会话检查壁纸选择、真实
背景像素、切换失败后的保留与登录恢复，使用隔离配置目录，禁止对当前主桌面运行。

`integration_test/launcher_native_test.dart` 在专用嵌套输出、私有 D-Bus 和隔离配置下
检查启动器首次搜索、分页、反向动效、原生表面保留与释放，以及真实截图文件和像素。
截图输出仅写入测试目录；电源交互使用 demo 服务，实际系统仅作只读检查。

`integration_test/mac_shell_configuration_native_test.dart` 与
`integration_test/mac_dock_details_native_test.dart` 在隔离的嵌套会话检查当前菜单栏/Dock 配置、
动态绘制厚度与预留、双视图输入协调及 Dock 细节。早期 `dual_shell_native_test.dart` 尚有旧
windowState 字段和固定 84 px 断言需要迁移，不能作为当前功能已全部覆盖的证明。
测试注入和组件检查不等同于物理鼠标穿透、多屏热插拔或 Catalina 逐帧一致性验收。

## 桌面文件组件

桌面“显示简介”读取真实文件类型、大小、修改时间、路径、权限和符号链接目标；`⌘I` 打开。
“快速查看”支持空格/`⌘Y`，左右切换所选项目，Esc/空格关闭；UTF-8 文本最多 256 KiB，图片最多 8 MiB/16 MP、首帧解码到最高 1600 px。读取可取消，不执行快捷方式，也不自动跟随符号链接；预览中的“打开”继续走原有信任检查。PDF/影音/Office 等格式暂需打开对应应用。
`⌘I`/`⌘Y` 的桌面精确转交还需要含本轮规则的合成器；只更新 Shell 时，已有全局 Mod 绑定可能优先执行。规则仅作用于取得焦点的 macOS 原生桌面，不覆盖普通应用、Material 桌面、模态交互或锁屏。
macOS 背景菜单提供“整理”“整理方式”“排序方式”，多选提供“整理所选内容”。名称为稳定自然排序，另有种类、修改日期、大小；手工整理只消除空位，持续排序会使拖动回弹，选“无”保留当前布局后恢复手工位置。排序按输出保存，旧布局兼容；不以 ctime 冒充创建/添加日期。级联菜单共用一路由，父子均可鼠标命中，支持方向键、Enter、Esc。


正式外壳默认在每个输出上创建透明的桌面文件视图，与顶栏共用一个进程，
无需再启用第二个自启服务。各屏展示同一个 XDG Desktop 目录中的文件、文件夹和
`.desktop` 快捷方式；目录由 `xdg-user-dir DESKTOP` 解析。目录不存在或桌面目录被
禁用时显示错误，不自动创建目录或把整个 HOME 当作桌面。

- 单击选择、双击打开、方向键移动选择。macOS 风格 `⌘O`/`⌘↓` 打开、Return 原位重命名；Material 风格 Enter 打开、F2 重命名。
- 普通文件和目录通过 `gio open` 使用用户的默认应用。若目录在终端中打开，
  可运行 `gio mime inode/directory` 检查关联，并通过系统默认应用设置选择文件管理器。
- 文件名常态显示三行，超出时省略；仅鼠标悬停提示显示完整名称，较长提示可滚动阅读。
  提示避开顶栏，选择文件时不出现全文浮动框，不改变图标位置或提升桌面图层；
  文字放大时相应增大图标格子。
- Delete 或项目右键菜单中的“移到回收站”先确认，再通过系统回收站处理；
  Esc 取消操作。
- 空白处右键可新建文件、文件夹或快捷方式。快捷方式可以选择已安装应用，
  或输入已存在的本地文件、文件夹的绝对路径；界面不接受任意 shell 命令。
- 未信任的 `.desktop` 应用快捷方式在运行前需要确认。由已安装应用选择器创建的
  快捷方式可以直接打开；创建和重命名拒绝覆盖已有项目。
- 拖动图标可以调整网格位置，拖到已占位置时交换位置；取消或放到无效区域恢复原位。
  文件变化按稳定路径更新图标，较多项目通过底部分页或滚轮访问。普通翻页清除选择，
  避免键盘快捷键作用于不在当前页的项目。

位置按输出分别保存到 `~/.config/clarion-shell/desktop-layouts/output-<id>.json`
（设置 `XDG_CONFIG_HOME` 时遵循该目录），各输出独立写入，避免多个 Flutter engine
覆盖同一份布局。旧 `desktop-layout.json` 仅用于读取迁移。窗口尺寸改变后，布局
重新限制到屏幕范围并避免重叠；顶部或底部顶栏所在边始终留出 40 个逻辑像素。
macOS 布局按配置菜单栏高度和 Dock 所在边避让，Dock 为图标尺寸加 16 px（默认 64 px），
自动隐藏的相应预留为零；主题、外壳样式、顶栏位置和减少动态设置跟随外壳偏好更新。

原生桌面图标始终位于 `Bottom` 层，键盘按需取得焦点。右键菜单和对话框使用同一
engine 的独立透明 `Overlay` 视图；图标保持绘制、位置和选择，应用遮住的图标仍然
位于应用之后。交互视图首次建立后保持映射；关闭菜单时清空画面、设置空输入区域并
释放键盘，避免反复重建原生表面时丢失重绘。首帧失败或输出移除时清理。
macOS 风格采用紧凑菜单、蓝色选择反馈及“新建文件夹”优先的空白菜单；Material
风格保留原来的控件尺寸。Esc 和点击菜单外取消，不会执行菜单动作。
背景始终透明，四个角各留出 8×8 逻辑像素输入空隙，保留合成器热区。

```bash
# 仅创建桌面视图，不启动顶栏；正式模式仍管理所有输出。
scripts/run-shell.sh --desktop-only

# 普通窗口预览；文件操作仍作用于传入目录，因此使用独立测试目录。
mkdir -p /tmp/clarion-desktop-preview
XDG_CONFIG_HOME=/tmp/clarion-desktop-preview-config \
  scripts/run-shell.sh --desktop-preview --desktop-dir=/tmp/clarion-desktop-preview

# 仅启动原有顶栏与面板，不附带桌面文件视图。
scripts/run-shell.sh --no-desktop
```

`--desktop-dir=/absolute/path` 覆盖 XDG Desktop 目录，可与正式桌面模式或预览配合。
`--preview`、`--demo` 的原有外壳预览不自动附带桌面组件；桌面单独预览使用
`--desktop-preview`。`--no-desktop` 不能与仅桌面模式组合。

`test/desktop_service_test.dart`、`test/desktop_layout_test.dart`、
`test/desktop_view_test.dart` 和 `test/desktop_application_test.dart` 使用临时目录、
假启动命令与隔离布局，覆盖文件操作、运行确认、键盘、拖放、输出布局以及交互层级，
不改用户桌面文件或启动真实应用。

`integration_test/desktop_native_test.dart` 在隔离的嵌套 Clarion 输出中检查真实
GTK 桌面图层、菜单与对话框的覆盖层切换、文件操作、图标拖动和偏好更新。
它要求 `CLARION_SHELL_ISOLATED_TEST=1` 和 `CLARION_SHELL_TEST_DESKTOP_ONLY=1`，
并通过独立目录注入文件服务，禁止在当前主桌面直接运行。运行现有顶栏集成测试时，
设置 `CLARION_SHELL_DISABLE_DESKTOP=1`，避免其测试入口在附属桌面 engine 中重复执行。
