# Catalina 菜单栏与 Dock：镜像证据、公开行为与 Realand 实现边界

检查日期：2026-10-09。目标版本：macOS Catalina 10.15.7，构建 19H2。

本轮已经从完整安装镜像的 **InstallESD → Core.pkg → Payload** 中选择性恢复 Dock.app、SystemUIServer.app、Dock.prefPane、AppKit 与 Menu Extras。此前 Recovery/BaseSystem 没有 Dock，不能据此推断完整安装镜像也没有。恢复的五个主要 Mach-O，其代码页均与嵌入 CodeDirectory 的 SHA256 匹配；三个完整 bundle 的 895 个资源哈希也全部匹配。原镜像尾部仍缺失，所以不能宣称整个安装包通过校验，也没有验证 Apple CMS 签名或证书链。

这些文件足以确认组件、资源、菜单文案和部分真实调用接口；它们是 Darwin 二进制及资源，并非可移植的 Linux 源码。本次未执行任何镜像代码，也未启动 macOS 应用、安装包脚本或修改运行系统。未运行 Flutter SDK。

## 1. 证据等级

| 标记 | 含义 | 可支持的结论 |
| --- | --- | --- |
| 镜像文件 | 用户镜像、既有恢复文件，以及本轮读取的 plist、资源和 Mach-O 数据 | 文件存在、版本、实际符号、编译资源和哈希；不能自动证明运行时行为 |
| Catalina 帮助 | Apple 官方帮助，URL 明确选择 `10.15` | Catalina 面向用户的选项、菜单栏与 Dock 行为 |
| AppKit 公开接口 | Apple 开发者 API 与归档 Cocoa 文档 | NSMenu、target/action、响应链、动态启用、应用 Dock 菜单的公开模型 |
| 项目建议 | 将以上行为映射到 Realand/Wayland/Linux | 实现取舍；不是 Apple 私有实现、默认值或动画参数的证明 |

当前公开 API 文档和 HIG 会随系统发展更新。本文以镜像中的 Catalina 文件及 `10.15` 帮助确定版本特征；公开 AppKit 文档用于解释长期存在的接口，不能据此补出 Catalina 私有动画曲线。

## 2. 原镜像实际可访问性

原镜像：`$HOME/项目/macOS Catalina 10.15.7 19H2.dmg`，8,278,775,785 字节。既有 `source-fingerprint.json` 记录 SHA256：

```text
15009479b32f57a320830e4d0c08502b8a797a3501214197d8ea07c2872944d1
```

工作目录简称 `W`：

```text
$HOME/Documents/Codex/2026-10-06/jia/work/macos-image-inspection-20261008
```

本轮新增目录 `R = W/menu-dock-research-20261009`。原 DMG 只读，未重做整个 8 GB 镜像提取。

### 2.1 外层与既有恢复

- 原 DMG 的尾部 UDIF footer/块表缺失，源文件从 7,931,428,864 附近进入既有报告所述的全零尾部。不能用普通 DMG 完整性测试为其背书。
- 既有恢复确认 GPT 主头及分区表 CRC；HFS 分区起点 209,735,680、长度 9,656,049,664。既有稀疏文件 `W/catalina-recovered.hfs` 的逻辑大小是该 HFS 长度；报告累计恢复分配数据 7,971,577,856 字节。
- `W/hfs-recovery-result.json` 记录：HFS 块大小 4096，catalog extent `(34378, 2816)`，最后恢复的 HFS 偏移 8,202,264,576。缺少 UDIF 块表的 RAW 边界仍属于推导；已解压的 ZLIB 流经过校验，不代表所有 RAW 块独立校验。
- `W/hfs-list.txt`、`W/install-esd-list.txt`、各 recovery JSON 和原有提取脚本先被读取。本轮没有把旧的无效 InstallESD 提取文件当成可信输入。

### 2.2 有效 BaseSystem 与完整系统包的区别

`W/extracted/Install macOS Catalina/Install macOS Catalina.app/Contents/SharedSupport/BaseSystem.dmg` 是有效的 Recovery 镜像，498,626,206 字节，SHA256：

