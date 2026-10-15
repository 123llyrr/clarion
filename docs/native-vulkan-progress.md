# 独立 Vulkan 渲染链路

> 本文件保存实现过程中的历史记录，各段测试数量和“尚未支持”的说明对应当时版本。当前架构、能力边界和可重复验证步骤以 [Vulkan backend validation](vulkan-validation.md) 为准。原生 TTY/WSI、跨 GPU 线性 DMA-BUF、按损伤复制 SHM、异步上传及多平面 YUV 均已进入实际代码；是否可用由驱动能力探测决定。离屏测试不替代真实 KMS、热插拔、休眠恢复和长期运行验收。

目标是 Vulkan 模式下由 Vulkan 完成客户端纹理导入、整个场景与特效渲染、输出提交和截图/录屏所需的图像处理，不创建 GLES 场景渲染器。现有混合后端和仅渲染窗口主表面的实现都不满足完成条件。

## 当前推进

- 已添加无需 EGL 的 CPU 内存纹理上传，覆盖 RGBA/BGRA 和无 alpha 的 RGBX/BGRX，验证输入长度、行跨度、溢出以及设备能力。源内存立即可释放，纹理由 Vulkan 拥有。
- `VulkanTexture` 已实现 Smithay 的 `Texture` 元数据接口。
- 原生帧已支持按绘制顺序局部清除和物理坐标裁剪：透明清除替换旧像素，清除与裁剪均约束在帧损伤范围内，空/越界裁剪禁止绘制。
- `native::NativeRenderer` 已实现 Smithay `Renderer`/`Frame`、原生纹理与 DMA-BUF 绑定、带 alpha 的离屏目标。支持源裁切、纹理过滤、旋转/翻转和局部重绘；对重复损伤去重以防半透明像素被重复混合。DMA-BUF 目标复用现有有界导入缓存。
- 新硬件回归直接执行内存上传 → Vulkan 绘制 → DMA-BUF 目标，并通过独立像素读回检查行跨度、通道顺序与透明度。测试中的 GLES 仅作读回参照，不上传源纹理或渲染被测场景。
- 新增 `native_frame_scissor_and_ordered_transparent_clear` 硬件测试，验证透明/不透明背景上的半透明混合、穿插清除、空裁剪与裁剪复位；Intel 和 NVIDIA 的当前 Vulkan 套件各 17 项通过，严格 Clippy 通过。
- 原生 Smithay 适配器增加非正方形输出、源图裁切以及 8×8 种源/输出变换的 GLES 像素对比，另验证离屏纹理重采样、重叠损伤只混合一次和非法目标拒绝。已修正翻转四分之一圈的点/矩形坐标差异及帧尺寸契约；最新 Intel、NVIDIA 套件各 19 项通过，验证层及严格 Clippy 无错误。


- 原生帧保留多个生产者信号，通过 GPU 等待可导出的 SYNC_FD，修复连续等待覆盖前一信号的问题。双生产者及取消帧回归验证零生产者 CPU 等待和资源清理；不可导出信号仍有 CPU 回退；原生帧结束同步已在后续异步完成点实现中改造（见下文）。
- 原生适配器已实现 `ImportDma` 和 DMA-BUF 目标格式反馈，直接查询 Vulkan 的显式 DRM modifier、采样/线性过滤、颜色混合和外部内存导入能力，初始化后缓存格式集合，无需 EGL 提供格式列表。具体缓冲区仍验证布局及辅助平面共享分配；尚不支持 YUV 和独立多平面分配。
- 新增硬件回归从 Vulkan 反馈分配四种 8 位 RGB DMA-BUF，执行原生绑定、绘制、缓存导入、再次绘制及独立像素读回。Intel、NVIDIA 最新 Vulkan 套件各 21 项通过，验证层无错误。日志位于 `outputs/native-vulkan-dma`（本次 Codex 工作目录）。

内存纹理上传已改为 staging buffer → optimal-tiling 设备本地图像，不再要求线性采样图像支持。新增按完整源图坐标局部更新，保持纹理句柄及未修改像素，校验上下文、外部图像、数据长度和矩形边界；上传资源在实际 GPU 完成后回收，失败路径有资源清理。四种 8 位 RGB 格式的绘制后更新及共享句柄回归通过，Intel、NVIDIA 最新套件各 22 项通过且验证层无错误（`outputs/native-vulkan-staging`）。当前每次上传仍同步提交等待，尚需批次合并/异步回收。原生适配器现已实现 `ImportMem` 导入、更新和能力反馈，纹理句柄保留纵向翻转属性，局部更新继续使用原始源内存坐标。

