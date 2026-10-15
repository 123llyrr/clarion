# macOS 动画方案：REA 镜像证据复核

2026-10-10，按用户要求使用 [REA](https://github.com/morluto/rea) 重新分析镜像中的原生二进制。本轮使用 REA 6.3.0、Ghidra 12.1.4、Temurin JDK 21.0.12.1+1；实际调用 REA 的 `open_binary`、`analyze_function`、`search_procedures`、`search_strings`、`read_bytes`，保留完整 Evidence、函数边界及伪代码。分析没有执行 Apple 程序。

本机证据目录：`$HOME/Documents/Codex/2026-10-09/zh/work/macos-animation-plan-20261010/rea/`。`evidence-index.json` 是简化索引，相应 `*.json` 保存完整返回；请求顺序见 `requests.jsonl`。下文 Evidence ID 的完整值保存在索引中。

## 样本与安装

- QuartzCore：3,862,896 字节，SHA-256 `548c33b372abb978150e99f064d38bb4bed7fdb70a79ef233daff8d4b18c4ec7`。
- SkyLight：4,193,888 字节，SHA-256 `38cde43140bfb76a9cc19cda64fc8e8d260c8c25963bfba650a3f078d8d5b78b`。
- 二者均从已恢复的 Catalina 10.15.7 / 19H2 `BaseSystem.dmg` 中对应 framework 的 `Versions/A/` 成员只读重新提取，哈希与既有分析样本一致，见 `library-provenance.json`。该恢复系统框架不能代表已运行完整 macOS 桌面。
- Dock 可执行文件：5,003,904 字节，SHA-256 `0bb8bd9fcc72720497f8f849ddf1c5743d6d7724ec264b2056b89b0378496e15`，来自已有镜像恢复的 `Dock.app/Contents/MacOS/Dock`。
- REA CLI 原已安装，本轮确认其版本与 npm 官方发布一致。新增的 Ghidra、JDK 下载包经官方 SHA-256 校验后安装到 `~/.local/share/rea-toolchains/`，见 `installation.json`。
- 原生分析命令 `~/.local/bin/rea-native` 选择上述工具链，并限制两颗 CPU、1536 MB Java 堆及后台优先级。`rea-native doctor --provider ghidra --json` 的所选 provider 范围检查通过。没有修改 Codex 的 MCP 配置。

## 已确认的内容与方案修正

| 问题 | 观察与适用边界 | 原始 REA 证据 |
| --- | --- | --- |
| CASpring API 默认物理参数 | `+[CASpringAnimation defaultValueForKey:]` 在 `0x4511d–0x4526b` 返回质量 1、刚度 100、阻尼 10、初速度 0；对应阻尼比 0.5 是由这些值计算而来。它们是该 API 的默认值，不能推导所有 macOS 窗口动效均采用相同弹簧。 | `quartz-spring-defaults.json`，`ev_1a11c659…` |
| settlingDuration 的 epsilon | `-[CASpringAnimation settlingDuration]` 在 `0x6deb5–0x6dece` 调用 `durationForEpsilon:`，传入 double 0.001。 | `quartz-settling.json`，`ev_9bd90d1b…` |
| 停摆时间是否一概无需迭代 | `durationForEpsilon:` 在 `0x6ded5–0x6e13d` 对欠阻尼使用对数包络公式；临界及过阻尼路径以 0.1 秒步长调用 `State::eval`，检查持续收敛，再按动画 speed 缩放时长。因此方案把“全部直接解析、几个时钟周期”称作原生事实并不准确。Realand 同时约束位置与速度的解析包络是本项目的优化。 | `quartz-duration-epsilon.json`，`ev_e02af19a…` |
| 五条内置 timing 曲线 | `_getPoints:` 在 `0x13227–0x13251` 以索引读取 `0x236f10` 的 float32 表，再转换为 double。表按 default、linear、easeIn、easeOut、easeInEaseOut 排列；常用十进制写法分别为 `(0.25,0.1,0.25,1)`、`(0,0,1,1)`、`(0.42,0,1,1)`、`(0,0,0.58,1)`、`(0.42,0,0.58,1)`。实际 float32 精度见原始字节，不能把十进制 double 写法称作位级相同。 | `quartz-timing-points.json`，`ev_783d1802…`；`quartz-builtin-table.json`，`ev_92d977ba…`；`quartz-timing-name-index.json`，`ev_1cfb8d0f…` |
| DisplayLink 相位锁定 | `align_phase_locked` 在 `0x689f6–0x68be9` 依据时间间隔取模对齐，避免设定早于当前 host time 的期限，重新设置 Mach timer。`dispatch_items` 更新回调状态与时间字段。它们支持相位对齐的设计方向；仅这些函数不足以证明所有 WindowServer 动画的求值时刻与线程策略。 | `quartz-phase-align.json`，`ev_4936ea15…`；`quartz-dispatch.json`，`ev_ce0f1dbb…` |
| DisplayLink 的 targetTimestamp | `CADisplayLink::targetTimestamp` 在 `0x69582–0x695c6` 将当前存储的 host timestamp 加上 frameInterval × duration；它提供目标回调时间，支持按目标呈现时刻求值的设计。仍不能据此推导未分析的所有 WindowServer 动画调用点。 | `quartz-displaylink-target.json`，`ev_ef082ed6…`；`quartz-displaylink-timestamp.json`，`ev_3232c9ee…` |
| “系统过渡统一为 255 ms” | **原方案的引用不能支持该结论。** `_derive_anim_duration` 在 `0x98a0f–0x98b9b` 把 0.255 用作 0.01 与 0.5 之间的初始二分点，随后依据输入迭代。它不是无条件返回 255 ms 的默认时长。Realand 如采用 255 ms，应标作项目动效参数。 | `skylight-animation-duration.json`，`ev_0177dda4…` |
| “Genie 原生分段阈值 0.75” | **原方案混淆了函数边界。** `_SLSCreateGenieWindowAnimation` 的完整函数体只有 `0x4275e–0x42768` 共 11 字节，无条件返回 `0x3ee`；它没有网格形变代码。0.75 来自后面的 `_SLSCreateSheetWindowAnimationWithParent`，并且根据调用参数有条件选取，随后由 `_SheetAnimationProgress` 使用。这是 Sheet 路径，不能归因于 Genie。 | `skylight-genie-create.json`，`ev_1d7b1243…`；`skylight-sheet-create.json`，`ev_1685e817…`；`skylight-sheet-progress.json`，`ev_127ffea7…` |
| Genie 450 ms / Scale 300 ms | 上述被原方案引用的 SkyLight 函数没有建立这两个值。它们目前是 Realand 的动效参数，仍需实际 Dock 路径或运行时逐帧验证。 | 以上 Genie / Sheet / duration 三组明确函数边界 |
| “动画时提升到最高实时优先级并固定预留 2–4 ms” | 本轮相关 DisplayLink 函数与 ManagedDisplayIsAnimating 事务包装只建立计时/状态传递证据，没有建立该优先级与固定裕量结论。Realand 继续根据本机输出周期、渲染耗时与迟到样本自适应计算预算。 | `quartz-phase-align.json`；`skylight-managed-*.json` |
| 交互弹簧 ratio 0.92 / stiffness 300 | 是方案建议及 Realand 的项目预设，本轮没有获得它们是 Catalina 拖拽/平铺原生参数的证据。 | 与 API 默认值的证据边界区分 |

## 对 Realand 实施的含义

保留可证实的 API 参数与 timing 曲线预设，以输出呈现时刻冻结动画采样；结合本机测量维护 latch 预算。Genie 的两段运动需要保证接缝位置和速度连续，不能因原方案把 `smoothstep` 的零导数放在内部接缝而产生人为停顿。弹簧结束条件需同时约束位置和速度，不能把一次穿过目标误判为收敛。

255 ms 窗口展开、450/300 ms 最小化、32×8 网格、超椭圆展开和交互弹簧预设属于本项目的实现选择；它们的可用性与正确性由 Realand 测试验证。测试通过不等于已经与运行中的 Catalina 逐帧/逐像素一致。真实 Dock 动画路径、完整桌面实测、多输出与高刷新率硬件验收仍需独立证据。

## 实际 Dock 路径的有界调查

本轮也向 REA 导入真实 Dock 可执行文件。procedure regex `genie|minimiz|magnif|suck` 和两个 string literal `genie`、`minimiz` 均无匹配；见 `dock-animation-procedures.json`（`ev_1bbe7bf4…`）、`dock-genie-strings.json`（`ev_1c6a848e…`）、`dock-minimize-strings.json`（`ev_b2ef28fa…`）。这些结果只描述本次分析数据库中的匹配情况，不能证明不存在该功能。

原始 Mach-O 的 `__TEXT` 段 flags 在文件 `0xac`，字节 `08 00 00 00`，即 `SG_PROTECTED_VERSION_1`；该标记的含义见 [Apple 的 loader.h](https://github.com/apple-oss-distributions/xnu/blob/main/EXTERNAL_HEADERS/mach-o/loader.h#L376-L380)。当前没有建立该保护段的可分析明文，因此没有继续把任意解码结果当成私有 Genie 算法。REA 的独立 load-image 校验对 Mach-O 明确返回 `unsupported`，不是验证成功，见 `dock-load-image.json`（`ev_08f5595d…`）。

本轮共保留 38 条 canonical Evidence，所有自有 REA/Ghidra 会话已关闭。真实 Dock 的曲线和时长仍需可分析的代码样本，或实际 macOS 逐帧观察。这个证据边界不影响 Realand 的接缝连续性、弹簧收敛与 GPU 同步错误修复。
