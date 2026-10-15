# Catalina 镜像分析与 Realand 全域复核

日期：2026-10-11。已确认 [REA](https://github.com/morluto/rea) CLI 6.3.0 安装在
`~/.local/bin/rea`；`rea-native` 使用现存 Ghidra 12.1.4、JDK 21 工具链，所选 provider 的
doctor 检查通过。全环境 doctor 仍报告 Codex MCP/skill 未注册及其他可选 provider 缺失；
这与 CLI 和 Ghidra 可用是两件事。本轮通过 CLI 完成分析，没有改全局 MCP 配置。

本轮覆盖动画/调度、Dock/Launchpad、Finder 桌面、菜单/通知、偏好及材质的证据与源码。
已经修复三个可复现问题；没有将整个 macOS 或 Realand 的全部剩余功能宣称为完成。
分析没有执行镜像中的程序，也没有重启当前桌面会话。

## 样本与证据

本轮工作根目录：
`$HOME/Documents/Codex/2026-10-11/github-com-morluto-rea/work/`。

原 DMG 路径为 `$HOME/项目/macOS Catalina 10.15.7 19H2.dmg`。
历史恢复记录明确源尾部缺失；本轮读取最后 512 字节均为零，没有 UDIF `koly` footer，
见 `work/source-dmg-boundary.json`。这次仅检查边界，没有重新计算整镜像哈希。
`项目/rea-analysis` 的子目录为空；新建的 raw/img 文件在预期
HFS header 位置没有有效签名，因此没有将它们当作可信的完整解压系统。
复用的是之前已经恢复并留有资源校验记录的 `selected/` 与 Recovery framework 样本，
具体来源见 `catalina-rea/samples.json`。本轮重新核对其中 17 个常规文件的尺寸与 SHA256，均匹配。
这只确认这些文件的身份，不是整镜像完整性或 Apple CMS 证书链验证。

`catalina-rea/evidence-index.json` 保留 28 条新 REA canonical Evidence，
`native-dock/` 另保留一次函数搜索及六个完整函数分析。每项有样本 digest、函数边界或归档位置。
`native-scheduling/` 另外保留一个 SkyLight 调度函数的新 canonical Evidence。
以下 ID 的完整值保存在原 JSON；短 ID 只是便于查找。

| 范围 | 本轮直接观察 | 证据与边界 |
| --- | --- | --- |
| Dock bundle | 目录库存、资源及 bundle 组成 | `dock-artifact.json`，`ev_c95bde27…`；库存不是运行行为 |
| Dock.prefPane 大小 | `setSize:` 将 double 转为 float 并调用 `CoreDockSetTileSize(value, 0)`；放大尺寸调用 `CoreDockSetMagnificationSize(value, 1)` | `set-size.json` / `set-magnification-size.json`，`ev_81aacb5e…` / `ev_ee531ed1…`；未分析 CoreDock 的最终像素换算，不能据此修改 Realand 的 32–64 px 工程范围 |
| Dock 位置/最小化效果 | orientation UI 值映射到 2/3/4 后调用 `CoreDockSetOrientationAndPinning`；effect setter 映射后调用 `CoreDockSetEffect` | `set-orientation.json` / `set-minimize-effect.json`，`ev_2f1a117a…` / `ev_3ffd79e4…`；不是 Genie 动画网格/时长证据 |
| 双击与最近应用 | 双击 setter 写入 `AppleActionOnDoubleClick` 的 None/Minimize/Maximize，并同步/发通知；最近应用 setter 写布尔偏好 | `set-double-click-behavior.json` / `set-show-recents.json`，`ev_6fbacd66…` / `ev_d336c747…`；设置链路已确认，实时视觉未测 |
| Finder | 整理、按规则整理、持续排序、叠放分别有 action；显示选项有图标大小、网格间距、文字大小、标签位置、简介、预览字段 | `finder-interface.json`，`ev_6253e345…`；ViewOptions 补证 `ev_d626ccef…`。字段存在不代表默认值、动态布局或控件测量 |
| 通知中心 | 标签切换、勿扰、偏好入口，以及应用/日期/单通知各自删除动作 | `notification-interface.json`，`ev_d37a99eb…`；22 个现存 NIB 与原字节及 CodeResources 一致 |
| 输出调度的 125 ms 上限 | `WSDisplayUpdateQueueSchedule` 在当前时间上加 0.125，将超过该值或 NaN 的请求截到该上限；policy 1/2/3 分别限制提前/延后或无条件重新登记 | `native-scheduling/display-update-schedule.json`，`ev_bdda56fd…`；函数完整边界 `0xddf2e–0xde038`，267 字节。地址 `0x288288` 的原文件 double 字节独立核对为 0.125。支持 Realand 的 125 ms 工程上限归因，未建立固定渲染裕量、线程优先级或整个 WindowServer 的调度策略 |

Finder decoder 单次硬上限为 64 个档案。该批次保持 `truncated=true`，其余 25 个
原 NIB 使用 `inspect-keyed-archive` 逐项补证，均无对象分页截断。合计覆盖现存
**89/89 个 Finder NIB 文件**，并核对 SHA256 与 CodeResources。
`finder-resource-coverage.json` 明确这是现存恢复资源的覆盖率；不是完整 Finder UI 或动态行为覆盖率。
请求 96 的 schema 拒绝信封仍保存为 `finder-interface-full.json`，未计入成功证据。

Dock、Finder、SystemUIServer 的 `__TEXT` 有 `0x8` 段保护标记；当前没有建立这些保护代码的
可分析明文。因此本轮原生代码分析选用无该保护的 52 KB Dock.prefPane。
Launchpad 和真实 Dock Genie 的私有实现仍未知。已有 QuartzCore API 参数、timing 曲线与
DisplayLink 证据继续使用 [2026-10-10 REA 复核](macos-rea-evidence.md) 的适用边界。

SkyLight 的新导入期间 Ghidra Objective-C message analyzer 报过一个数组越界异常；
默认自动分析随后完成，返回上述 C++ 调度函数的完整 dossier。所见异常日志保留在
`native-scheduling/ghidra-analysis.log`；临时引擎目录清理后该文件注明保存的摘录范围。
该函数的直接时间比较有汇编及原文件常量支持；
没有把这个结果外推为完整 Objective-C 调用图覆盖率。Decompiler 输出仍是伪代码。

## 已落到源码的改进

1. **Downloads 排序能力一致性。** 原右键菜单允许选择无法取得的添加/创建日期，打勾和保存
   日期偏好后却按名称排序。现在服务提供统一 `effectiveSort`，右键与 Stack 菜单都勾选实际
   生效排序；缺少元数据时禁用该日期，能力变化后刷新，打开后已失效的旧动作也会被拒绝。
   没有使用修改时间冒充创建或添加时间。原 Catalina `DockMenus.strings` 的日期选项是结构依据，
   其 SHA256 为 `647a5ab0d83c034682b06ab0c8d6850a55e651986de8d8f7e96584d156eece5a`。
2. **Dock 强制退出忙碌状态。** 菜单原来未订阅 `force-quit-app` busy 字段，挂起操作时按钮
   可能仍可点击。现纳入 selector，操作期间禁用、完成后恢复。回归使用 fake Future，没有杀用户进程。
3. **同时间戳反向手势。** `SwipeTracker` 原先在首个反转分片到达时删除历史；同刻 `+5/-15`
   与合并的 `-10` 会得到不同速度（复现为 0 与 −1000 px/s）。现在保留有界历史，先合并同刻
   样本，再根据最终方向选速度区间，惯性终点也一致。这是 Realand 输入正确性修复，未声称为
   Catalina 私有滤波算法。

改动在 `src/gestures/swipe_tracker.rs`，及 Shell 的 `dock_folder_service.dart`、
`mac_dock_context_menu.dart`、`mac_downloads_stack_panel.dart`、`mac_dock_app_menu.dart`。
`src/output_schedule.rs` 仅补充已确认的调度证据出处及格式整理，未改变调度逻辑。
本轮前文件副本与精确增量分别保存在 `work/animation-audit/`、`work/shell-audit/`。
保留项目原有未提交改动。

另新增 `scripts/verify-catalina-samples.py`，分析前核对显式 manifest 的常规文件身份；
缺文件、同尺寸损坏、哈希时路径替换都会失败；输出不能覆盖输入（含硬链接）。
它只核对 manifest 声明的尺寸/哈希，不验证 manifest 的出处、REA Evidence ID 或 Apple 签名。

```bash
python3 scripts/verify-catalina-samples.py \
  $HOME/Documents/Codex/2026-10-11/github-com-morluto-rea/work/catalina-rea/samples.json
```

## 验证与各域剩余边界

- Rust：手势 17、动画 39、输出调度 5、帧时钟 18 项通过；一个既有 microbenchmark 忽略。
  另有 49 项最小化/手势/overview 有限回归通过（部分与前组覆盖重叠）。真实 GLES/Vulkan 窗口
  像素两项显式跳过；这些结果不代表物理桌面逐帧验收。
- Flutter：本轮改动相关 44 项通过；桌面排序/叠放/内部复制粘贴、Launchpad、通知、系统菜单、
  MPRIS 的 80 项扩展回归通过；四个修改的 Dart 源文件和两项新测试的静态检查无问题。
- Python：样本校验脚本 7 项回归通过，项目脚本套件共 26 项通过；17 个实际样本校验通过。
- 源文件格式与定向 diff 检查通过；`scripts/build-shell.sh --release` 成功，产物在
  `realand-shell/build/linux/x64/release/bundle/`。构建记录在 `work/shell-release-build.log`。
  本轮没有把产物安装到运行会话，也没有重启合成器；源码修改需使用新构建启动后生效。

| 范围 | 当前源码已具备/本轮验证 | 仍需完成或测量 |
| --- | --- | --- |
| 动画、手势、调度 | 物理弹簧、Genie/Scale、输出相位/预算、手势修复与相关回归 | 当前未取得 Catalina Genie 明文实现/实机逐帧参数；不能将已有工程值称为原值 |
| Dock / Launchpad | 放大/隐藏、Downloads/Trash、文件夹分页/抖动、上述两项 Dock 修复 | Hide 当前用最小化模拟，Quit 只关闭窗口；`realand msg quit` 退出的是合成器，不能拿来退出应用。跨 native view 拖入 Dock、真实窗口纹理仍待补 |
| 桌面 / Finder | 文件操作、内部文件复制粘贴、叠放及显示选项已有源码和回归 | 不等于完整 Finder 目录窗口；跨应用系统剪贴板/文件拖放、完整标签/预览提供者仍欠缺 |
| 系统菜单 / 通知 | 系统信息、最近应用、强制退出入口、真实通知删除/动作已有实现并测过 | 最近文稿/服务器、横幅/声音、账户天气/日程及第三方 Today 组件未补齐 |
| 偏好 / 控制中心 | 原偏好 setter 和 UI 字段已证实；已有 Linux 设置与 MPRIS 操作 | 目前混合 Catalina 桌面与 Big Sur 控制中心；不能从 Catalina 推导 Big Sur 布局。完整网络选择/配对、颜色管理等后端仍需实现 |
| 材质 / 字体 / 性能 / 多屏 | 既有字体、模糊、缓存、逻辑输出实现保留 | 同尺度截图/录屏、混合 DPI/GPU 物理验收、偶发停顿的同步调用栈尚缺；未宣称卡顿消失 |

需要真实 Catalina runtime 的事项继续保持未知。后续可从原目录中的可信样本及本轮索引直接
继续，避免重复解压损坏镜像或把旧清单中的已实现功能重复开发。
