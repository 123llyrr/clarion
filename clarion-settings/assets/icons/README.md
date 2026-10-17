# Preference-pane icons

These eight PNGs are the artwork used by the System Preferences home grid:

| File | Pane |
| --- | --- |
| `general.png` | 通用 |
| `mission-control.png` | 调度中心 |
| `keyboard.png` | 键盘 |
| `trackpad.png` | 触控板 |
| `display.png` | 显示器 |
| `advanced-settings.png` | 全部设置 |
| `configuration.png` | 配置源码 |
| `window-layout.png` | 窗口布局 |

They are project artwork supplied for Clarion, not files extracted from a
macOS installation. The separate `../shell/` directory and
`resources/licenses/removed-apple-assets.sha256` document the Apple bitmaps
that were removed from this tree for licensing reasons; nothing in this
directory comes from that import path.

`PreferenceIcon::asset()` embeds each file with `include_bytes!`. A pane whose
file is removed falls back to its original vector symbol in `src/icons.rs`, so
the grid still renders if an asset is missing.
