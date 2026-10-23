# Dock 文件夹与拖文件到应用设计

日期：2026-10-22
状态：待审阅
范围：B（Dock 文件夹）+ C（外部文件拖到应用图标）

## 1. 目标

把 Dock 从“应用图标列表”扩展为“应用 + 文件夹”启动器，并让 Dock 应用图标能接收外部拖入的文件和文件夹：

- 文件夹可以固定在 Dock 上，位于 overflow 磁贴之后、垃圾桶之前。
- 文件夹支持两种磁贴样式、四种展开布局、四种排序字段和升降序。
- 文件夹内容可以浏览、打开、在文件管理器中显示，并可以拖到 Dock 应用图标上由目标应用打开。
- 外部拖入的文件/文件夹落到应用图标时，只有该应用的 `MimeType` 全部支持时才接收，并通过该应用打开。
- 视觉上保持 Dock 连续可见：不使用全屏遮罩，不挖洞，不整屏模糊。

## 2. 已确认的决策

| 主题 | 决策 |
| --- | --- |
| 文件夹归属 | 独立文件夹区，排在 overflow 磁贴之后、垃圾桶之前；不与 pinned 应用混排 |
| 文件夹拖入 | 拖到 Dock 空白/文件夹区即固定到文件夹区末尾；拖到应用图标走 C；拖到垃圾桶不固定 |
| 文件夹重排 | 文件夹磁贴可以在文件夹区内拖动重排，顺序持久化 |
| 内部拖动 | 文件夹条目拖到 Dock 应用图标，由目标应用打开；不做拖到桌面或文件管理器 |
| 自动布局 | 同时参考项目数量和屏幕空间；左右侧 Dock 自动使用列表或网格，不强行竖扇形 |
| 显式扇形 | 左右侧 Dock 也支持，旋转为沿屏幕内侧展开的竖扇形 |
| 多文件匹配 | 所有拖入项都被目标应用支持才接收；否则不接收，不静默丢文件 |
| 子文件夹单击 | 在弹窗内进入 |
| 子文件夹外部打开 | 右键子文件夹，或点弹窗顶部“在文件管理器中显示”；不使用双击，避免与单击进入冲突 |
| 默认磁贴 | 叠层预览；空文件夹回退为标准文件夹图标 |
| 排序字段 | 名称、修改日期、类型、大小；支持升降序 |
| B/C 推进 | 一份合并设计，实施计划按 B → C 分阶段 |

## 3. 非目标

- 不实现内置 Downloads 叠放；现有 `show_downloads` 本轮不接线。
- 不移动、复制、删除或重命名用户文件；从 Dock 移除文件夹只改偏好，不碰磁盘。
- 不做从文件夹弹窗拖到外部应用或文件管理器。Wayland 数据源留到后续。
- 不做文件缩略图、Quick Look、目录实时监听。
- 不做文件夹内容的任意手动顺序；顺序由排序字段决定。
- 不做多选、批量操作。
- 不在应用没有 `MimeType` 时猜测支持类型，也不回退到系统默认应用。
- 不实现应用自定义 Dock 菜单。

## 4. 数据模型与持久化

### 4.1 偏好

在 `clarion-shell/src/preferences.rs` 增加：

```rust
pub struct DockFolderPreference {
    pub path: String,
    pub display: FolderDisplay,
    pub layout: FolderLayout,
    pub sort: FolderSort,
    pub descending: bool,
}

pub enum FolderDisplay {
    Stack,   // 默认：叠层预览
    Folder,  // 标准文件夹图标
}

pub enum FolderLayout {
    Auto,    // 默认
    List,
    Grid,
    Fan,
}

pub enum FolderSort {
    Name,      // 默认
    Modified,
    Kind,
    Size,
}
```

规则：

- 新字段：`dock_folders: Vec<DockFolderPreference>`，默认空。
- 保存时继续合并磁盘上的未知顶层字段，保留现有原子写入和 0600 权限。
- `path` 必须是绝对路径、非空、无 NUL、长度不超过 4096 字节；保存前尝试 `canonicalize`，失败则拒绝固定。
- 去重按规范化后的路径；保持首次出现的顺序。
- 文件夹数量上限 8。达到上限后新的固定请求被忽略。
- 枚举值非法时回落到默认值，不丢弃整个文件夹条目。
- 版本号从 2 升到 3。
- `show_downloads` 保持原样，不在本轮使用。

### 4.2 视图模型

新增 `DockFolderView`，由 `Host::snapshot()` 生成：