`ImportMem` 回归覆盖局部更新后的两种纵向翻转状态及 8×8 源/输出变换，共 128 组像素比较；Intel、NVIDIA 各 22 项套件通过且验证层无错误（`outputs/native-vulkan-importmem`）。当前 Smithay GLES 的 `y_inverted` 矩阵仅取负纵坐标、导致边缘夹取，因此翻转参照使用显式反转源行后以非翻转 GLES 上传，验证预期翻转而不复现该异常。

已实现 `ImportMemWl` 和 `ImportDmaWl`：SHM 通过 Smithay 的受保护原始指针访问复制自有像素，检查偏移、行跨度及长度，避免创建指向客户端可写内存的 Rust 切片。每个表面按渲染上下文缓存纹理，正确处理轮换缓冲区、局部损伤、首次空损伤和格式/尺寸变化；目前每次仍复制完整源图，损伤上传逐区域同步提交，后续应合并上传批次。真实 Wayland socket、SHM pool、buffer 和 surface 回归在 Intel、NVIDIA 各 23 项套件中通过，验证层无错误（`outputs/native-vulkan-shm`）。原生 `ImportAll` 现已显式分发 SHM 和 DMA-BUF，无需实现 `ImportEgl`；未知及非纹理缓冲区按接口约定返回 `None`。

进一步验证真实 `wl_surface` 提交、同步子表面位置与父提交，通过 Smithay `render_elements_from_surface_tree` 和标准 `RenderElement::draw` 直接进入原生 Vulkan，逐像素验证父/子表面的遮挡顺序和输出裁剪。Intel、NVIDIA 各 23 项套件通过且验证层无错误（`outputs/native-vulkan-surface-tree`）。这仍是基础表面集成验证，不能等同于 Realand 的完整窗口、特效及 TTY 场景接入。

原生离屏目标现已支持 `Xrgb8888`、`Xbgr8888`、`Xrgb2101010` 和 `Xbgr2101010`。同一不透明图像使用 identity 附件视图与 alpha-one 采样视图，纹理描述符使用采样视图，两个视图随纹理资源共同清理。硬件回归故意写入零/部分物理 alpha，反复切换附件绘制和采样并检查背景不透出；Intel、NVIDIA 各 24 项套件通过且验证层无错误（`outputs/native-vulkan-opaque`）。

原生 `ExportMem` 已实现纹理/帧缓冲区区域读取、可读性查询和自有内存映射；通过 Vulkan image-to-buffer 复制读回，恢复原布局及外部图像 FOREIGN 所有权，再提供紧密排列的 8/10 位 RGB 数据、通道转换和不透明 alpha 修正。映射保留翻转元数据且独立于源纹理寿命；内存上传纹理现在支持 TRANSFER_SRC。八种 RGB 格式、非零裁切、非法区域、映射保持、读取后采样以及 DMA-BUF 读取后重绘在 Intel、NVIDIA 各 25 项套件中通过，验证层无错误（`outputs/native-vulkan-readback`）。当前读回和格式转换同步完成，截图/录屏的完整业务接入与异步流水线仍待实现。

`VulkanTexture` 已使用 `Arc` 共享图像所有权、原子布局元数据和互斥描述符缓存，自动满足 `Send + Sync`，未添加 unsafe 线程标记。描述符池分配和释放共用互斥锁，避免跨线程销毁与分配并发违反 Vulkan 外部同步要求。8 个线程处理 512 个纹理、1024 个独立描述符并并发复用共享描述符，主线程持续绘制，渲染器销毁后最后一个纹理在另一线程释放；Intel、NVIDIA 各 26 项套件通过且验证层无错误（`outputs/native-vulkan-texture-send`）。这解决了现有场景对纹理 `Send` 的类型要求，尚未移除场景其余 GLES 依赖。

Realand 的 `render_snapshot_from_surface_tree` 已从 `GlesRenderer/GlesTexture` 改为通用 `Renderer + ImportAll`，保留既有 GLES 调用行为，并为 Vulkan `BakedBuffer<TextureBuffer<_>>` 实现原生纹理渲染元素转换。真实父子表面生成快照后发送销毁请求，快照依然通过原生 Vulkan 绘制，逐像素验证位置和遮挡顺序；Intel、NVIDIA 各 26 项套件通过且验证层无错误（`outputs/native-vulkan-snapshot`）。此步骤打通基础表面快照；窗口动画及特效中的 GLES 类型仍需继续泛化/移植。


