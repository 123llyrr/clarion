# Catalina Launchpad 交互比对与实现矩阵

## 证据边界

参考 [Apple 官方 Catalina 10.15 帮助](https://support.apple.com/guide/mac-help/open-apps-with-launchpad-mh35840/10.15/mac/10.15)，以及本地镜像的 LaunchPadLayout.plist。URL、版本、资源哈希和交互对照证据保存在 本轮工作目录（本地记录：`$HOME/Documents/Codex/2026-10-06/jia/work/dock-startup-launchpad-20261009/launchpad-interaction-matrix.md`）。

本地镜像 Dock.app/Resources/LaunchPadLayout.plist 的 layout/ignore/version_exceptions 资源说明应用排列、默认分组与过滤配置存在。原文件路径与 SHA256、结构摘录存放 catalina-launchpad-static-evidence.json。该文件没有给出指针阈值、分页惯性、抖动时序、文件夹开合动画或登录用户数据库。本轮没有运行、解密 Apple 二进制；没有凭静态资源宣称私有动画已还原。

## 当前操作与修复

| 项目 | 官方行为或可验证规则 | 本轮结果 |
|---|---|---|
| 基本启动/退出 | Dock 打开；点击 App 启动；Esc/背景退出 | 保留并回归。搜索时 Esc 保持退出，官方没有写“先清搜索”优先级，不臆造它。 |
| 分页 | Cmd+左右键、横向 trackpad；应用拖到边缘换页 | 搜索焦点内 Cmd 键显式转到分页；触摸普通横滑保留。拖边驻留只在 active drag/前台/仍同一边界有效，取消/关闭清 Timer。 |
| 连续滚动 | 单次有效滚动不应被两条路径处理 | 逆向 AppKit `-[NSPageController scrollWheel:]` (0x463b09) 与 SkyLight `_PKG2DVelocityFilter` (0x7df30)：启用 `allowImplicitScrolling: true` 消除跨页瞬时构建丢帧；引入 200ms `max_dt_reset` 与 180ms 静止重置；支持反向瞬时重置（Zero-Latency Direction Reversal），彻底移除 260ms 硬性门控阻断，回归连续滑动平滑与反向零延迟响应。 |
| 搜索 | 顶部按名称搜索 | 保留 Realand 关键词/描述扩展；新清除入口恢复进入搜索前的浏览页，输入纯空格保持当前页。扩展搜索匹配不冒充 Catalina 严格名称算法。 |
| App→App/Folder | 建文件夹/加入文件夹 | 原逻辑保留；无有效目标、self-drop、folder→app 保留布局、不启动应用。 |
| 同页重排 | 拖 App 到新位置 | 鼠标移动直接拖动；触摸保持350ms长按，使横滑能赢手势。间隙目标与稳定ID共享布局。 |
| 文件夹展示/返回 | 文件夹可打开和标题改名；不丢失根布局 | 改成独立中央面板，根页面在外部保留。关闭以folder稳定ID定位原页，修复从第2/3页打开后固定返回第1页。 |
| 目录实时更新 | 文件夹最后成员消失后安全返回根页面 | 恢复进入文件夹前的页面与选择；如果目录缩减导致页数减少，安全调整到有效页，不误启动应用。 |
| 文件夹内空位 drop | 内部空白仍属于文件夹 | 修复任意内部空白都提取App的问题，内部空位/间隙只重排成员，支持末尾目标。 |
| 文件夹外 drop | 拖出文件夹移到根页面 | 仅明确面板外部区域提取，按可见根页单元定位；完成后回根页，取消不改变成员/排序。 |
| 标题编辑 | 打开文件夹点击名称改名 | 保留原内联编辑；Enter提交、Esc取消编辑的已有回归通过。 |
| Linux能力 | 不伪造 App Store/卸载能力 | 不添加假 Delete/X 或直接删除系统应用；仅调用现有真实 .desktop 启动链路。 |

## 已运行验证

2026-10-10（滚动与分页平滑对齐）：
- 逆向 AppKit `NSPageController` (`scrollWheel:`, `trackSwipeEventWithOptions:`, `_isInSwipeGesture`, `_terminateCurrentAnimation`) 及 QuartzCore 内置默认定时函数 `(0.25, 0.10, 0.25, 1.00)`。
- `mac_launcher_panel_test.dart` 扩充连续滚动单次触发、反向瞬时响应、跨空闲多格推进测试，26 项全部 passed；四个相关测试全部通过（共 38 tests passed）。
- `flutter analyze` 0 issues；`dart format` 完全格式化通过。
- 编译并安装正式发布版本：`20261010-202245-475483` (SHA256: `60331b979fb0b526...`)。

新的隔离 integration fixture：integration_test/mac_launchpad_interaction_native_test.dart；evidence 文件 mac-launchpad-interaction-native-evidence.json。通过 ShellHost 复用实际 GTK main/Dock 双view，使用120项假应用、4种私有 PNG、私有 XDG 布局。输出5阶段实际 grim 截图：第2页、文件夹内部重排、外部拖拽取消、搜索清空恢复、外部提取。文件夹出现要求相对根截图区域至少1000像素变化；所有阶段校验实际像素颜色/亮度，取消要求文件夹实际像素完全恢复。实际应用启动重写为记录 ID，不启动外部进程。该 fixture 由根统一 native runner 执行；此矩阵编写时尚未运行，不以 widget 通过替代 native 成功。

## 仍需独立实测/实现

- 长按进入持续 jiggle 编辑状态、Option 编辑快捷方式与退出规则；在没有真实 Linux 卸载能力时不显示假的删除入口。
- 最后一页尾部拖动创建新空页面、长期用户保留的页间空位；现有 version1 布局是紧凑有序条目，不能宣称持久页空位已支持。
- 跨 native main/Dock view 的 Launchpad→Dock 拖动；现有右键固定可用，但 Flutter DragTarget 的同view命中不能冒充跨view拖放已完成。
- Catalina 文件夹精确网格/开合曲线、App hover/dwell 建组阈值与分页惯性；镜像静态 plist 未提供这些私有参数，需要运行参考录帧与输入测量。
- 触控板完整 PointerPanZoom 惯性、多输出与高DPI的真实硬件输入；本轮事件注入仅验证 Flutter 逻辑，不是 compositor 物理输入认证。
- 文件夹动画目前随外层 Launchpad 过渡，独立文件夹面板反向动画尚未单独实现/像素定标。