```text
16d25e469399880002045c46ce0b032711984782ef4de21073aac8b230274399
```

既有 chunklist 记录 48/48 块 SHA256 匹配、长度一致；既有 `7z t` 成功。chunklist 的 RSA 签名并未验证。它确实包含 AppKit 和 AirPort.menu，未包含 Dock.app、SystemUIServer.app、Dock.prefPane。这个缺项只描述 Recovery 的内容。

### 2.3 本轮定位 InstallESD 与 Core.pkg

直接解析已恢复 HFS catalog，得到 InstallESD 的 data fork：

| 对象 | 实际位置与大小 |
| --- | --- |
| InstallESD.dmg | 外层 HFS extent `(191099, 1889025)`，起点 782,741,504；逻辑长度 7,737,443,123 |
| 可用 InstallESD 前缀 | 按外层恢复边界约束为 7,419,523,072 字节，尾部不可用 |
| 内层 HFS | 首个含 HFS header 的可校验 ZLIB 流从 InstallESD 输入偏移 208,370 开始；块大小 4096、总块数 1,983,527 |
| Core.pkg | 内层 HFS extent `(94535, 1830930)`，起点 387,215,360；逻辑长度 7,499,485,537 |
| 其他实际包 | EmbeddedOSFirmware.pkg、FirmwareUpdate.pkg、OSInstall.mpkg、SecureBoot.pkg 的 catalog 记录也存在；本轮不提取或执行它们 |

内层分配图得到五段已分配范围：`(0,17264640)`、`(132608000,11534336)`、`(387215360,7499489280)`、`(7886712832,181149696)`、`(8067870720,56655872)`。按这些范围重建内层 RAW/ZLIB 映射后，才读取真正的 Core.pkg XAR 数据。

重要纠正：旧 `7z` 日志能在原始 InstallESD 字节中发现 XAR magic，但不能将后面的整包字节当作连续存放的原始 XAR。内层 DMG 会交错 RAW 和压缩块；忽略这层映射，会把后续 Payload 偏移读错。

Core.pkg 的真实 XAR header 为 28 字节，压缩 TOC 为 4145 字节，heap 起点 4173；Payload 在 heap 偏移 14,577,519。Payload 格式是 **PBZX 分块 → XZ 或原始块 → ASCII CPIO**，不是单个 XZ 文件。PackageInfo 的标识为 `com.apple.pkg.Core`，版本 `10.15.7.1.1.1600727528`。

XAR TOC 内的元数据校验全部匹配：

| 元数据 | 压缩内容 SHA1 | 解压内容 SHA1 |
| --- | --- | --- |
| Bom | `7287fbf40c1b0301f91b07db54687a9265d8099d` | `b0e19ce095281e4dc99b1b5959cc4f6c3557e60e` |
| PackageInfo | `bcf42b9890088479fd7b5d9146683def81d33647` | `9199d0e3aba514f5b009313307b303261a95bdc8` |

扫描在约 150 秒内读取 472,592 个 CPIO 条目，解码位置达到 15,116,271,577；触及已知缺失源尾部后明确停止。读过 901 个 PBZX 块，其中 806 个 XZ 块完成解压校验、95 个为原始块；内层映射记录 40 个可校验 ZLIB 块、7064 个推导 RAW 块。**不能把所有块都称为“XZ 已校验”。**

本轮只保存 1230 个选定常规文件，共 41,372,373 字节，以及一条不创建实际链接的 symlink 记录。没有生成或解包整个 7.5 GB Core.pkg。保存的 Mach-O 全部按非可执行文件处理。

## 3. 从真实系统 Payload 恢复出的组件

下表路径相对于 `R/selected/`。完整逐项路径、CPIO 偏移、大小和 SHA256 保存在 `R/payload-research-result.json`。