`create_texture`、`render_to_texture`、`render_to_encompassing_texture` 和内部元素合成已接受通用原生离屏纹理；`RenderSnapshot<C, B, T = GlesTexture>` 的三类懒缓存可使用 Vulkan 纹理，既有 GLES 调用继续通过默认类型工作。硬件回归通过原生 Vulkan 验证负坐标包围框重定位、绘制顺序、普通/背景遮挡/完整隐私遮挡分支及缓存复用。Intel、NVIDIA 各 27 项套件通过，验证层无错误（`outputs/native-vulkan-lazy-snapshot`）。当前快照仍沿用既有单渲染上下文、首次尺度缓存语义；完整窗口动画和多 GPU 场景缓存迁移尚需继续。

`copy_framebuffer`、`render_and_download` 和 `render_to_vec` 已改为通用 `ExportMem`、离屏纹理和帧缓冲区接口，原生 Vulkan 可直接走 Realand 的合成到字节输出辅助路径。硬件回归对比非正方形输出下全部 8 种变换、ARGB/ABGR 通道顺序和半透明叠加；Intel、NVIDIA 各 28 项套件通过，验证层无错误（`outputs/native-vulkan-download`）。实际截图/录屏入口和 TTY 渲染器选择尚未迁移。

`render_to_dmabuf` 与 `clear_dmabuf` 已接受通用 `Renderer + Bind<Dmabuf>`。新回归先通过 Realand 辅助函数完整绘制，再用 Smithay `OutputDamageTracker` 和明确的 buffer age=1 验证元素移动、内容变化和无变化帧，在 8 种输出变换下与 GLES 逐像素一致，并验证原生清屏。Intel、NVIDIA 各 29 项套件通过，验证层无错误（`outputs/native-vulkan-damage-output`）。捕获辅助函数自身继续使用 age=0，因为调用方未提供可证明有效的缓冲区历史；不能将该接口泛化宣称为 TTY 局部提交已接入。

`render_to_shm` 已改为通用原生渲染/导出接口，支持 XRGB8888 缓冲区非零偏移与行填充，检查尺寸、溢出和池边界，仅逐行写入有效像素，并处理导出映射翻转。协议缓冲区在 GPU 工作前校验，GPU 合成/读回移出 SHM 映射保护范围，最终写入时再次检查边界。真实 Wayland SHM 池验证前缀/行填充/尾部哨兵保持、错误格式无写入，以及 Vulkan/GLES 相同像素输出；Intel、NVIDIA 各 29 项套件通过且验证层无错误（`outputs/native-vulkan-shm-output`）。此路径仍使用同步读回和完整重绘，实际捕获业务的原生场景选择仍待接入。


原生帧现已实现可导出的异步 GPU 完成点：查询 SYNC_FD 导出能力，提交信号 semaphore 并导出独立完成 FD，保留在途命令、生产者/完成 semaphore、管线和源/目标纹理。渲染器回收已完成提交，最多保留 8 个未完成帧并在超限时施加背压；导出/提交失败路径也保留目标直到 GPU 清理完成。原有混合后端的同步接口保留。最终回归使用已提交的 GPU 填充工作负载，观察生产者与目标帧都未完成时 `finish()` 已返回，验证重复 FD 导出、完成后像素和渲染器销毁后的 FD 寿命。此前两种不合法的测试同步构造已替换，不能把其像素通过视为验证通过。最新 Intel、NVIDIA 各 30 项套件通过且验证层无错误（`outputs/native-vulkan-async`）；KMS 接线、多屏提交与设备恢复仍需继续。

原生适配器尚未接入 TTY 场景选择和 KMS 的实际 fence 提交。在支持 SYNC_FD 导出的设备上，原生 `Frame::finish` 已异步返回真实完成点；不支持导出的设备仍同步回退。上传/读回、队列背压和销毁仍可能等待，不能宣称所有 CPU 等待或完整显示链路已经移除。

配置通知已增加通用 `render_with` 场景入口，文字/边框纹理通过所选渲染器的 `ImportMem` 上传并返回自身纹理元素；缓存按纹理类型、渲染上下文和分数尺度隔离，消息切换清空缓存，现有 GLES 桌面入口委托同一实现。新增回归在同一通知对象上交替使用 Vulkan/GLES，覆盖创建成功与配置错误消息、1/1.5/2 倍缩放、上传缓存复用和隐藏，实际可见输出逐像素一致（容差 2）。Intel、NVIDIA 各 31 项套件通过，验证层和严格 Clippy 通过（`outputs/native-vulkan-notification`、`outputs/native-notification-clippy.log`）。这只完成配置通知自身的原生接口，TTY 完整场景选择及其他 UI/特效仍未接入，不能据此宣称独立后端完成。

