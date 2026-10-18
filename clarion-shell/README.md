# Clarion Shell

原生 Rust/GPUI 桌面 Shell，保留 Clarion 品牌与单一 macOS 风格界面。顶部栏、Dock、应用面板、控制中心、通知、托盘和最近内容
共用一个事件循环与服务所有者；每个输出拥有自己的 layer-shell 表面。界面不提供旧 Material 或多样式切换。应用目录、Desktop Entry 启动及视图偏好复用 `clarion-launcher` 库。

## 构建与预览

需要仓库要求的 Rust 工具链、Wayland、Vulkan/EGL、fontconfig、FreeType 和 xkbcommon。
不需要 Flutter SDK 或单独构建启动器可执行文件。

```sh
scripts/build-shell.sh --release
scripts/run-shell.sh --release --preview
```

`--preview` 使用普通窗口和示例状态，不连接真实桌面 IPC、剪贴板和 D-Bus 服务。
`--check` 只读取配置并输出能力报告。`--quit-after=N` 用于有界诊断。
正式模式为 `scripts/run-shell.sh --release --monitor=0`；`--monitor` 保留服务兼容，
进程管理所有输出，`--output-name=NAME` 可限定测试输出。一个运行时目录只允许一个正式实例。

## 安装

```sh
python3 scripts/install-shell.py --check-only
python3 scripts/install-shell.py --binary target/release/clarion-shell
```

默认安装到 `~/.local`，服务文件放入 XDG 用户配置目录。版本内容为
`lib/clarion-shell/releases/native-<时间>/bin/clarion-shell` 和 `assets/icons`，
由 `current` 原子选择；公开命令通过该版本路径解析。安装前校验可执行权限、图标许可文件、
大小、符号链接与 SHA256，原生版本目录及本次事务的入口备份用于安装失败恢复。
安装器不修改偏好、不启用服务、不切换当前桌面。

如果已经使用包含合成器、设置中心和 Shell 的统一版本目录，应在同一桌面版本中更新
Shell 二进制及 `assets/icons`，再统一切换入口。独立安装器不会推断或改写统一版本的其他组件。
Flutter、Material 界面与独立启动器均已退役，不提供可运行的旧实现回退。正式迁移在原生版本验收成功后清理已托管的退役运行时；历史分析报告与用户素材不属于运行时。

`clarion-shell-msg launcher`、`spotlight`、`control-center`、`notifications` 和 `close`
继续使用运行时目录中的命令 socket，兼容 `realand-shell` 与 `realand-shell-msg` 命令。
应用面板在 Shell 内创建，安装器不再安装 `clarion-launcher` 可执行文件。

## 验证

```sh
python3 -m unittest scripts/test_identity_compat.py scripts/test_packaging_sources.py
python3 scripts/test-native-shell.py \
  --clarion-bin target/debug/clarion --shell-bin target/debug/clarion-shell \
  --artifacts /tmp/clarion-native-shell-evidence --capture
```

原生 smoke 创建私有配置、D-Bus、运行时目录和嵌套 winit 输出。只有嵌套合成器连接父
Wayland；Shell、命令、截图均限定在嵌套输出。检查首帧 READY、栏/面板角色、应用面板共进程、
重复实例保护与预览隔离，结束时回收测试子进程。`--capture` 需要 `grim`。
这项检查不能代替物理多屏、热插拔、HDR 或实际鼠标/触控板验收。

[架构及边界](../docs/native-shell.md) · [服务顺序](../docs/shell-startup-order.md)