```rust
pub struct DockFolderView {
    pub path: PathBuf,
    pub name: String,
    pub display: FolderDisplay,
    pub layout: FolderLayout,
    pub sort: FolderSort,
    pub descending: bool,
    pub snapshot: Option<Arc<FolderSnapshot>>,
    pub error: Option<String>,
}
```

`ViewState` 增加：

```rust
pub dock_folders: Vec<DockFolderView>,
pub folder_navigation: Option<FolderNavigation>,
```

`FolderNavigation` 只保存当前打开的文件夹状态：

```rust
pub struct FolderNavigation {
    pub root: PathBuf,
    pub current: PathBuf,
    pub history: Vec<PathBuf>,
}
```

Dock 磁贴只读 `DockFolderView.snapshot`；弹窗读取同一快照和当前导航状态。

## 5. 目录扫描与排序

新增 `clarion-shell/src/dock_folder.rs`，负责扫描、排序和快照，不依赖 GPUI。

### 5.1 扫描规则

- 只扫描当前目录，不递归。
- `read_dir` 最多访问 4096 个目录项，最多保留 256 个可见条目。
- 跳过以 `.` 开头的隐藏项。
- 使用 `symlink_metadata`；符号链接当作普通条目，不跟随目录链接，避免环。
- 不可读的条目跳过；整个目录不可读时记录错误。
- 条目结构：

```rust
pub struct FolderEntry {
    pub path: PathBuf,
    pub name: String,
    pub kind: FolderEntryKind, // Directory, Document, Image, Application
    pub modified: Option<SystemTime>,
    pub size: u64,
}
```

类型判断：

- 目录 → `Directory`
- `.desktop` → `Application`
- MIME 为 `image/*` → `Image`
- 其他 → `Document`

### 5.2 排序

- 文件夹始终排在文件前，不受升降序影响。
- 名称：不区分大小写的自然排序；数字段按数值比较。
- 修改日期：`None` 排最后；降序时有效时间反向。
- 类型：目录、应用、图片、文档的顺序；同类型再按名称。
- 大小：目录按 0，同大小再按名称。
- 最终都用名称和路径作为稳定排序键。

排序在扫描完成时执行；用户改变排序字段时，对缓存条目重新排序，不重新读盘。

### 5.3 缓存与刷新

- `Host` 持有 `HashMap<PathBuf, Arc<FolderSnapshot>>` 和每路径的扫描代数。
- 固定文件夹、打开文件夹、进入子文件夹时触发后台扫描；渲染线程不读目录。
- 同一路径只接受最新代数的扫描结果，旧结果丢弃。
- 没有快照时磁贴显示标准文件夹图标；扫描完成后替换为叠层预览。
- 扫描失败保留错误；弹窗显示“无法读取文件夹”，磁贴回退为标准文件夹图标。
- 从 Dock 移除时不清除磁盘内容；缓存可以丢弃。
- 本轮不做实时监听；刷新发生在固定、打开、进入子目录和右键菜单“刷新”。

## 6. Dock 几何与磁贴

### 6.1 槽位顺序

从左到右（或从上到下）固定为：

```text
[launcher] [visible pinned apps] [running apps] [recent apps]
[overflow tile if needed] [folders...] [trash]
```

- 文件夹区永远在垃圾桶左侧。
- 文件夹区和应用区之间画一条分隔线；没有文件夹时，分隔线仍然在垃圾桶前，保持现有行为。
- recent 分隔线逻辑保持不变。
- 文件夹不参与 `pinned_app_ids` / `dock_order`，使用独立顺序。

### 6.2 DockMetrics

- `DockMetrics::new` 增加 `folder_count: usize`。
- `utilities = 1（垃圾桶）+ show_launcher + folder_count`。
- 容量先满足垃圾桶、启动台和文件夹，再安排可见应用；应用溢出仍然折叠进 overflow 磁贴。
- 文件夹上限 8，保证在最小图标尺寸和常见输出尺寸下不需要隐藏文件夹。
- 新增槽位解析 helper，例如 `dock_slot_kind(metrics, show_launcher, slot)`，返回 Launcher / App(index) / Overflow / Folder(index) / Trash，避免多处手写索引。
- `dock_tile_center`、最小化目标、右键菜单定位、文件夹菜单定位都使用同一槽位解析。

### 6.3 磁贴外观

- 标准模式：使用 `docs`/`desktop_icons` 中已有的 `desktop-folder.png`。
- 叠层模式：
  - 取当前排序的前 3 项，按类型图标叠放（文件夹/文档/图片/应用）。
  - 图标约为磁贴的 56%，从左下到右上错开，带轻微阴影。
  - 空文件夹回退为标准文件夹图标。
