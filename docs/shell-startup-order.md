# Shell 服务就绪与启动顺序

原生 Shell 的静态服务定义位于 `resources/clarion-shell.service`。用户安装器与系统包使用
同一份定义，只替换可执行路径。服务使用 `Type=notify`、`NotifyAccess=all` 和 30 秒启动超时。
安装脚本不自动 enable、start 或 restart；切换当前桌面由部署阶段单独处理。

顺序为 `After=clarion.service graphical-session.target`，
`Before=xdg-desktop-autostart.target`，`WantedBy=graphical-session.target`。
这让合成器与 graphical target 先就绪，再允许 Shell、portal 与其他桌面服务并行启动，
Shell 保持先于应用自动启动。不要把 Shell 的 READY 反过来作为 graphical target 完成条件。

GPUI Shell 在已连接输出并绘制首帧后向本次 `NOTIFY_SOCKET` 发送 `READY=1`。
命令 socket 建立本身不代表画面已出现。`scripts/test-native-shell.py` 使用私有 datagram
socket 检查真实 READY，同时核对自有嵌套输出上的栏角色；它不连接或修改当前用户服务。

## 历史原因与验证边界

此前 GTK/Flutter Shell 的冷启动曾在同步 GTK portal 激活中等待约 25 秒。
旧服务等待 Shell READY 才完成 graphical target，而 portal 后端又等待该 target，形成运行时环。
将 Shell 放在 graphical target 之后，曾通过私有 D-Bus/嵌套 Wayland 冷启动探针和离线
`systemd-analyze verify` 验证。旧 GTK 探针耗时不代表当前 GPUI 启动性能。

新实现不再依赖 Flutter runner，但保留已验证的服务顺序。实际冷登录耗时、通知与托盘
首次出现时间仍须在正式原生版本安装后单独测量；不能从嵌套首帧推断真实开机登录时间。
