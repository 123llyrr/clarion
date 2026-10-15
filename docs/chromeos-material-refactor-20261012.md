# ChromeOS / FydeOS 逆向工程与 Realand Material 外壳重构报告

**日期**: 2026-10-12  
**镜像源**: `$HOME/下载/FydeOS_for_PC_iris_v23.0-SP1-com.bin` (FydeOS v23.0-SP1 Iris x86_64)  
**目标工程**: `$HOME/realand/realand-shell` (`ShellStyle.material`)

---

## 1. 镜像逆向分析与资产提取

### 1.1 GPT 分区解析与 Ext4 根分区提取
通过逆向分析 FydeOS 磁盘镜像的 GPT 分区表，定位 ChromeOS 根分区布局：
- 分区 3：`2.ROOT-A`（ChromeOS 根系统，Ext4，起始 LBA 2,134,016，大小 2.66 GiB）。
- 使用 e2tools 将关键系统资产从 `/opt/google/chrome/` 无损提取：
  - `resources.pak`（36 MB，DataPack v5 主资源包）
  - `chrome_100_percent.pak`（100% 缩放光栅资产）
  - `chrome_200_percent.pak`（200% HiDPI 光栅资产）

### 1.2 Chromium DataPack v5 逆向解包
编写专用解包器，解析 DataPack v5 二进制结构：
- **包头**（12 字节）：`uint32 version=5`，`uint32 encoding=1` (UTF-8)，`uint16 resource_count`，`uint16 alias_count`。
- **资源索引表**：`(resource_count + 1)` 条记录，每条 6 字节（`uint16 id`，`uint32 file_offset`）。
- **别名表**：`alias_count` 条记录，每条 4 字节（`uint16 alias_id`，`uint16 target_index`）。
- **提取产物**：
  - 共解包 **7,954 个独立资源文件** 与 **402 个别名映射**，导出至 `$HOME/Documents/Codex/fydeos-inspection/extracted-resources/`。
  - 包含 590 个 SVG 矢量图标、207 个 PNG 资产、707 个 CSS 样式表、514 个 JS 模块。

---

## 2. ChromeOS Material You (Jelly) 官方设计规范提取

从 `res_57941.html` 等系统资源中提取出 ChromeOS 官方 Jelly Design Tokens：

### 2.1 调色板映射（Palette Tokens）
| 角色 (Token) | Light 模式 | Dark 模式 | 说明 |
|---|---|---|---|
| `primary` | `#405d98` | `#aec6ff` | Material You 标志性主色 |
| `on_primary` | `#ffffff` | `#002b73` | 主色文本/图标 |
| `primary_container` | `#d8e2ff` | `#3d475c` | 激活胶囊/Pod 背景 |
| `on_primary_container` | `#001a41` | `#d8e2ff` | 激活胶囊文本/图标 |
| `secondary_container` | `#dbe2f9` | `#3d4758` | 次级容器/选中高亮 |
| `on_secondary_container` | `#141b2c` | `#dbe2f9` | 次级容器前景 |
| `surface` (`app_base`) | `#fefbff` | `#16181c` | 桌面与面板基底色 |
| `surface_container_low` | `#eff0fa` | `#1b1d22` | 卡片与容器低层级 |
| `surface_container` | `#eff0fa` | `#222429` | 任务栏与标准面板 |
| `surface_container_highest` (`base_elevated`) | `#e1e2ec` | `#2d303b` | 悬浮胶囊/未激活 Pod |
| `on_surface` | `#1a1b20` | `#e2e2e9` | 默认文本与图标 |
| `on_surface_variant` | `#44474f` | `#c5c6d0` | 次要说明与副标题 |
| `outline` | `#74777f` | `#8e9099` | 边框与分割线 |
| `focus_ring` | `#3e5e98` | `#aec6ff` | 聚焦光晕外圈 |

