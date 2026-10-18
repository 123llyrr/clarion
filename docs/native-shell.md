# 原生 Clarion Shell

`clarion-shell` 是采用 macOS 风格界面的 Rust/GPUI 客户端，保留 Clarion 品牌，与合成器分别拥有自己的事件循环与 GPU 资源。
桌面状态、窗口管理、输入路由、背景效果、颜色管理和呈现调度由合成器管理。
Shell 通过 Clarion IPC 接收桌面快照并发送动作，通过 layer-shell 声明栏、Dock 和面板角色。

## 进程与资源

一个 Shell 进程管理所有输出，共享应用目录、偏好、通知、托盘和最近内容服务。
应用面板直接使用 `clarion-launcher` 的 catalog、launch 和 preferences 模块，不启动
单独的启动器可执行文件。主进程持有运行时命令 socket 的锁；重复启动不能替换已有连接入口。

栏使用 `clarion-shell` namespace 和 Top 层，不请求键盘焦点；Dock 为 `clarion-dock`。
交互面板为 `clarion-panel`，打开时在 Overlay 层按需请求键盘，关闭时释放其表面。
合成器保留对这些明确角色的输入、工作区和全屏策略，不能依靠普通窗口标题判断。

`--preview` / `--demo` 使用普通窗口和示例数据，不连接真实剪贴板、桌面 IPC 或 D-Bus。
正式桌面的全局命令继续发送到 `$XDG_RUNTIME_DIR/clarion-shell.sock`，兼容旧 Realand 名称。
`resources/clarion-shell-msg` 保留裸行命令协议；安装器不会自动启动第二个 Shell 来执行命令。

## Dock 偏好迁移

原生 Dock 在固定应用和运行应用之后，按最近使用顺序补充最多三个仍已安装的应用；
`show_recent_applications=false` 关闭此补充，重复条目和已卸载应用不会占用名额。
`use_original_icons=true` 保留应用的系统图标；关闭后使用随安装包提供的开放许可图标，
按常见应用用途和类别选择。图标路径在首次使用时解析并缓存，不在悬停或每次重绘时扫描目录。
`minimize_into_app=false` 会撤销应用图标的最小化目的地，交由合成器选择默认目的地；
不会把窗口缩进其他应用的图标。运行标记、Dock 位置/大小、自动隐藏、最近应用、图标选择和
最小化效果等已接入的偏好继续生效。`show_launcher=false` 隐藏 Dock 中的应用入口，
顶部栏和快捷键仍可打开应用面板。

尚未迁移的交互是 Dock 悬停放大（`magnification` / `magnification_scale`）、
Downloads 堆栈（`show_downloads`）、顶部栏自动隐藏（`bar_auto_hide`）和
应用启动时的 Dock 弹跳（`animate_opening_applications`）。原有值仍随偏好文件保留，
当前版本不显示暗示这些交互已经实现的开关。这些限制不影响合成器自身的开窗和平铺动画。

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