- 文件夹名通过现有 Dock 悬停 tooltip 显示。
- 文件夹磁贴有右键菜单、左键打开、拖动重排。
- 文件夹磁贴不显示运行指示点。

### 6.4 文件夹重排

- 按下文件夹磁贴后，主轴移动超过现有 `DRAG_THRESHOLD` 进入重排。
- 只在文件夹区内部计算插入位置；应用区和垃圾桶不参与。
- 其他文件夹磁贴按现有临界阻尼动画让位。
- 松开后写入 `dock_folders` 顺序，并保存偏好。
- 拖拽期间不打开文件夹、不打开菜单。

## 7. 文件夹弹窗

新增 `Panel::DockFolder { root: PathBuf }`。弹窗是全屏透明 layer surface，与现有 Dock 右键菜单一致：

- 全屏窗口只画弹窗内容和 backdrop；透明区域不填色、不压暗。
- 列表/网格的模糊区域只覆盖弹窗轮廓；扇形不设置整块模糊。
- 输入区域覆盖全屏以捕获外部点击；点击 backdrop 关闭。
- 不使用“挖洞”绘制 Dock；Dock 由下层 surface 继续显示。

### 7.1 列表

- 宽度 300，最大高度为输出高度的 70%。
- 顶部：返回按钮（有历史时）、当前文件夹名（单行省略）、在文件管理器中显示按钮。
- 中间：可滚动条目列表，每行 34px，24px 图标 + 单行名称。
- 底部：在文件管理器中显示；条目被截断时显示“显示剩余项目”。
- 超过最大高度时只滚动条目区，头部和底部固定。

### 7.2 网格

- 宽度 380，最大高度为输出高度的 70%。
- 4 列图标网格；图标 40，单行名称省略。
- 头部/底部与列表一致；中间可滚动。
- 小屏幕使用 `min(380, 输出宽度 - 24)`。

### 7.3 扇形

- 不使用矩形弹窗，不画三角箭头；图标直接从文件夹磁贴中心向外展开。
- 底部 Dock：向上展开；左侧 Dock：向右展开；右侧 Dock：向左展开。
- 可见条目最多 8 个；显式扇形有更多条目时通过滚轮滚动窗口。
- 每个条目由图标和独立文件名胶囊组成；胶囊单行省略，并约束在输出内。
- 扇形末端固定一个“在文件管理器中显示”胶囊；有历史时起点固定“返回”胶囊。
- 扇形滚动只改变可见条目窗口，不移动 Dock 磁贴。
- 自动布局选择扇形时，只有全部条目都能一次显示才用扇形，避免自动模式一打开就需要滚动。

### 7.4 自动布局

- 底部 Dock：
  - 若条目数 ≤ 当前扇形可见容量，且扇形包围盒完全在输出内 → 扇形。
  - 否则 → 网格。
- 左右侧 Dock：
  - 若条目数 ≤ 12 且列表高度/宽度能放在输出内 → 列表。
  - 否则 → 网格。
- 显式选择列表/网格/扇形时始终尊重用户选择；内容放不下时使用滚动。

### 7.5 导航与打开

- 单击文件：用系统默认应用打开（`gio open`），关闭弹窗。
- 单击子文件夹：在当前弹窗内进入，追加历史；不关闭弹窗。
- 返回按钮/扇形返回胶囊：回到上一级。
- 右键子文件夹：用文件管理器打开该子文件夹。
- 右键文件：用文件管理器打开其父目录。
- 顶部/末端“在文件管理器中显示”：打开当前目录。
- 空文件夹显示空状态和“在文件管理器中显示”。
- 读取失败显示错误路径和错误信息。
- 所有文件名单行省略；不换行撑高布局。

### 7.6 视觉连续性

- 列表/网格使用现有 `popup_shape`、圆角、边框和三角箭头；箭头指向文件夹磁贴中心。
- 扇形无底板、无箭头、无全屏模糊；只有图标阴影和文件名胶囊的局部背景。
- 全屏透明弹窗不添加任何全局色调、暗角或模糊。
- 拖动文件夹条目到 Dock 应用图标时，在弹窗内按 Dock 磁贴位置绘制透明投放区和高亮环；不改变 Dock surface 的大小或层级。

## 8. 文件夹右键菜单

新增 `Panel::DockFolderMenu { path: PathBuf }`，使用与 Dock 应用菜单相同的全屏透明弹窗和箭头定位。

内容：

1. 文件夹名标题（单行省略）。
2. “打开文件夹”。
3. “在文件管理器中显示”。
4. “刷新”。
5. 显示：`图标叠加` / `文件夹`。
6. 布局：`列表` / `网格` / `扇形` / `自动`。
7. 排序：`名称` / `修改日期` / `类型` / `大小`。
8. 方向：`升序 ↑` / `降序 ↓`。
9. “从 Dock 移除”。

