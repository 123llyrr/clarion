# GPUI Shell 的 Big Sur 组件布局

当前 Shell 使用 Rust/GPUI，以 macOS Big Sur 为视觉基线，保留 Clarion 品牌和 Linux 的真实服务。
每种组件有自己的窗口角色、位置与内容，不再把启动器、控制中心、截图和偏好设置放进同一个居中大框。
顶部栏不提供全局应用菜单，也没有调度中心入口；Dock 同样不添加调度中心按钮。
合成器的窗口与工作区能力不因此改变。

本文区分静态逆向证据和 Clarion 的工程实现。**尚未完成与运行中的原版 Big Sur 在相同尺度、
壁纸、主题及色彩条件下的逐像素对比，不能把布局回归或嵌套截图称为原版像素一致性验证。**
公开视觉参考为 [Apple 的 Big Sur 设计介绍](https://www.apple.com/newsroom/2020/06/apple-introduces-macos-big-sur-with-a-beautiful-new-design/)。

## 有直接证据的数值

以下来自 macOS 11.7.2／20G1020 的静态分析；地址只适用于对应版本与架构切片，不是跨版本 ABI。
分析材料与原始系统资源没有作为公开包的运行依赖。

| 项目 | 直接证据 | Clarion 的采用方式与边界 |
| --- | --- | --- |
| 顶部栏高度 | ARM64e SkyLight `default_menu_bar_rect`，`0x184c6374c..0x184c6376c` 含 22／24 两分支 | 默认采用 24 个逻辑点；不能由此声称 Big Sur 的所有状态都是 24。原分支条件的业务含义尚未完整追踪。 |
| 菜单默认字号 | `menuBarFontOfSize:0` 调用链；x86_64 UIFoundation 表项 `0x7fff8041b680` 为 double 13.0 | 顶栏使用 13pt。这个数值不能证明字体栅格、字距或 CoreText 的光学轴选择已经完全复现。 |
| Dock 运行标记 | ARM64e `DOCKIndicatorLayer updateIndicatorForSize:`，`0x10001f064..0x10001f07c`：tile 小于 33 时直径 3，否则 4，半径为直径的一半 | 原生 Dock 使用相应的 3／4 点圆标。图标间距、托盘尺寸、边距和阴影仍须分别验证。 |

文中的尺寸是 GPUI／Wayland 的逻辑单位。物理像素随输出缩放变化，不能把 24pt 直接当成任意显示器上的 24 个物理像素。

## 当前组件的位置与交互

窗口几何由 [`app/panels.rs`](../clarion-shell/src/app/panels.rs) 决定，视图分发与命中区域由
[`view.rs`](../clarion-shell/src/view.rs) 管理。表中面板尺寸、间距、透明度和圆角均为当前工程参数，
不是从原系统测得的完整布局常量；小输出还会限制可用尺寸。

| 组件 | 当前布局 | 交互与实现 |
| --- | --- | --- |
| 顶部栏 | 输出顶部，默认高 24 | 当前应用名称是标签，右侧为真实托盘、最近内容、截图、勿扰状态、控制中心和时钟；部分状态按输出宽度收缩。 |
| Dock | 底部悬浮，或用户选择的左／右边缘 | 独立 `clarion-dock` 表面；固定、运行和最近应用共用真实应用状态，不由其他面板复制一个装饰 Dock。 |
| Launchpad | 覆盖当前整个输出的 Overlay | 搜索位于上方，应用图标按可用宽度排列；内容可滚动，搜索、键盘选择和启动继续使用同一应用目录。 |
| 控制中心 | 右上，宽 320、高最多 424；距右侧 12、距顶栏下沿 8 | 12 点内边距；勿扰／截图两张小卡、显示器亮度与声音胶囊滑条、桌面与 Dock／系统设置／壁纸入口。 |
| 截图工具栏 | 底部居中，390 × 64 | 关闭、整个屏幕、当前窗口和所选部分；底部 Dock 常显时抬高避让，其他情况距底部 24。 |
| 桌面偏好 | 默认 640 × 520 的普通窗口，居中 | 使用普通窗口角色，而非独占键盘的 Overlay；内容滚动，失焦不会像临时菜单一样关闭。 |
| 通知 | 右侧 368 宽、最高 1800 的通知列，独立卡片 | 日期区后为高 132、间隔 12 的通知卡；卡间保持透明。临时通知另用独立通知表面。 |

Launchpad 的 [`launchpad_insets`](../clarion-shell/src/view/macos_launchpad.rs) 为应用网格避让 Dock 边缘：
宽／高取 `max(dock_size + 64, 112)`，自动隐藏模式也保留唤出空间。
绘制、输入与模糊共用 `backdrop_regions`，只在**实际 Dock 画布**处留出透明孔，其他壁纸仍连续覆盖，
避免整个底部或侧边出现磨砂分界。孔内允许操作实际 Dock 表面，并非全屏界面里的第二套 Dock。
Dock 完全隐藏时背景覆盖全屏。

普通桌面偏好使用独立 `io.github.clarion.DesktopPreferences` 身份，避免与系统设置的 `clarion-settings` 开关冲突。
Wayland 普通窗口的初始 bounds 只是请求；Shell 在收到本进程对应窗口的 IPC 标识后，针对该 ID 一次性切至浮动、设置实际尺寸与位置。
用户经窗口管理器关闭时，异步清理旧状态，并按窗口 ID 防止延迟事件关闭新面板。
本机安装配置为这个独立身份加入初始浮动和自绘边框规则，避免先平铺再缩小的过渡及额外焦点框。

```kdl
window-rule {
    match app-id=r#"^io\.github\.clarion\.DesktopPreferences$"#
    open-floating true
    default-column-width { fixed 640; }
    default-window-height { fixed 520; }
    border { off; }
    focus-ring { off; }
    geometry-corner-radius 10
    clip-to-geometry true
}
```

输出尺寸以合成器 IPC 的逻辑宽高为准，工具包缓存作为尚未连接时的回退；缩放不重复乘算。

[`macos_controls.rs`](../clarion-shell/src/view/macos_controls.rs) 只展示真实状态和操作。
HDR 亮度优先使用当前输出状态，其余亮度与音量来自后台服务；不可用时显示禁用胶囊。
拖动复用 80ms 派发节流，按下／松开保留即时与最终值，不靠持续重绘轮询。
没有可用服务的 Wi-Fi、AirDrop 或屏幕镜像不以假开关占位。截图工具栏本身不画选区，选择“所选部分”后才由合成器进入选区流程。

[`macos_notifications.rs`](../clarion-shell/src/view/macos_notifications.rs) 按当前可见卡片计算命中与模糊区域，
避免把整个通知列变成一张不透明大底板。通知动作仍使用通知服务提供的真实 action。

## 材质、字体与验证边界

磨砂背景由原生 Wayland `ext-background-effect-v1` 请求交给合成器处理；GPUI 的透明填色、圆角与阴影提供前景层次。
输入与背景效果共用表面区域，保留圆角外部、通知卡间和 Launchpad 的 Dock 画布孔。
固定 tint、Dock／卡片圆角、阴影、玻璃浓度、网格间距和上述面板尺寸仍是视觉拟合值，
不能从“协议已接通”推出其模糊核、色彩响应或视觉效果与原系统一致。材质也会受实际壁纸、SDR／HDR 和缩放影响。

Shell 可使用用户自行注册的 `Clarion Big Sur Text` 与 `PingFang SC`；前者为本机字体适配 family，
四个静态字重固定在原字体合法的最小 `opsz17`，13pt 是界面字号，二者不是同一个坐标。
缺少这些字体时使用系统可用字体。公开仓库和安装包不分发 Apple 字体或提取的私有图形，
文档也不要求其他机器访问开发工作区或某个本地系统镜像。字体选择与字重可用不等于原版 CoreText 的最终像素已验证。

几何与命中测试、GPUI 输入测试以及 [`test-native-shell.py`](../scripts/test-native-shell.py) 的嵌套验收
用于检查组件角色、打开／关闭、输入与布局回归；普通窗口的 `--preview` 不能替代正式 layer-shell 的模糊与点击穿透验收。
原版视觉一致性仍需同版本、同语言、同缩放和同色彩条件的实机基准，以及匹配的截图和输入轨迹。

Dock 悬停放大、Downloads 堆栈、顶部栏自动隐藏、应用启动弹跳仍未迁移；保留偏好字段不代表交互已实现。
其余架构、构建与隔离边界见[原生 Shell](native-shell.md)。旧 Catalina／Flutter 文档属于历史实现记录，不作为当前 GPUI 的通过证据。
