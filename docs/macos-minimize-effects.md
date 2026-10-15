# Dock 最小化效果

macOS 风格设置的「最小化窗口时使用」提供 **神奇效果** 和 **缩放效果**。
Material 仍使用独立配置。旧 macOS 外壳 JSON 没有 `minimize_effect` 时按
`genie` 解析，未知配置值也保持该默认值。设置只改变之后开始的最小化呈现，
不会清空仍可见图标的目标矩形，也不会重新启动正在播放的动画。

窗口由合成器真实最小化，保留映射、窗口 ID、工作区归属和后续客户端提交。
普通布局和输入不再命中该窗口；从 Dock 激活时恢复它，并使其真正获得焦点。
两种效果都采样真实客户端内容，沿用各自输出、截图、录屏的隐私遮挡规则。

## 呈现方式

神奇效果从 Dock 所在边缘开始收口，再将窗口拉向应用图标。底部、左侧和
右侧 Dock 使用对应方向的形变；网格同时支持顶部目标。方向显式来自 Dock
位置，不用窗口和图标之间最大的位移猜测，因此位于对角线的图标不会造成
收口方向错误。横向网格沿运动方向保留 32 段细分，纹理 UV 不旋转。

缩放效果保持窗口原始比例，均匀缩小并移入图标中心；最终矩形等比适配图标
范围。它只需两个三角形，神奇效果使用同一 GLES/Vulkan 路径的细分网格。
中途恢复从当前网格继续反向播放，不从端点重新开始。布局在窗口最小化期间
变化时，恢复动画平滑交接到当前布局位置。

「减少动态」或者合成器关闭动画时，两种效果都立即完成真实状态转换。
无有效目标、输出移除、锁屏或窗口销毁也会结束呈现并清理动画及纹理缓存。
自动隐藏后没有可见图标、滚出屏幕的图标和尚未稳定的图标仍不提供目标。

## IPC 兼容

`SetMinimizeTarget` 的动作格式保留。目标对象增加两个可选字段：

| 字段 | 值 | 省略时 |
| --- | --- | --- |
| `edge` | `bottom`、`left`、`right`、`top` | `bottom` |
| `effect` | `genie`、`scale` | `genie` |

例如底部 Dock 的缩放效果目标：

```json
{"Action":{"SetMinimizeTarget":{"id":4,"target":{"output":"eDP-1","x":800,"y":700,"width":48,"height":48,"edge":"bottom","effect":"scale"},"reduce_motion":false}}}
```

旧客户端仍能注册默认神奇效果；旧合成器忽略目标新增字段，继续执行已有
底部神奇效果。更新磁盘上的合成器不会替换正在运行的进程，新方向和缩放
效果在下一次登录后生效。外壳串行登记目标和策略，迟到的旧 IPC 回复不能
吞掉较新的效果设置；连接更换时不会把旧窗口动作重放到新会话。

## 参考与边界

用户提供的 Catalina 镜像内真实中文资源已经重新检查：

- `Dock.prefPane/Contents/Resources/zh_CN.lproj/PrefPane.strings`：
  `最小化窗口时使用：`、`神奇效果`、`缩放效果`；SHA-256
  `f65aeb77aed85696b6fd2f77ab4061006a4f69668a9e4d5de1dcb8e80b36de23`。
- `Dock.app/Contents/Resources/zh_CN.lproj/DockMenus.strings`：
  `GENIE_EFFECT`、`SCALE_EFFECT`；SHA-256
  `647a5ab0d83c034682b06ab0c8d6850a55e651986de8d8f7e96584d156eece5a`。

[Apple 官方 Dock 配置说明](https://support.apple.com/guide/deployment/dock-payload-settings-depef1fdf19/web)
同样列出左右底部位置以及 Genie/Scale 二选一。资源可确认选项名称和语义，
不能证明私有插值曲线、时间或完整 Dock 实现。本项目采用方案建议的 450/300 ms；
收口函数和反向交接是自己的实现，尚不能称为苹果私有动画参数的 1:1 还原。
再次核查 Catalina 的 SkyLight 后发现 `_SLSCreateGenieWindowAnimation` 是 11 字节的
返回桩；越过该函数边界得到的后续 Sheet 函数及其常量不能当作 Genie 的实现。
当前网格在 Dock 一侧先收口、远端后收口，位置和速度在 75% 交界处连续；
它修复了原先分段公式在这里跳变和短暂停顿的问题。
独立最小化窗口缩略图、Downloads 叠放、最近应用区及其他完整 Dock 能力
是另外的功能，不由这两种动画自动具备。
