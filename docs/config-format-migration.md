# 配置格式迁移：KDL → TOML

状态：设计已定，实现未开始。
日期：2026-10-17。

## 目标与非目标

目标：配置文件从 KDL 换成 TOML，语法归 Clarion 所有，不再与 niri 的配置互为镜像。

非目标：不改变任何配置项的语义；不因为换格式而新增或删除功能。

## 为什么不是"改个扩展名"

KDL 与 TOML 的结构模型不同，直接影响三处现有行为：

| 能力 | KDL | TOML | 处置 |
|---|---|---|---|
| 节点顺序 | 有序，`include` 按出现位置合并 | 表无序 | 见「include 语义」 |
| 同名节点重复 | 原生支持（`window-rule` 可写多次） | 需要 `[[array-of-tables]]` | 逐类映射 |
| 同名属性重复 | 原生支持（一个规则里多次 `match`） | 需要数组 | `match = [...]` |
| 注释 | `//`、`/* */` | `#` | 直接换 |
| 注释掉节点 | `/- node` | 无等价物 | 删除该行 |

## 映射规则

### 1. 普通嵌套块 → 表

```kdl
input {
    keyboard {
        xkb {
            layout "us,ru"
        }
        numlock
    }
}
```

```toml
[input.keyboard.xkb]
layout = "us,ru"

[input.keyboard]
numlock = true
```

无参数的 flag 节点（`numlock`、`tap`、`natural-scroll`）一律变成 `key = true`。

### 2. 带位置的块 → 表 + `name`

```kdl
output "eDP-1" {
    scale 2.0
    mode "3840x2400@60"
}
```

```toml
[[output]]
name = "eDP-1"
scale = 2.0
mode = "3840x2400@60"
```

`output`、`workspace`、`spawn-at-startup`、`layer-rule` 都是这一类；`[[...]]` 保留重复语义。

### 3. 多行规则 → 数组表

```kdl
window-rule {
    match app-id=r#"^org\.wezfurlong\.wezterm$"#
    default-column-width {}
}
window-rule {
    match app-id=r#"firefox$"# title="^Picture-in-Picture$"
    open-floating true
}
```

```toml
[[window-rule]]
match = [{ app-id = "^org\\.wezfurlong\\.wezterm$" }]
default-column-width = {}

[[window-rule]]
match = [{ app-id = "firefox$", title = "^Picture-in-Picture$" }]
open-floating = true
```

同一个规则里出现多次 `match` 时，TOML 用数组表达：

```kdl
window-rule {
    match app-id=r#"^(org\.fcitx\.Fcitx5|fcitx)$"#
    match app-id="WeChatAppEx"
    open-floating true
}
```

```toml
[[window-rule]]
match = [
  { app-id = "^(org\\.fcitx\\.Fcitx5|fcitx)$" },
  { app-id = "WeChatAppEx" },
]
open-floating = true
```

### 4. 按键绑定 → 以组合键为键的表

```kdl
binds {
    Mod+Return { spawn "alacritty"; }
    Mod+Shift+E { quit; }
    Mod+T repeat=false { toggle-window-floating; }
}
```

```toml
[keybinds]
"Mod+Return" = { spawn = "alacritty" }
"Mod+Shift+E" = { quit = true }
"Mod+T" = { repeat = false, toggle-window-floating = true }
```

同一组合键绑定多个动作：

```kdl
Mod+Shift+Q { close-window; spawn "notify-send" "closed"; }
```

```toml
"Mod+Shift+Q" = [{ close-window = true }, { spawn = "notify-send closed" }]
```

原则：一个动作节点 → 表里一个键；动作名即键名，参数变成值；`repeat`、`allow-inhibiting` 这类属性变成同级字段。

### 5. `include`

```kdl
include "work.kdl"
input { ... }
include "home.kdl"
```

```toml
include = ["work.toml", "home.toml"]
```

**这是唯一的行为变化**：KDL 按位置合并，`include` 之后的设置会覆盖 include 里的同名项，include 之间的先后也影响结果。TOML 没有节点序，因此改为：

1. 先按数组顺序递归读入所有 include；
2. 再应用当前文件自身的值，永远覆盖 include 的结果。

结果是确定的、更容易预测的，但与现有「位置决定覆盖」不再等价。迁移文档必须写明这一点，并在 `clarion config migrate` 时提示被影响的行。

## 兼容策略

分三阶段，任何阶段都不让现有用户配置失效：

1. **双读**：`.toml` 优先；同目录下没有 `.toml` 时读 `.kdl`，并打印一次提示。
2. **迁移命令**：`clarion config migrate` 把现有 `.kdl` 转成 `.toml`，保留原文件为 `.kdl.bak`。
3. **收尾**：发布说明标注 KDL 读取将在某个版本移除，之后删掉 knuffel 依赖。

## 里程碑

| 阶段 | 内容 | 估时 |
|---|---|---|
| M1 | schema 与映射规则（本文档） | 完成 |
| M2 | 解析器：`clarion-config` 27 个模块从 knuffel 派生改为 `toml` + `serde`，保留合并语义 | 3–4 天 |
| M3 | 设置应用：`kdl_edit.rs` → `toml_edit.rs`，`config_schema.rs` 目录跟着改 | 2–3 天 |
| M4 | 默认配置、29 篇文档示例、`clarion config migrate` | 1–2 天 |
| M5 | 双读路由 + 弃用提示 + 最终移除 KDL | 0.5 天 |

合计 7–10 个工作日。M2 是主体，约占 50%。

## 风险

- **knuffel 的位置特性无处安放**：`include` 之外，`binds` 的覆盖规则也依赖节点顺序（后写的同名键覆盖先写的）。TOML 表内键无序，只能定义为「文件内后出现的同名键覆盖先出现的」——多数 TOML 解析器保留插入顺序，但这不是规范保证。需要在 M2 里用自己的有序映射，而不是 `toml::Table`。
- **正则里的反斜杠**：KDL 的 `r#"..."#` 原生字符串在 TOML 里要写 `\\`，迁移工具必须正确转义，否则窗口规则会静默失配。
- **设置应用的原位编辑**：`kdl_edit.rs` 保留注释与格式，`toml_edit` 也能做到，但行内表与多行表的取舍会影响 diff 噪音。
- **文档量**：29 篇 wiki 页面含 KDL 示例，不能只改语法，还要保证示例仍然能跑。

## 退回路径

如果 M2 进行中发现有序语义无法用 TOML 稳妥表达，退回方案是：保留 KDL，改用 `config-version` + 别名迁移（顶层节点逐步换名，旧名长期可用）。这条路径成本是 1–2 天，且不破坏任何现有配置。
