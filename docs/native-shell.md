# 原生 Clarion Shell

`clarion-shell` 是采用 macOS 风格界面的 Rust/GPUI 客户端，保留 Clarion 品牌，与合成器分别拥有自己的事件循环与 GPU 资源。
桌面状态、窗口管理、输入路由、背景效果、颜色管理和呈现调度由合成器管理。
Shell 通过 Clarion IPC 接收桌面快照并发送动作，通过 layer-shell 声明栏、Dock 和面板角色。

## 进程与资源

一个 Shell 进程管理所有输出，共享应用目录、偏好、通知、托盘和最近内容服务。
应用面板直接使用 `clarion-launcher` 的 catalog、launch 和 preferences 模块，不启动
单独的启动器可执行文件。主进程持有运行时命令 socket 的锁；重复启动不能替换已有连接入口。

栏使用 `clarion-shell` namespace 和 Top 层，不请求键盘焦点；Dock 为 `clarion-dock`。
临时交互面板为 `clarion-panel`，打开时在 Overlay 层按需请求键盘，关闭时释放其表面；
桌面偏好设置采用独立的普通窗口角色，失焦时保留窗口。
合成器保留对这些明确角色的输入、工作区和全屏策略，不能依靠普通窗口标题判断。

`--preview` / `--demo` 使用普通窗口和示例数据，不连接真实剪贴板、桌面 IPC 或 D-Bus。
正式桌面的全局命令继续发送到 `$XDG_RUNTIME_DIR/clarion-shell.sock`，兼容旧 Realand 名称。
`resources/clarion-shell-msg` 保留裸行命令协议；安装器不会自动启动第二个 Shell 来执行命令。

## Dock 偏好迁移

原生 Dock 在固定应用和运行应用之后，按最近使用顺序补充最多三个仍已安装的应用；
桌面偏好的「最近使用」开关控制此补充，`show_recent_applications=false` 时关闭。开启后
最近应用拥有独立槽位：固定应用放不下时先折叠进溢出磁贴，不会挤掉最近应用；重复条目
和已卸载应用不会占用名额。
`use_original_icons=true` 保留应用的系统图标；关闭后使用随安装包提供的开放许可图标，
按常见应用用途和类别选择。图标路径在首次使用时解析并缓存，不在悬停或每次重绘时扫描目录。
`minimize_into_app=false` 会撤销应用图标的最小化目的地，交由合成器选择默认目的地；
不会把窗口缩进其他应用的图标。运行标记、Dock 位置/大小、自动隐藏、最近应用、图标选择和
最小化效果等已接入的偏好继续生效。`show_launcher=false` 隐藏 Dock 中的应用入口，
顶部栏和快捷键仍可打开应用面板。

Dock 悬停放大（`magnification` / `magnification_scale`）、应用启动弹跳
（`animate_opening_applications`）和自动隐藏已接入实际动画。桌面偏好提供放大开关、
100%–200% 比例滑块与启动弹跳开关，自动保存并应用；旧配置相同字段继续生效。
减少动态效果优先于这些装饰动画。启动弹跳只表示启动请求，并不推断客户端就绪状态。
动画在稳定的 Dock 画布内绘制，预留放大与弹跳空间；工作区保留尺寸、截图避让与静止托盘一致，
不随装饰位移反复改变。

最小化／还原复用合成器既有 Genie 450ms、Scale 250ms 轨迹，Shell 登记图标目标和减少动态策略。
Big Sur 静态分析中的 500ms 参考与当前 Genie 时长有区别。悬停、启动弹跳和自动隐藏的
曲线、时长与位移是 Clarion 的工程调校值，不能称为已恢复的 Apple 动画常量。
Downloads 堆栈（`show_downloads`）和顶部栏自动隐藏（`bar_auto_hide`）仍未迁移，
原有值随偏好文件保留，当前版本不显示暗示这两项交互已经实现的开关。
这些限制不影响合成器自身的开窗和平铺动画。

## Dock 几何、溢出与重排

托盘静止长度不超过输出主轴的 80%（同时保留 16px 屏幕边距）；放不下时图标在 64–24px
之间二分缩小，悬停放大允许临时越过该上限。24px 仍放不下时，超出容量的应用折叠进末尾的
“更多”磁贴（圆角矩形内显示省略号，图标不小于 32px 时角标显示折叠数量），省略号磁贴与普通
应用图标同尺寸，数量角标的位置和尺寸随磁贴等比缩放，悬停时与磁贴作为一个整体放大。点击
弹出这些应用的图标网格，网格使用当前磁贴的实际尺寸（含悬停放大），其中右键仍打开单个
应用的 Dock 菜单。图标到托盘四边的间距相等，运行指示点位于
下方边带的中心；行距、端部间距和托盘圆角都随图标尺寸缩放，48px 图标时分别为
12、14、20px，主轴两端间距与横轴间距保持接近，不刻意缩小。

