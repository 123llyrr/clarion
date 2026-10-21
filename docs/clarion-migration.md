# Clarion 名称与配置迁移

Clarion 使用 `clarion` 可执行文件、`clarion.service` 会话服务与 `clarion-*` 模块。
原生 Vulkan 后端与 GLES 兼容后端消费同一桌面及场景路径；旧命令只转交参数，不启动另一套核心。
代码来源、版权、第三方声明及许可证继续保留。仓库已迁至
[123llyrr/clarion](https://github.com/123llyrr/clarion)，旧的 remote 需要重新指向该地址；
`REALAND_*` 环境变量与旧命令入口的兼容只在本地运行期有效，与仓库位置无关。

## 配置优先级

1. 显式 `--config` 参数。
2. 非空的 `CLARION_CONFIG`，其次为旧的 `REALAND_CONFIG`。
3. `$XDG_CONFIG_HOME/clarion/config.kdl`，不存在时继续使用原有
   `$XDG_CONFIG_HOME/realand/config.kdl`。
4. `/etc/clarion/config.kdl`，不存在时兼容 `/etc/realand/config.kdl`。

未设置 `XDG_CONFIG_HOME` 时使用 `~/.config`。新安装写入 Clarion 目录。
兼容读取不复制或重写旧配置，包含文件的相对路径、文件权限和配置热重载保持原有含义。
已存在但无效的 Clarion 文件或符号链接会报告其错误，不会悄悄切换回旧配置。

运行前导入旧 `REALAND_*` 覆盖值；对应 `CLARION_*` 已设置时以新名称为准。
CLI 及设置应用的 IPC 优先使用非空 `CLARION_SOCKET`，缺省时兼容 `REALAND_SOCKET`。
合成器向子进程、systemd 与 D-Bus 同时导出两个 socket 变量，指向同一个实例。
显式 socket 无法连接时不会选择另一个桌面实例。

## 命令与桌面数据

安装脚本保留 `realand`、`realand-session`、`realand-settings`、`realand-shell` 和
`realand-shell-msg` 命令入口，
它们执行相应的 Clarion 程序。已有按键绑定和启动命令可以继续使用。
`clarion-shell-msg` 向现有 Shell 发送面板命令；旧命令的 Spotlight 绑定继续生效。

Shell 优先使用 `$XDG_CONFIG_HOME/clarion-shell`。该目录不存在而原有
`realand-shell` 目录存在时，继续在旧目录读写偏好、Dock 固定项目、启动器和各输出的桌面位置，
不会丢弃用户数据或自动创建一份分叉副本。手动创建新目录会明确选择新命名空间。
设置应用以同样的“新文件优先、旧文件缺省兼容”规则读取保存的会话布局。
字体及图标资源保留。

协议可见的应用标识为 `clarion-shell`、`clarion-shell-dock` 和 `clarion-settings`。
窗口和 layer 规则对这三个内置标识提供有限的旧名匹配，现有匹配及排除规则继续生效，
其他应用标识和正则表达式保持原义。

## 激活与回退

安装和配置检查不会注销或重启当前桌面。验证后的新版本在下次登录选择
“Clarion”时启用。项目名称和登录会话名称在所有语言环境下均使用英文。
保留旧安装用于回退；不要在同一个显示会话同时启动两个主合成器。

更名及模块边界不代表全部重构验收已经通过。实际采用的模块、功能回归与物理设备帧时间
应以对应构建的测试记录为准。