### 2.2 官方 SVG 图标提取与还原
- **Launcher Concentric Rings**（源自 `res_31799.svg` `<g id="launcher">`）：
  - 内部实体圆半径 $r = 5.0$（居中于 $(10, 10)$）
  - 外部同心圆环外径 $r = 8.5$，内径 $r = 7.0$，线宽 $1.5$
- **Quick Settings 图标族**（`res_31798.svg` / `res_31799.svg` / `res_31800.svg`）：
  - Wi-Fi 信号阶梯、蓝牙几何、14 级电池动态电量等。

---

## 3. Realand Shell 重构实施

为在保持 `ShellStyle.macos` 完好无损的同时，将 `ShellStyle.material` 全面升级为 1:1 ChromeOS 桌面保真度，重构了以下 4 大核心模块：

### 3.1 主题层：`lib/theme/shell_theme.dart`
- 新增 `CrosTokens` 常量类，完整内置 Light 与 Dark 模式的 ChromeOS Jelly 颜色变量。
- 重构 `buildShellTheme`，默认采用 ChromeOS 原生 Jelly 配色与 16px 圆角卡片规范。

### 3.2 任务栏（Shelf）：`lib/ui/shell_view.dart`
- **Launcher 按钮**：以 `ChromeLauncherIcon` / `_ChromeLauncherPainter` 替换原有通用图标，绘制像素级还原的 ChromeOS 同心圆发射器。
- **Shelf 应用程序栏（`_ShelfApps`）**：
  - 放置于任务栏中心/工作区右侧，展示当前固定与运行的应用。
  - 每个运行中应用下方带有 4px 紧凑运行指示圆点（活动聚焦窗口显示为 8×3px 药丸线段），与 ChromeOS Ash 任务栏一致。
- **ChromeOS 状态胶囊（Status Area Pod）**：
  - 将右侧状态元素（截图、电池百分比、音量、托盘、通知徽标、控制中心）统一收拢入圆角 20px 的独立 Material 半透明状态胶囊容器。

### 3.3 控制中心（Quick Settings）：`lib/ui/quick_settings_panel.dart`
- 重构为 ChromeOS Ash 风格的 **6 宫格 Feature Pods**：
  1. **Wi-Fi Pod**（支持开启/关闭/网络名/处理中动效/服务不可用提示）
  2. **蓝牙 Pod**（支持开启/关闭/状态展示）
  3. **勿扰模式 Pod**（实时切换通知静音）
  4. **屏幕截图 Pod**（一键触发区域/全屏截图）
  5. **深色模式 Pod**（一键切换浅色/深色 Jelly 主题）
  6. **减弱动效 Pod**（一键切换动画响应）
- 每个 Pod 具备左侧独立高亮图标环、粗体标题与自适应副标题，激活时呈现 Jelly `primaryContainer` 与 `onPrimaryContainer` 高对比配色。

### 3.4 应用启动器（Launcher）：`lib/ui/launcher_panel.dart`
- 重构为 ChromeOS Ash Productivity Launcher（生产力气泡启动器）风格。
- 顶部集成胶囊式搜索条与 ChromeOS 品牌标识，保持方向键网格移动、滚轮分页、回车启动等全套交互逻辑。

---

## 4. 验证与测试结果

- `flutter analyze`: **0 issues**，代码符合全部规范要求。
- 单元测试与组件测试覆盖：
  - `bar_surface_test.dart`: **PASSED**
  - `selective_rebuild_test.dart`: **PASSED** (包括 30 帧硬件拖拽绝不重建任务栏的严格测试)
  - `ui_shell_test.dart`: **99/99 PASSED**
  - `launcher_test.dart`: **17/17 PASSED**
  - `slider_widget_test.dart` / `slider_controller_test.dart`: **PASSED**
  - `shell_style_settings_test.dart` / `bar_position_settings_test.dart`: **PASSED**
- **双风格共存保证**：macOS 风格（Dock 放大、Launchpad 抖动、菜单栏、玻璃模糊等）无任何改动与回归。