快捷键面板和退出确认框已增加原生通用 `render_with` 入口，快捷键纹理缓存按后端纹理类型、上下文和输出隔离，尺度变化及配置更新会重建。退出框共享场景准备逻辑，保留文字缩放动画、透明度和后方半透明遮罩，新增按上下文/源尺度缓存的文字纹理，现有 GLES 路径同样不再每帧重复上传静态文字。硬件回归在 1/1.5/2 倍尺度比较快捷键与退出动画中间状态的 Vulkan/GLES 像素（容差 2），验证缓存复用、配置刷新、背景存在和隐藏后无元素。Intel、NVIDIA 各 32 项套件通过，验证层及严格 Clippy 通过（`outputs/native-vulkan-ui`、`outputs/native-ui-clippy.log`）。这些 UI 的原生入口已可用，但生产合成器仍通过原 GLES 包装入口调用，完整场景聚合、MRU/窗口特效和 TTY/KMS 仍待迁移。

实际 `BorderRenderElement` 已实现原生 Vulkan `RenderElement`，保留元素 ID/损伤语义并直接提交 Vulkan 边框管线。新增几何坐标范围支持边框子区域和归一化源裁切，原生帧处理输出变换与去重损伤；原有低层边框接口的默认范围仍为完整几何。首轮验证发现新增 vec4 的 Rust/GLSL 对齐不匹配，已补齐至 192 字节并检查字段偏移 176；随后像素对比暴露原 Vulkan 的 sRGB 透明混合及 OKLab 转换差异，已将颜色插值公式与现有场景实现一致化。最终回归覆盖四种颜色空间、四种色相路径、八种输出变换（128 组）、非对称圆角、半透明边框、源裁切和重复损伤，像素容差保持 3。Intel、NVIDIA 各 33 项套件通过且验证层无错误，SPIR-V 校验及严格 Clippy 通过（`outputs/native-vulkan-border-parity`、`outputs/native-border-clippy.log`）；先前失败日志不作为通过依据。边框原生绘制已可用，但布局的 shader 能力判断及聚合元素仍有 GLES 约束，阴影、背景效果与完整 TTY 场景接线尚未完成。

实际 `ShadowRenderElement` 已实现原生 Vulkan 绘制，新增解析圆角阴影管线，保留预乘颜色、模糊公式、窗口内部挖空、元素裁切和 `with_alpha` 动画覆盖。阴影使用 176 字节参数，复用不含描述符的边框管线布局，按附件格式创建并由现有构建失败/销毁保护释放；原生帧对重复损伤去重。硬件回归发现既有零模糊路径的圆角函数在矩形外可能返回不透明，且直边及挖空硬边界在旋转时对浮点误差敏感；已在 GLES/Vulkan 两侧补齐相同的直边抗锯齿覆盖率，修复伪影而未放宽像素容差。最终比较 0/0.05/1.5/5 模糊半径、有/无挖空、八种变换（64 组），另验证源裁切、重复损伤和透明度覆盖，像素容差 3。Intel、NVIDIA 各 34 项套件通过且验证层无错误，SPIR-V 与严格 Clippy 通过（`outputs/native-vulkan-shadow-parity`、`outputs/native-shadow-clippy.log`）；此前失败日志不作为通过依据。当前模糊阴影仍沿用原算法仅使用左上圆角半径的行为；布局层的 shader 可用性判断、背景效果与 TTY/KMS 场景集成尚需继续。

布局焦点环/边框和阴影现已接受独立 `DecorationRenderer` 能力接口，Vulkan 不再通过 GLES shader 缓存进行这两类布局效果的可用性判断；既有 GLES 调用沿用原 shader 检查。`FocusRingRenderElement` 聚合类型已可直接由原生 Vulkan 绘制。新增回归执行实际布局生成逻辑，覆盖活动/紧急/非活动状态、工作区相对旋转渐变、圆角多段边框、阴影、1/1.5/2 倍尺度及关闭配置，并断言渐变不会被替换为纯色。NVIDIA 首轮暴露 4.5 像素内边框的硬裁切不稳定，已在两后端以直边覆盖率和圆角遮罩相交修复；像素容差仍为 3。最终 Intel、NVIDIA 各 35 项套件通过且验证层无错误，SPIR-V 及严格 Clippy 通过（`outputs/native-vulkan-decorations-fixed`、`outputs/native-decorations-clippy.log`）。完整窗口/图层聚合、背景特效和 TTY/KMS 接线仍未完成。