| 实际文件 | 大小 | SHA256 | 文件证据 |
| --- | ---: | --- | --- |
| `System/Library/CoreServices/Dock.app/Contents/MacOS/Dock` | 5,003,904 | `0bb8bd9fcc72720497f8f849ddf1c5743d6d7724ec264b2056b89b0378496e15` | x86_64 Mach-O 可执行文件；bundle `com.apple.dock`、1.8 / 2044.50.1 |
| `System/Library/CoreServices/SystemUIServer.app/Contents/MacOS/SystemUIServer` | 776,032 | `a48141714dfa05fcb01e0a3899561583e54ab204b6247d3cc2d5da32fa1b0dc5` | bundle `com.apple.systemuiserver`、1.7 / 499 |
| `System/Library/PreferencePanes/Dock.prefPane/Contents/MacOS/Dock` | 52,208 | `9969e08688385a3c857b2766d3215f8e6560b926c55e0d5859da4f563e55108f` | `NSPrincipalClass=DockPref`；`NSMainNibFile=PrefPane`；1.7 / 118 |
| `System/Library/Frameworks/AppKit.framework/Versions/C/AppKit` | 25,735,072 | `c6c1616fe02bc306f4b2f165565c8883af530051255e10ab66ae64804e8b4164` | framework `com.apple.AppKit`、6.9 / 1894.60.100；与 Recovery 的同尺寸 AppKit 哈希不同，未混用 |
| `System/Library/PrivateFrameworks/SystemUIPlugin.framework/Versions/A/SystemUIPlugin` | 53,872 | `0f657bad945708378ba7ce3385e4a1012183d6110cb0a0439c9c407d64b1d16b` | 含 NSMenuExtra 类和状态菜单相关方法符号 |

静态读取 `LC_CODE_SIGNATURE`，核验嵌入 CodeDirectory 的 SHA256 代码页：Dock 1208 页、SystemUIServer 184 页、Dock.prefPane 9 页、AppKit 6230 页、SystemUIPlugin 9 页，全部匹配。三个 bundle 的 Info.plist 与 CodeResources 也匹配特殊哈希 slot；其 CodeResources 对普通资源的 SHA256 检查结果分别为 409/409、320/320、166/166，无缺项、不匹配项。

这是文件内部完整性证据，**不是** Apple CMS 签名或根证书信任校验。Dock 与 SystemUIServer 的 `__TEXT` 段 flags 均为 `0x8`；本轮没有解除段保护，不能声称通过 `strings` 已读出它们完整的 Objective-C selector、偏好默认值或动画实现。

### 3.1 NSApplication、菜单和响应链的实际 AppKit 符号

系统 Payload 的 AppKit 符号表存在以下具体条目，括号内为 nlist 值。`n_type=0x0e` 为定义的局部节符号；`0x0f` 为定义的外部节符号。这些不是靠网页描述推定的类名：

```text
-[NSApplication mainMenu]                         0x35269
-[NSApplication setMainMenu:]                     0x2fd7b
-[NSApplication(NSResponder) sendAction:to:from:]  0x28367c
-[NSApplication(NSResponder) targetForAction:]    0x7c8e46
-[NSApplication(NSResponder) targetForAction:to:from:] 0x15ef1d
-[NSApplication validateMenuItem:]                0x233616
-[NSApplication validateUserInterfaceItem:]       0x233c4f
_NSApp                                           0x11384a8
_OBJC_CLASS_$_NSApplication                       0x1123370
_OBJC_CLASS_$_NSMenu                              0x1114aa0
_OBJC_CLASS_$_NSMenuItem                          0x1114af0
_OBJC_CLASS_$_NSResponder                         0x1115d38
_OBJC_CLASS_$_NSDockTile                          0x1113830
```

还实际发现 `-[NSMenu update]`、`autoenablesItems`、`performActionForItemAtIndex:`、`cancelTracking`、`+[NSMenu menuBarHeight]`、`-[NSResponder nextResponder]`、`-[NSApplication setDockMenu:]`、`_createDockMenu:`、`-[NSDockTile setBadgeLabel:]`。这些名字证实接口存在；例如 `menuBarHeight` 的存在本身不证明一个跨主题、显示器和系统版本恒定的高度值。

AppKit 导入 `_CoreDockGetRectAndOrientation`、`_CoreDockSetMiniView`、`_CoreDockMinimizeItemsWithTitle`、`_CoreDockRestoreItem`、`_CoreDockUpdateWindow` 等真实 Dock 通信接口。私有接口名称说明应用与系统 Dock 有协作，不能转换成 Linux 的同名 RPC 而假定其语义相同。