规则：

- 选中项用现有强调色和背景标示。
- 修改显示/布局/排序/方向后立即保存并刷新磁贴与已打开弹窗；菜单保持打开。
- “从 Dock 移除”只删除 `dock_folders` 条目并关闭菜单。
- 菜单宽度 360；内容超过可用高度时滚动，不裁掉行。
- 所有按钮沿用单行省略样式。

新增动作：

```text
PinFolder(PathBuf)
UnpinFolder(PathBuf)
FolderOrder(Vec<String>)
OpenFolder(PathBuf)
NavigateFolder(PathBuf)
FolderBack
RevealFolder(PathBuf)
SetFolderDisplay(PathBuf, FolderDisplay)
SetFolderLayout(PathBuf, FolderLayout)
SetFolderSort(PathBuf, FolderSort)
SetFolderDescending(PathBuf, bool)
OpenPathsWithApp { app_id: String, paths: Vec<PathBuf> }
```

## 9. 外部拖放

### 9.1 Dock 文件夹固定

- Dock 窗口接收 `ExternalPaths`。
- 应用磁贴消费所有外部拖放，但只对支持的路径执行 C 流程；不支持的拖放静默取消，不能冒泡到文件夹固定逻辑。
- 垃圾桶消费拖放但不执行动作，避免文件夹被误固定。
- 其余托盘/空白区域：仅当拖入恰好一个目录且文件夹数未达上限时接受，固定到文件夹区末尾。
- 拖动经过时显示文件夹区高亮/幽灵投放位；无兼容目标时不显示。
- 拖入普通文件到空白区域不固定、不打开。
- 固定失败（非目录、已存在、达到上限）静默忽略，不修改文件。

### 9.2 文件夹条目拖到应用

- 文件夹条目使用 GPUI `on_drag`，携带 `FolderEntryDrag { path }`。
- 弹窗渲染时，为每个可见 Dock 应用磁贴生成透明投放区：
  - 位置由 `dock_tile_center` 和当前 Dock 布局计算。
  - `can_drop` 检查 `app_accepts_paths(app, [path])`。
  - 兼容目标在拖动悬停时显示强调色描边环。
  - 放下后触发 `OpenPathsWithApp`，关闭文件夹弹窗。
- 不兼容目标不接受，拖动结束自动取消。
- 投放区只覆盖 Dock 图标位置，不遮挡文件夹弹窗内容。

## 10. C：MIME 与启动

### 10.1 应用目录

在 `clarion-launcher/src/catalog.rs` 的 `DesktopApp` 增加：

```rust
pub mime_types: Vec<String>,
```

- 解析 `[Desktop Entry]` 的 `MimeType`，按 `;` 分隔。
- 去空白、转小写、去重，最多保留 128 项，单项长度不超过 256。
- 空 `MimeType` 保持空列表。
- 增加目录解析测试，覆盖多值、空格、大小写、重复和缺失。

### 10.2 路径 MIME

新增 `clarion-shell/src/mime.rs`：

- 目录 → `inode/directory`。
- 文件 → `mime_guess::from_path(path).first_raw()`。
- 无扩展名回退 `application/octet-stream`。
- 匹配规则：完全相等、`type/*`、`*/*`。
- 所有路径都必须存在、绝对、非 NUL；一次最多 16 项。
- `app_accepts_paths` 要求每一项都被支持；任何一项不支持就整体拒绝。

在 `clarion-shell/Cargo.toml` 增加 `mime_guess = "2.0.5"`。

### 10.3 启动

新增 backend 命令：

```rust
LaunchDesktopWithPaths(PathBuf, Vec<PathBuf>)
```

- 校验 desktop 文件绝对且扩展名 `.desktop`。
- 校验所有路径绝对、无 NUL、数量 ≤ 16。
- 调用 `gio launch <desktop-file> <paths...>`，沿用现有 systemd scope 探测、超时和错误处理。
- 扩展 `clarion-launcher/src/launch.rs` 的 `launch_with_runner` 接受附加路径，并补充假 runner 参数测试。
- 失败时沿用现有 backend 错误通道，不重试、不重复打开应用。

### 10.4 拖放反馈

- 应用磁贴用 `on_drop::<ExternalPaths>`；`can_drop` 始终接受，以确保不兼容的拖放不会冒泡成文件夹固定。
- 兼容时在 `drag_over::<ExternalPaths>` 中检查路径并显示强调色描边；不兼容时无高亮、无动作。
- 全部匹配才触发一次 `OpenPathsWithApp`；多文件作为一个参数列表传给目标应用。
- 拖放成功后更新最近应用顺序（如果应用已固定则不重复）。