标签指示器和拖放插入提示布局入口已改用 `DecorationRenderer`，`TabIndicatorRenderElement` 聚合类型现可直接由原生 Vulkan 绘制，插入提示复用已迁移的焦点环元素。新增硬件回归执行实际布局生成，覆盖四个方向、1/1.5/2 倍尺度、连体/分隔圆角、打开动画中间状态、活动/非活动透明度、工作区相对渐变，以及隐藏单标签和关闭插入提示；逐像素容差 3。Intel、NVIDIA 各 36 项套件通过且验证层无错误，严格 Clippy 通过（`outputs/native-vulkan-tabs`、`outputs/native-tabs-clippy.log`）。这继续减少布局 GLES 依赖，但完整窗口/图层聚合、圆角表面裁切、背景特效和 TTY/KMS 接线仍待完成。

`ClippedSurfaceRenderElement` 现通过 `ClipRenderer` 的关联程序类型兼容原生 Vulkan，原生实例使用 `()` 而非 GLES 程序句柄。Vulkan 纹理绘制新增可选矩阵/独立圆角遮罩，复用表面裁切坐标计算，保留父子表面的矩形裁切和圆角透明度；参数总长 192 字节并检查遮罩偏移 112。原生绘制结束会恢复先前遮罩，GLES 绘制失败时也清除覆盖程序，防止状态影响后续元素。真实 Wayland 父/子表面回归覆盖 8 种客户端缓冲区变换 × 8 种输出变换 × 3 种尺度（192 组），不对称圆角与半透明叠加逐像素容差 3。测试客户端最初因绑定 v1 后调用 v2 变换请求被断开，已修正为 v2；该失败不作为渲染结论。最终 Intel、NVIDIA 各 36 项扩展套件通过且验证层无错误，SPIR-V 与严格 Clippy 通过（`outputs/native-vulkan-clip-transforms-v2`、`outputs/native-clip-clippy.log`）。当前验证为 SHM 表面，viewporter 特殊裁切及外部 DMA-BUF 纵向翻转仍需进一步覆盖；完整窗口/图层聚合、背景特效、TTY/KMS 接线尚未完成。

新增原生 `NativeBlur` 和 dual-Kawase Vulkan 管线，沿用现有降采样/升采样公式、采样偏移、1–31 级参数和 ABGR8888 中间图像。各 pass 在同一队列执行，返回最终真实完成点；在途源/目标和管线由提交资源保留，用户仍持有的旧输出不被覆盖，中间纹理复用。修正缓存复用判断前未清理已完成提交引用造成的不必要分配，现先无阻塞回收已完成提交，再判断输出是否独占。回归覆盖奇数/单像素宽尺寸、1/3/31 级、偏移 0/1.25、半透明输入和纵向翻转，逐像素容差 3，并验证旧结果保持和已释放输出的图像句柄复用。上一轮进程句柄已丢失且日志不完整，未据此宣称通过；恢复后 Intel、NVIDIA 各 37 项整套测试通过且验证层无错误，严格 Clippy 和 SPIR-V 校验通过（`outputs/native-vulkan-blur-recovery`、`outputs/native-blur-clippy.log`）。当前每个 pass 仍独立提交，渲染器队列背压可能等待；这完成原生模糊核心，背景效果的帧缓冲捕获、噪声/饱和度、场景聚合与 TTY/KMS 接线仍需继续。

原生帧新增按绘制顺序的 `capture_into`：在同一命令缓冲区结束当前动态渲染作用域，将指定附件区域复制到原生纹理，经显式传输/采样屏障后以 LOAD 恢复绘制。后续绘制可立即采样捕获图像，不增加队列提交或 CPU 等待；捕获图像由异步完成点保留。接口使用未变换的附件像素坐标，当前仅支持等尺寸、同格式且透明度语义一致的私有目标，拒绝越界、外部目标、当前附件别名以及同帧已采样/已捕获的目标。新增硬件回归覆盖八种输出变换、非零区域、半透明像素、捕获后清除、同帧采样再捕获和跨帧纹理复用，与 GLES 独立读回/采样逐像素比较（容差 2）。最终 Intel、NVIDIA 各 38 项套件通过，验证层及严格 Clippy 通过（`outputs/native-vulkan-capture-final`、`outputs/native-capture-clippy-final.log`）。这是背景捕获的底层基础；缩放/格式转换捕获、模糊中间渲染作用域、背景效果元素接入及完整 TTY/KMS 场景仍未完成，生产桌面路径未切换。