固定应用可以在 Dock 上拖拽重排：按下后主轴移动超过 4px 进入拖拽，其余图标让出空位，
松手把新顺序写入 `dock_order` 偏好（只记录固定应用，未知与重复条目会被过滤，取消固定会
移除其顺序项）。运行中和最近使用的应用不参与重排，拖拽重排不会触发应用启动。
分隔线所在边界额外增加 12–18px 的间距，普通图标之间的行距保持不变；分隔线厚度为 2px，
并居中于这段加宽后的间隙。
以上几何参数与交互阈值是 Clarion 的工程调校值，不能称为已恢复的 Apple 常量。

## Dock 文件夹与文件投放

Dock 文件夹使用独立的 `dock_folders` 顺序，位于 overflow 磁贴与垃圾桶之间；文件夹磁贴
可以在自己的组内拖动重排。每个文件夹保存路径、显示样式（叠层预览／标准文件夹）、
展开布局（列表／网格／扇形／自动）和排序字段（名称／修改日期／类型／大小，支持升降序）。
叠层预览用最多三个满尺寸、带不同旋转角的类型图标叠放，优先把文件图标放在表面，子
文件夹只填充不足三个文件时的后层；打开文件夹时，Dock 磁贴替换为满尺寸圆角矩形收起
控件，中央为稍大且略高于几何中心的向下箭头，关闭后恢复叠层。目录在后台扫描为最多 256
项的不可变快照，跳过隐藏项，不递归、不跟随目录符号链接；Dock 渲染只使用缓存，不读磁盘。

点击文件夹打开全屏透明面板。列表和网格使用现有弹窗轮廓与三角箭头；扇形从文件夹磁贴
沿贴靠边的反方向展开，以等间距、均匀变化的倾角排成柔和圆弧，并用约 420ms 的回弹缓动
弹性散开；滚轮移动可见窗口形成展开／收缩效果；最外侧使用圆形图标按钮在文件管理器中
打开当前目录。从扇形拖出文件时，拖拽预览整体降到 30% 不透明度。图标与文件名标签
共用同一个旋转角，标签中心相对图标的偏移也一起旋转，因此两者始终是一个刚体；底部扇形
最大倾角约 18°，外侧不再压缩间距，而是保持均匀步进，靠末端透明度渐隐的叠加图标形成
收敛。扇形不绘制矩形底板；内容在可见窗口外端未完全展开时，外侧使用最多三个与内容图标
同尺寸的叠加图标，越远越淡直至消失；叠加图标、较小的圆形磨砂打开按钮及其侧面文字标签
都沿末端切线排列，按钮和标签使用同一旋转角并作为整体避让输出边缘。文件名标签使用约
25% 不透明度的磨砂背景、圆角矩形（非胶囊）、更大内边距和单行省略，标签宽度固定。
左右侧 Dock 从点击的文件夹磁贴位置开始展开：图标先水平向屏幕内部伸出，再按较浅的二次
曲线向上弯曲，弯曲方向朝屏幕而不是贴向 Dock。最靠近磁贴的图标保持正常直立角度，向外
沿曲线逐渐倾斜到约 18°，不会在起始位置突然翻转 90°；文件名标签与图标作为同一张卡片，
从图标下方开始随图标一起逐渐倾斜和上扬，并共用同一个旋转角。标签底色、描边
和文字颜色随明暗主题切换，保证不同背景下的可读性。侧边扇形使用足够容纳旋转后标签的
均匀步进，并为末端按钮预留空间，保证标签不会叠合。靠近 Dock 一侧不再添加额外图标，
折叠方向
由 Dock 磁贴的圆角方形向下箭头控件表示。三者都不添加全屏遮罩或整屏模糊，Dock 由下层
surface 完整透出，不需要挖洞。扇形显式声明空的模糊区域，避免 `None` 回退到全屏 input
region 造成整屏模糊。长名称统一单行省略。文件夹配置菜单使用窄幅纵向选项列表，勾选在左，
文字颜色统一，名称、显示、布局、排序、方向和操作组之间用细分隔线分开。子文件夹单击在
弹窗内进入，右键或顶部按钮交给文件管理器；文件单击用默认应用打开。

文件夹条目可以拖到兼容的 Dock 应用磁贴，拖动经过时磁贴显示蓝色高亮，由目标应用在本进程
外打开。外部文件拖入 Dock 时，指针经过的应用与普通悬停一样放大并持续跟随，拖放高亮不会
中断这段放大动效。文件夹面板打开时，拖拽目标只发送给该面板所在输出的 Dock 视图，不会
影响其他显示器；Dock 窗口直接收到的拖拽也只更新自己的悬停状态。外部拖入的文件／
文件夹落到应用磁贴时，只有目标应用 `MimeType` 全部支持才通过 `gio launch <desktop> <files…>`
打开；任何一项不支持就完全不动。Dock 空白处的单目录拖放会固定为文件夹，文件夹组末尾
显示与落点一致的占位槽，不再给整条 Dock 描边；垃圾桶和应用磁贴会消费拖放，避免误固定。
移除文件夹只修改偏好，不删除或移动原目录。
本轮仍不接入 Downloads 堆栈（`show_downloads`）和顶部栏自动隐藏（`bar_auto_hide`）。