### 3.2 Dock 偏好面板调用的真实接口

Dock.prefPane 主 Mach-O 明确导入以下未定义符号（`n_type=0x01`），并定义 `_OBJC_CLASS_$_DockPref`：

```text
_CoreDockCopyPreferences / _CoreDockSetPreferences
_CoreDockGetTileSize / _CoreDockSetTileSize
_CoreDockGetMagnificationSize / _CoreDockSetMagnificationSize
_CoreDockIsMagnificationEnabled / _CoreDockSetMagnificationEnabled
_CoreDockGetOrientationAndPinning / _CoreDockSetOrientationAndPinning
_CoreDockGetAutoHideEnabled / _CoreDockSetAutoHideEnabled
_CoreDockGetEffect / _CoreDockSetEffect
_CoreDockIsLaunchAnimationsEnabled / _CoreDockSetLaunchAnimationsEnabled
```

编译 `Base.lproj/PrefPane.nib` 是可静态解析的 NSKeyedArchiver plist，39084 字节，SHA256 `a8ce59fe7da4b9497b76efaa7f5643d3bb9dd77b5562f3778974f1e1980eab5e`。其中两个 NSSliderCell 的归档范围均为 `0.0..1.0`，设计归档值为 `0.5`；这不等于用户首次登录的默认值，更不能直接推导 Dock 图标的物理像素上下限。

适合 Realand 配置界面的分组如下。接口栏仅列本次确实读到的导入符号；“Pinning”名称不能直接解释成固定应用列表的排序协议。

| 建议界面组 | 原面板文件证据 | 实际 CoreDock 接口 |
| --- | --- | --- |
| Dock 大小与放大 | 大小、放大、两种滑块 | Get/SetTileSize、Get/SetMagnificationSize、Is/SetMagnificationEnabled |
| Dock 位置与隐藏 | 左/底/右、自动显示隐藏 | Get/SetOrientationAndPinning、Get/SetAutoHideEnabled |
| 最小化与窗口 | 两种效果、最小化到应用图标、标题栏连按、文稿标签页 | Get/SetEffect；其余不据此捏造独立 CoreDock setter |
| 应用行为 | 启动弹跳、运行指示灯、最近应用 | Is/SetLaunchAnimationsEnabled；指示灯与最近项文案存在，但本轮未观察对应独立 setter |
| 通用偏好 | 面板读取/提交偏好 | CopyPreferences、SetPreferences；本轮未证明所有字段的完整私有序列化格式 |

此处省略的 `_CoreDock` 前缀及完整符号见上方原始列表。菜单栏隐藏属于 General，应独立于上述 Dock 组。

### 3.3 SystemUIServer 与 Menu Extras

SystemUIServer 的 Info.plist 注册 `.menu` 为 `MenuExtra plugin` 包类型，`NSPrincipalClass=NSApplication`，`LSUIElement=1`。其 Mach-O 导入 `_CoreMenuExtraAddMenuExtra`、`GetMenuExtra`、`RemoveMenuExtra` 和 NSMenuExtra/NSMenu/NSMenuItem，并依赖私有 SystemUIPlugin、SkyLight、CoreUI 等框架。

完整系统包中实际恢复的 Menu Extras 包括：Clock、Battery、AirPort、Bluetooth、Volume、Displays、User、TimeMachine、VPN、PPP/PPPoE、WWAN、UniversalAccess、DwellControl、SafeEjectGPUExtra、Eject、ExpressCard、IrDA。其 Info.plist 指定如 `AppleClockExtra`、`BatteryExtra`、`AirPortExtra`、`AppleBluetoothExtra`、`AppleVolumeExtra` 等 principal class。文件存在不代表某台 Mac 默认启用所有项目，尤其硬件专用项。

SystemUIPlugin 中实际存在 `-[NSMenuExtra popUpMenu:]`、`setMenuDown:`、`setView:`、`setImage:`、`setTarget:`、`setAction:`、`unload` 等符号。这与状态菜单作为独立组件的模型一致；不能将它们当作普通前台应用的 NSApplication.mainMenu。