原生模糊现可在已有帧内执行：`prepare_in_frame` 在借用帧前准备图像，返回一次性 `PreparedBlur`，捕获后按顺序记录全部降采样/升采样，再由后续场景绘制采样结果。新增离屏模糊作用域及附件写入/采样屏障，切回主附件时保留内容、恢复视口和当前裁剪；异步完成点保留所有中间图像。独立 `NativeBlur::render` 也改为同一命令缓冲区的一次提交，取代原先每 pass 一次提交，因而不会在同一模糊任务的 1–31 级间触发队列背压等待。扩展既有模糊回归，真实执行背景绘制→非零区域捕获→帧内模糊→合成，只在末尾 finish；覆盖奇数/单像素尺寸、1/3/31 级、两种偏移、半透明及源翻转，模糊结果和合成区域与 GLES 比较（容差 3），检查周边透明像素保持，并继续验证旧输出及纹理复用。最终 Intel、NVIDIA 各 38 项扩展套件通过，验证层及严格 Clippy 通过（`outputs/native-vulkan-batched-blur`、`outputs/native-batched-blur-clippy.log`）。仍需缩放/格式转换捕获、噪声/饱和度处理、实际背景效果元素及完整场景/TTY/KMS 接线；本项未切换生产桌面后端。

新增原生 `capture_scaled_into`，支持非零附件区域的线性缩放与 RGB 格式转换，检查区域范围、用途、设备 blit/filter 能力和透明度语义。NVIDIA 首轮直接 blit 在裁切边缘与 GLES 相差最高 43/255，未作为通过依据；定位为裁切区域外像素参与过滤的差异，最终明确采用“精确复制裁切区域→缩放临时图像”，保证采样不越过区域边缘。DMA-BUF 仅做传输复制，线性过滤在已检查能力的 optimal 图像上执行，不依赖 DRM modifier 的 blit 支持；临时图像由异步完成点保留，不增加提交/等待。新回归对比显式先裁切的 GLES 参照，覆盖普通附件与真实 DMA-BUF、放大/缩小/1×1/同尺寸、ARGB→ABGR 及同格式、半透明源、越界拒绝与捕获后清除，像素容差仍为 3。最终 Intel、NVIDIA 各 39 项套件及验证层通过，严格 Clippy 通过（`outputs/native-vulkan-scaled-capture-cropped`、`outputs/native-scaled-capture-cropped-clippy.log`）。原 GLES 背景元素仍直接 blit，其边缘行为尚未统一；不能据新参照宣称实际背景元素已完全同像素。当前缩放捕获每次分配临时裁切图像，缓存复用、不透明附件到带 alpha 中间图像的转换、噪声/饱和度和实际背景效果/TTY/KMS 接入仍待完成。

## 必须完成的后续集成与验收

1. 实现 Smithay 原生 `Renderer`/`Frame`、绑定/离屏、DMA-BUF/共享内存导入和图像导出接口，正确处理变换、裁剪、损伤、混合、纹理过滤及资源寿命。
2. 移除 Vulkan 路径上的 `AsGlesRenderer`、`AsGlesFrame`、`Offscreen<GlesTexture>` 等硬性依赖。窗口及子表面、弹出层、layer-shell、光标、概览和界面元素均进入同一 Vulkan 场景。
3. 移植现有 GLES 特效、动画、自定义着色器与截图/录屏处理，保持功能和绘制顺序；不能以静默关闭效果代替移植。
4. 为 DRM 提交 Vulkan 自己的完成同步，正确传递生产者和 KMS fence；多显卡导入、错误恢复、销毁和在途资源管理需要独立验证。不能把每帧 CPU 等待 GPU 完成当作最终性能方案。
5. 真实硬件验证全部场景、缩放/旋转、多屏热插拔、休眠唤醒、录屏以及长时间运行；验证层无错误。确认 Vulkan 模式不经过 EGL/GLES 场景路径，并与原场景输出进行像素或图像对比。

未完成以上集成和验证前，生产 TTY 路径仍是原混合实现，不更改运行中的桌面，不宣布独立 Vulkan 后端已完成。