## 11. 错误与限制

- 文件夹最多 8 个，目录条目最多 256 个，单次访问最多 4096 项。
- 单次拖放最多 16 个路径。
- 目录扫描失败不阻塞 Dock；显示标准文件夹和错误状态。
- 文件名、文件夹名、窗口标题全部单行省略。
- 不执行任何隐式文件写入或删除。
- 移除文件夹、取消固定不会删除原目录。
- 没有兼容应用时不回退到 `gio open`，避免误打开。

## 12. 测试

### 12.1 偏好

- `dock_folders` 默认空、往返、未知字段保留。
- 非法路径/空路径/NUL/重复/超过 8 个被拒绝或去重。
- 非法枚举回落到默认值。
- 版本升级不破坏旧偏好。

### 12.2 扫描与排序

- 隐藏文件跳过、256 条截断、4096 访问上限。
- 目录始终在文件前。
- 名称自然排序、大小写不敏感。
- 修改日期、类型、大小和升降序。
- 缺失路径、不可读目录、符号链接。
- 排序修改只重排缓存，不重新读盘。

### 12.3 Dock 几何

- 文件夹槽位在 overflow 后、垃圾桶前。
- 有/无 overflow、有/无 launcher 的槽位映射。
- 文件夹使用后应用溢出策略仍然正确。
- 左侧/右侧/底部文件夹磁贴中心和箭头偏移。
- 文件夹重排只影响文件夹区。
- 分隔线位置。

### 12.4 文件夹弹窗

- 列表/网格/扇形/自动的布局选择。
- 弹窗边界在输出内，箭头指向磁贴。
- 扇形在三种 Dock 位置都展开到屏幕内侧，所有条目和胶囊在输出内。
- 扇形滚轮窗口、最多 8 个可见项、返回/显示更多固定项。
- 子文件夹导航、返回、外部打开、空目录、错误目录。
- 长文件名不换行、不撑高。
- 右键菜单修改显示/布局/排序/方向后立即生效并持久化。
- 移除文件夹不删除磁盘内容。

### 12.5 拖放

- 外部文件夹落到 Dock 空白区会固定；落到应用图标不固定。
- 外部文件夹落到垃圾桶不固定。
- 外部文件落到空白区不固定。
- 应用支持全部路径时收到 `LaunchDesktopWithPaths`；任一项不支持时不产生命令。
- 目录按 `inode/directory` 匹配。
- 文件夹条目拖到兼容应用磁贴产生打开命令；不兼容不产生。
- 测试使用 GPUI `FileDropEvent` 和真实窗口坐标；在 vendored GPUI 的 `ExternalPaths` 上增加公开的 `new(paths)` 构造器，供测试构造拖入路径。

## 13. 实施阶段

### B1：数据与 Dock 文件夹

- 偏好字段、解析、保存、测试。
- 扫描/排序/缓存模块。
- DockMetrics 文件夹槽位、分隔线、磁贴渲染、叠层预览。
- 外部文件夹拖入固定、移除、重排。
- Dock 文件夹 tooltip 和右键入口。

### B2：文件夹弹窗

- `Panel::DockFolder` 定位、透明 backdrop、列表/网格滚动。
- 扇形几何、滚动窗口、返回/显示更多。
- 子文件夹导航、文件打开、外部显示。
- `Panel::DockFolderMenu` 和配置动作。
- 文件夹条目拖到 Dock 应用投放区。
- 视觉连续性验收。

### C：拖文件到应用

- `MimeType` 解析。
- 路径 MIME 匹配和 `app_accepts_paths`。
- `LaunchDesktopWithPaths` 和 GIO 参数测试。
- 应用磁贴 drop/can_drop/highlight。
- 文件夹拖放复用同一匹配逻辑。

## 14. 验收标准

- 三个 Dock 位置下文件夹磁贴、弹窗和箭头都对齐，无一个图标间隙级别的偏移。
- 文件夹叠层预览、标准文件夹、列表、网格、扇形、自动都可切换并持久化。
- 长文件名不换行、不改变行高、不把后续控件推出弹窗。
- 文件夹条目可以拖到兼容的 Dock 应用图标并由该应用打开。
- 外部拖入多个文件时，只有目标应用全部支持才打开；否则完全不动。
- 全屏弹窗不会压暗、模糊或遮住 Dock；Dock 始终作为底层画面完整可见。
- 现有 Dock 偏好、最近应用、overflow、重排、最小化目标和全部已有测试继续通过。