### 3.4 原资源与 `.car` 的范围

AppKit `Resources/Assets.car` 为 BOMStore 格式，1,354,416 字节，SHA256 `e236c1327224bdc3ff1d325d75f8d0004a9ea9ca1172a1df42b1817f43414500`。字符串目录可见 `NSLeftMenuBarCap`、`NSRightMenuBarCap`；还恢复了 VersionsButtons、多个 Touch Bar appearance `.car`。它们是编译资源目录，不能直接当作 Flutter widget、完整主题定义或公开动画源码。

原 AppKit 的 `AquaGuideRules.plist` 包含控件间距规则，SHA256 `7eda5d064e190bc03fbc75f7a9c3cbeca0eddd6aa21712523315db8dcaf548d1`；其内容并未给出 Dock Genie 的曲线或完整菜单栏尺寸规范。

Dock 原 bundle 包含 `dockbackground@2x.png`（284×284，SHA256 `a071f705d033e904600d7a7973e974bf67be2f907a5f04ab91c409104277a933`）、`dockbackgroundax@2x.png`（284×284，`dd6ecf51817a0a19c13b9daf7f32f990d7f306dabf51b9977bbde725a391c338`），以及 dark/AX、cartouche 和 shadow 变体。原图片可以作为视觉参照，但文件名和尺寸不证明原 Dock 如何九宫格切分、模糊、组合或响应无障碍设置。

原始 Downloads 图标已经存在于 `W/BaseSystem/macOS Base System/System/Library/CoreServices/CoreTypes.bundle/Contents/Resources/DownloadsFolder.icns`，不是 `DownloadsFolderIcon.icns`。项目已有图标、字体和 Apple logo 的来源见 `clarion-shell/assets/shell/provenance.json`；早期 Launchpad/Mission Control 的 60px 图来自 Apple Catalina 官方帮助。2026-10-09 进一步恢复了 Core.pkg 中 Launchpad.app/Launchpad.icns 和 Mission Control.app/Expose.icns，现已换为未修改的 1024px `ic10` PNG；两份 ICNS 均匹配各自 CodeResources 的 SHA-1 和 SHA-256。此次仅验证选定资源完整性，不代表缺尾镜像或 Apple 证书信任验证。

## 4. 菜单真正由谁提供、动作发给谁

Apple 的菜单编程文档说明：应用在启动时创建自己的主 NSMenu，NSApplication.mainMenu 持有它；NSMenuItem 自身带标题、target、action、状态和子菜单，应用可以动态更新它们。菜单由应用提供，不应根据窗口标题拼出一个固定的“文件／编辑”假菜单。[How Menus Work](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/MenuList/Articles/HowMenusWork.html)