## 构建与部署

```sh
cargo build --locked --release -p clarion -p clarion-settings -p clarion-shell
python3 scripts/install-shell.py --check-only
python3 scripts/install-shell.py --binary target/release/clarion-shell
```

仅发布 Rust 可执行文件、命令入口和 `clarion-shell/assets/icons` 中的开放许可资源。
不依赖 Flutter SDK、Flutter bundle、Google Material Shell，也不包含提取的私有系统图形。
图标许可文件随产物安装；中文字体仍由系统 fontconfig 提供。

用户安装器先验证输入，再复制到不可变版本目录并核验 SHA256，最后切换单个 `current`
符号链接。本次事务备份入口，最终步骤失败会恢复；原生版本和个人配置按各自生命周期管理。
安装动作不启用、启动或重启服务。正式迁移会在验收后替换运行进程并清理已托管的退役 Flutter 运行时，不保留旧界面切换或可运行 fallback。统一桌面部署须将
合成器、设置中心、Shell 二进制和图标一起放入同一版本后再统一切换。

RPM、DEB、Nix 与 `scripts/install-session.sh` 安装原生 Shell、图标、命令和服务文件，
不安装独立 `clarion-launcher` 可执行文件。`install-session.sh` 支持 `DESTDIR` 暂存验收；
发行包构建与物理桌面重新登录仍属于各自独立验证。

服务继续使用 `Type=notify`，顺序为
`clarion.service → graphical-session.target → clarion-shell.service → xdg-desktop-autostart.target`。
READY 对应本次进程首帧。详见[服务顺序](shell-startup-order.md)。

## 隔离验收

```sh
python3 -m unittest scripts/test_identity_compat.py scripts/test_packaging_sources.py
python3 scripts/test-native-shell.py \
  --clarion-bin target/debug/clarion --shell-bin target/debug/clarion-shell \
  --artifacts /tmp/clarion-native-shell-evidence --capture
```

打包测试只操作临时目录与替身二进制，检查资源闭包、兼容入口、安装前拒绝、版本保留和失败回滚。
原生 smoke 使用私有 HOME/XDG 路径、私有 D-Bus 和自有嵌套 winit 合成器；只有该合成器
获得父 Wayland 地址。所有 Shell 命令和截图限定在嵌套 socket，不能读取父会话剪贴板。
它检查首帧 READY、栏和面板的真实层级、面板关闭、共进程应用入口、重复实例和预览隔离。
报告、日志和可选 `grim` 读回截图写入指定验收目录，子进程结束后回收。

嵌套读回截图不证明父显示器持续呈现，也不覆盖物理 HDR、多屏热插拔、硬件手势或桌面文件操作。
旧 Flutter/独立 Apps 的验收记录仅属于历史实现，不能作为当前 GPUI Shell 的通过证据。

## Big Sur 顶栏与 Dock

各组件的几何、Launchpad 与真实 Dock 的点击穿透、通知卡片和视觉证据边界见
[GPUI Big Sur 组件布局](gpui-bigsur-components.md)。

顶栏和 Dock 以真实 macOS 11.7.2（20G1020）的本地静态分析为依据，保留 Clarion 品牌；
Rust/GPUI 界面不提供全局应用菜单或调度中心入口。视觉参照见 [Apple 官方 Big Sur 介绍](https://www.apple.com/newsroom/2020/06/apple-introduces-macos-big-sur-with-a-beautiful-new-design/)。
顶栏默认采用 24pt 分支：SkyLight `default_menu_bar_rect` 的 `0x184c6374c..0x184c6376c`
同时保留 22/24 两分支，因此24不是所有系统状态的统一常量。
菜单默认字号13pt来自 `menuBarFontOfSize:0` 调用链与 UIFoundation 表项 `0x7fff8041b680`。
Dock 运行标记依据 `DOCKIndicatorLayer updateIndicatorForSize:` 的 `0x10001f064..0x10001f07c`：
tile尺寸小于33时直径3，否则4；圆角半径为直径的一半。
本机使用私有字体 family `Clarion Big Sur Text` 与 `PingFang SC`；前者由同版本SFNS派生
四个静态字重，固定合法最小光学轴opsz17，13pt仍是UI字号。字体仅在当前用户
`~/.local/share/fonts/clarion-bigsur-text/`、`clarion-bigsur/` 注册，不随公开源码或安装包分发；缺失时回退到系统可用字体。
固定tint、圆角和模糊等视觉值仍是工程选择，尚待同缩放、壁纸及色彩条件的实机像素基准核对，不能宣称全部像素已验证。
桌面已接入 Super+A、Super+O/Down、Super+Backspace、Super+Shift+N、Super+I/Y；
Quick Look支持有大小与解码预算限制的PNG/JPEG和UTF-8文本。Dock 动画与尚未迁移的交互边界见上文。
