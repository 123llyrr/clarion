# 外壳冷启动阻塞：GTK portal 与 session 顺序

## 已证实的启动环

本次冷登录实际记录：shell PID 1316 在 14:00:11.133 启动，11.243 输出 GTK 主题信息，36.332 才创建视图，41.177 被 `Type=notify` 的 30 秒启动超时终止。41.182 graphical-session.target 才完成，41.193 portal 才开始启动。第二次 shell PID 7336 在 43.200 启动，43.639 已就绪。原始时间线、源码归档、独立冷启动探针和日志保存在 验证目录（本地记录：`$HOME/Documents/Codex/2026-10-06/jia/work/shell-startup-order-20261009/root-cause-and-verification.md`）。

旧服务要求 shell 就绪先于 graphical-session.target；本机 portal 及 GTK/GNOME 后端却要求该 target 先完成。GTK 在 shell 激活 Dart/Flutter 之前同步尝试建立 portal 代理，因而形成运行时激活环。

具体路径为 [GTK 3.24.52 GtkApplication DBus startup](https://github.com/GNOME/gtk/blob/3.24.52/gtk/gtkapplication-dbus.c#L444)：没有 GNOME/Xfce session manager 时，Inhibit fallback 使用 `G_DBUS_PROXY_FLAGS_NONE`，helper 调用 `g_dbus_proxy_new_sync`。这会允许自动激活 portal。GLib 对 [StartServiceByName](https://github.com/GNOME/glib/blob/2.90.1/gio/gdbusproxy.c#L1505) 使用默认超时，[-1 对应 25 秒](https://github.com/GNOME/glib/blob/2.90.1/gio/gdbusconnection.c#L2067)，与实际停顿吻合。

这次不是 GTK Settings 的 ReadAll 路径：真实 shell 和用户 manager 都没有 GTK_USE_PORTAL，且非 Flatpak 下 [gdk_should_use_portal](https://github.com/GNOME/gtk/blob/3.24.52/gdk/gdk.c#L449) 只有值以 1 开头时才启用 Settings portal。该分支的 ReadAll 使用 G_MAXINT；设 GTK_USE_PORTAL=0 不会绕过 Inhibit fallback。

已安装 appmenu 模块的原始 `support.c:147–165` 只是查询已运行 DBus 的 ListNames；registrar watch 使用 FLAGS_NONE，没有自动激活 registrar。它不构成本次阻塞的根因。源树为 `work/gtk-module-qa-20261009/vala-panel-appmenu-fda6f0934b1bf0db4629098d68e468fd4effb308`。本轮保留模块与应用 ID。

原始 GTK/GLib 六个文件归档于 `upstream-source/`；来源、版本和 SHA256 见 `upstream-provenance.json`。

## 服务顺序验证

协调方案只改变 shell ordering：`After=clarion.service graphical-session.target`，`Before=xdg-desktop-autostart.target`。保留 WantedBy=graphical-session.target 与 DefaultDependencies=yes；显式顺序会抑制 target 自动添加反向 After。

使用实际安装单位的临时复制件，包括 clarion、shell、hotcorner、graphical/pre、xdg-autostart 与三个 portal 单位，执行 `systemd-analyze --user --generators=no verify`，退出 0。该工具创建离线 manager，不启动、重载或修改实际服务。

真实推导的 debug unit dump（`systemd-verify-new.log`）及解析结果（`systemd-ordering-graph.json`）确认：

`graphical-session-pre → clarion → graphical-session → shell / portal / hotcorner → xdg-autostart`

graphical target 的 Before 包含 shell，After 不包含 shell；shell After 包含 graphical target 与 clarion，Before 包含 xdg-autostart。图没有 ordering cycle。已有 hotcorner 的 WantedBy+After 写法也提供实际运行对照。portal 激活可在 target 完成后与 shell 并行，不再等待 shell 发 READY。

## 隔离冷启动回归

私有 nested Wayland 输出、私有 DBus 和私有 HOME/XDG 路径，实际调用同款 GtkApplication 注册。fake portal 激活等待 graphical.ready，私有 DBus 的 activation timeout 缩短为 4000ms；旧模型只在 GTK 注册结束后释放 target，新模型先释放 target 再启动 GTK。

| 顺序 | GTK_USE_PORTAL | GTK 注册耗时 | appmenu 已载入 / ID 保留 |
|---|---|---:|---|
| 旧 | 未设置 | 4061.59 ms | 是 / 是 |
| 新 | 未设置 | 101.648 ms | 是 / 是 |
| 旧 | 0 | 4035.03 ms | 是 / 是 |
| 新 | 0 | 86.054 ms | 是 / 是 |

四项断言通过，记录见 `cold-probe-results.json` 和各 case 的 probe.log。真实 core PID 1261、shell PID 7336、启动时间与三份个人配置 SHA 全部保留。没有 Flutter 调用、真实应用启动、实际服务重启或用户总线写入；私有进程已清理。

这些数值测量 GTK 注册，不能当作完整桌面冷启动时间。用户真实登录的 25 秒停顿由本次原始 journal 记录；修复后真实登录耗时仍需下一次登录测量。

## 原生时间戳

新增 opt-in `CLARION_SHELL_STARTUP_TRACE=1`，默认不打印。runner 记录 native-entry、gtk-register-begin/end、native-activate、flutter-view-create-begin/end、main-first-frame、dock-first-frame 的 monotonic_us 与 elapsed_ms，便于和 Dart 准备、私有 NOTIFY_SOCKET 的 READY 对齐。

改动仅为 `linux/runner/startup_trace.h`、main.cc、shell_application.cc，以及 window_controller.cc 中 DockFirstFrame 的观测一行。无 GTK/portal/appmenu 禁用逻辑。GTK 探针以 `-Wall -Wextra -Werror` 编译通过；最终 Flutter bundle 编译、首帧与 READY 测量由根代理统一执行。