target/action 与键盘、鼠标原始事件是不同层次。明确 target 时，应用向该对象发送 action；target 为 nil 时，从 key window 的 first responder 沿响应链寻找可处理对象，必要时再考虑不同 main window 的链及应用、代理等参与者。`targetForAction:` 只是找接收者，`sendAction:to:from:` 才发送动作。委托可以作为动作接收者，但不能把所有 delegate 都等同普通 `nextResponder` 链节点。[Event Architecture](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/EventOverview/EventArchitecture/EventArchitecture.html)、[sendAction:to:from:](https://developer.apple.com/documentation/appkit/nsapplication/sendaction(_:to:from:)?language=objc)

NSMenu 默认自动检查可见项目的启用状态：先找能处理 action 的 target，再优先询问 `validateMenuItem:`，其次 `validateUserInterfaceItem:`。因此“拷贝”的状态取决于当前应用/文档/选择对象；外壳不能因菜单标题叫“拷贝”就擅自启用。应用关闭自动启用后，也可手动管理状态。[Enabling Menu Items](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/MenuList/Articles/EnablingMenuItems.html)

应用自定义 Dock 菜单同样是真实的 NSMenu：delegate 的 `applicationDockMenu:` 返回值优先于 nib 的 dockMenu；独立 nib 可用 **AppleDockMenu** Info.plist 键声明。Dock 把选择的 target/action 交还应用处理。它不是单纯把全局主菜单再贴到应用图标下。[applicationDockMenu:](https://developer.apple.com/documentation/appkit/nsapplicationdelegate/applicationdockmenu(_:)?language=objc)

**项目映射建议：** Linux 只能消费应用实际导出的 GMenu/Actions 或 canonical DBusMenu，核验总线 owner、PID、窗口导出路径及菜单修订；应用负责 enabled/state/动作，外壳负责显示与请求。打开外壳菜单时临时取得键盘焦点，不应立即把目标应用改成外壳；目标窗口销毁、owner 更换或导出失效时必须拒绝旧动作。未导出菜单的应用应明确回退 Realand 菜单。上述映射是项目设计建议，不是对 Apple 私有跨进程协议的复刻。

## 5. Catalina Dock 配置及交互

真实偏好面板的简体资源 `Dock.prefPane/Contents/Resources/zh_CN.lproj/PrefPane.strings`，SHA256 `f65aeb77aed85696b6fd2f77ab4061006a4f69668a9e4d5de1dcb8e80b36de23`，与 Apple 的 Catalina [Dock 偏好帮助](https://support.apple.com/guide/mac-help/change-dock-preferences-mchlp1119/10.15/mac/10.15) 相互印证：

| 选项 | Catalina 实际行为 | Realand 对接建议 |
| --- | --- | --- |
| 大小、放大及放大滑块 | 基础尺寸与指针附近放大分开控制 | 分别持久化；保存值与屏幕不足时临时缩小值分开 |
| 位置 | 左、底、右 | 纵向 Dock 需要同步保留区、命中区、菜单朝向和最小化目标矩形 |
| 最小化效果 | 原中文为“神奇效果”“缩放效果” | 由 compositor 实现；未验证私有曲线/时长，不能声称参数 1:1 |
| 最小化到应用图标 | 启用时进入应用图标；未启用时在独立区域出现窗口项目 | 需要真实窗口最小化状态及窗口级缩略图/恢复操作 |
| 启动时弹跳 | 应用打开时可显示弹跳 | 应用启动与已运行应用聚焦应区分；不以持续计时器反复弹跳 |
| 自动隐藏 | 指针到相应屏幕边缘显示 | 菜单/拖动开启时需锁定可见，处理边缘触达和实际布局输入 |
| 运行指示灯 | 标识运行应用 | 从真实应用/窗口状态计算，不能用固定项身份代替运行状态 |
| 最近应用 | 与固定项目分开 | 独立记录真实最近使用应用；排除已固定项 |
| 文稿首选标签页 | 始终、仅全屏、手动 | 这是支持标签页的应用行为；没有应用协议能力时不能用外壳 checkbox 假装已生效 |
| 连按标题栏 | 最小化或缩放 | 需要实际 compositor/客户端标题栏配合，不是 Dock 自身的视觉设置 |

原 Dock 的 `Resources/zh_CN.lproj/DockMenus.strings` SHA256 为 `647a5ab0d83c034682b06ab0c8d6850a55e651986de8d8f7e96584d156eece5a`，明确包含 `GENIE_EFFECT`、`SCALE_EFFECT`、`KEEP_IN_DOCK`、`OPEN_AT_LOGIN`、`SHOW_ALL_WINDOWS`、`SHOW_IN_FINDER`，以及文件夹叠放、扇状/网格/列表、按名称/种类/日期排序、分配桌面等资源键。**文案存在不证明每个项目总是同时显示或始终可用。**

Catalina 帮助还明确：Dock 最多展示三个未固定的最近应用，默认有 Downloads 叠放；应用与文件/文件夹位于分隔线的不同侧。拖入是创建 Dock 项目，移除项目不会删除原文件；支持重排和图标快捷菜单。项目的固定应用不应与最近项混为一个无顺序去重集合。[Use the Dock — Catalina](https://support.apple.com/guide/mac-help/dock-mh35859/10.15/mac/10.15)

本次镜像不是已登录用户的 home，所以没有取得某位用户的 `~/Library/Preferences/com.apple.dock.plist`。`com.apple.LSSharedFileList.DockApplications.plist` 的实际内容为 `DefaultsDomain=com.apple.dockitems`、`DefaultsKey=applications`、`IsOrdered=true`；它是 SharedFileList 配置，不是可直接引用的 Dock 用户默认配置。

## 6. Catalina 菜单栏与偏好归属

Catalina 的菜单栏位于屏幕顶部：Apple 菜单位于左端，当前应用名加粗，随后显示该应用真正的菜单；右側是状态菜单、Spotlight、Siri 和通知中心等。应用切换会更换应用菜单。状态项目可通过 Command 拖动调整；不能以 Big Sur 的控制中心布局当作 Catalina 原结构。[Menu bar — Catalina](https://support.apple.com/guide/mac-help/menu-bar-mchlp1446/10.15/mac/10.15)

菜单栏自动隐藏的选项归属 **System Preferences → General**；Dock 自动隐藏归属独立 Dock 偏好面板。Catalina 帮助指定前者在指针到顶部时显示。它不是 Big Sur 后的“Dock 与菜单栏”合并设置。[General preferences — Catalina](https://support.apple.com/guide/mac-help/change-general-preferences-mchlp1225/10.15/mac/10.15)

Apple 菜单中的关于本机、系统偏好、最近项目、强制退出、睡眠、重新启动、关机、锁屏、退出登录等具有实际系统语义。Realand 可以显示平台对应功能，但例如“强制退出”必须选真实进程，而“最近项目”必须由真实历史数据支持。[Apple menu — Catalina](https://support.apple.com/guide/mac-help/whats-in-the-apple-menu-mchlp1130/10.15/mac/10.15)

当前项目的 `menuBarWidthPercent` 属于 Realand 自定义扩展；本轮原偏好面板与 Catalina 帮助未证明 Catalina 存在“菜单栏宽度百分比”选项。`dockSize=32..64`、放大倍率 `1..2`、48px 默认值等也是项目设计值，不能从上面的 NIB 归档值宣称为 Apple 原参数。

## 7. Realand 的配置与缺项建议

本轮只读参考 `clarion-shell/lib/mac_shell_preferences.dart`。新设置已经涵盖三位置、大小、放大、自动隐藏、运行指示灯、打开应用动画、Downloads、菜单栏隐藏和时钟等。以下是后续实现顺序建议，并非本次研究文档已实现这些行为：

原 Dock 菜单与当前能力的缺项矩阵（当前列结合源码及负责实现者本轮同步；本文没有独立运行 UI 测试）：

| 原菜单/能力 | 直接镜像证据 | 当前实现边界与下一步 |
| --- | --- | --- |
| 保留/移除应用、重排 | KEEP_IN_DOCK、REMOVE_FROM_DOCK；帮助描述重排 | 现有真实固定项与长按排序；移除应只改固定偏好 |
| 打开/切已有窗口 | OPEN、SHOW_ALL_WINDOWS | 现有窗口模型可聚焦与多窗口选择；全窗口展示须接真实 overview |
| 打开 Downloads | Downloads 原图存在；帮助描述默认叠放 | 当前是实际打开 Downloads 文件夹，**没有实现 Catalina Stack** |
| 文件夹显示及排序 | DISPLAY_AS_STACK/FOLDER、FAN/GRID/LIST、SORT_BY、日期/名称/种类键 | 尚需目录数据、排序、布局、选择和打开动作；普通入口不是叠放等价物 |
| 最近应用 | 原面板最近应用开关；SHOW_RECENTS | 当前 MacShellPreferences 未包含该开关；需独立最近历史、排除固定项、最多三项 |
| 登录时打开 | OPEN_AT_LOGIN | 需要真实、可撤销的用户自启动登记；不能只改 menu checked 状态 |
| 分配到桌面/显示器 | ASSIGN_TO、ALL_DESKTOPS、THIS_DESKTOP 等 | 需要 compositor 对应工作区/显示器规则，不能从标签直接推出功能已存在 |
| 隐藏/退出/强制退出 | HIDE、HIDE_OTHERS、QUIT、FORCE_QUIT | 必须分别接真实窗口/应用生命周期，并处理进程与多窗口归属 |
| 废纸篓/弹出卷 | EMPTY_TRASH、EJECT | 普通打开 Trash 已有对应能力；清空和卸载卷是额外真实操作 |
| 应用自定义 Dock 命令 | AppKit setDockMenu/_createDockMenu 符号；公开 delegate 协议 | Linux 应用导出的自定义快捷动作与 macOS Dock menu 不是自动等价协议，需明确支持来源 |

用户随后明确要求调节顶栏的上下高度；Realand 设置现提供 22–48 个逻辑像素的高度调节。旧的 50–100% 宽度配置仍可解析以兼容已有文件，但已移除该设置入口。这些参数属于项目扩展。普通 Downloads 入口、工具显示开关、启动弹跳与独立配置可以是有效项目功能，同时仍应明确上表未具备的原系统能力。

1. 保持 Material 的 barPosition 和旧设置独立；macOS 风格只写自身偏好。Dock 的屏幕保留区、真实输入区域与动画目标使用同一份实际 geometry。
2. 最近应用应有独立开关、最多三项和排除固定项的规则；固定项按用户重排结果保存。应用目录缺项、应用卸载和未知运行窗口应各有稳定处理。
3. Downloads 当前作为真实目录入口是有效能力；若宣称 Catalina“叠放”，还需目录读取、排序、扇状/网格/列表、键盘选择等真实后端，不是换一个原图即可。
4. 将“最小化到应用图标”与独立窗口项目补齐到 compositor/window 模型；多窗口、跨工作区、销毁和恢复时不能使用旧 target rectangle。
5. Dock 右键菜单按真实能力提供保留/移除、已有窗口选择、打开位置、退出等。Open at Login、桌面分配、Hide Others、Force Quit 需要对应平台功能，尚未接通时不要展示能点击却无效的项目。
6. 原 bundle 图片与本轮 `.car` 名称可帮助继续核对材质与资源，但模糊、放大、弹跳、Genie 的响应曲线须以真实参考观测和项目测量迭代。不能将原始纹理、私有符号或一个效果名称包装为已证实的完整 1:1 私有动画实现。

## 8. 可复核文件和命令

本轮工作文件：

- `R/read_core_payload.py`：从既有恢复 HFS 选择性读取内层映射、XAR、PBZX、CPIO；不用 7z 对损坏全镜像重复盲提。
- `R/Core.xar-toc.xml`、`Core.PackageInfo.xml`、`Core.Bom.bz2`：真实包元数据。
- `R/nested-udif-map.jsonl`、`pbzx-chunks.jsonl`、`payload-scan.log`：偏移、压缩类型、扫描范围与停止原因。
- `R/payload-research-result.json`：完整选定文件路径、CPIO 偏移、哈希与 `whole_package_verified=false`。
- `R/inspect_selected.py`、`static-inspection-result.json`：静态 Mach-O/header/nlist、CodeDirectory 代码页、特殊 slot 和 CodeResources 校验。只读取 bytes。
- `R/official-help-records.json`：五份 Apple 帮助的请求 URL、解析后 URL、10.15 选择、页面标题和响应 HTML SHA256；对应主题文本用于本地研究。

可安全复核已提取数据，无需再次扫描镜像：

```bash
python3 $HOME/Documents/Codex/2026-10-06/jia/work/macos-image-inspection-20261008/menu-dock-research-20261009/inspect_selected.py
```

有效 BaseSystem 的目录观察可复核为：

```bash
7z l -slt '$HOME/Documents/Codex/2026-10-06/jia/work/macos-image-inspection-20261008/extracted/Install macOS Catalina/Install macOS Catalina.app/Contents/SharedSupport/BaseSystem.dmg'
```

选择性扫描脚本的停止状态是预期的已知尾部缺失记录，不能改成整包成功。本次没有重新对原 8 GB 文件求 SHA256，原镜像指纹沿用先前报告；所有新增选定文件和验证结果在本轮重新计算。
