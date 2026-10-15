# Nourish 风格无限画布与二维平移

当前工作区可切换为自由浮动画布。窗口可以位于屏幕外，平移没有窗口边缘回弹或列吸附。普通工作区继续使用 Realand 原有的滚动平铺、Dwindle 和手势。

## 使用

默认配置新增：

```kdl
binds {
    Mod+Ctrl+V repeat=false { toggle-canvas; }
    Mod+Shift+Home { reset-canvas; }
    // 可选键盘平移：正 x 向右查看，正 y 向下查看。
    Mod+Alt+Right { pan-canvas 200 0; }
    Mod+Alt+Left  { pan-canvas -200 0; }
    Mod+Alt+Down  { pan-canvas 0 200; }
    Mod+Alt+Up    { pan-canvas 0 -200; }
}
```

已有用户配置不会被自动改写，请将需要的绑定加入配置。也可使用 IPC：

```sh
realand msg action toggle-canvas
realand msg action pan-canvas 200 100
realand msg action reset-canvas
```

启用后：
- Mod + 鼠标中键拖动：二维直接平移，画布跟随拖动。
- Mod + 滚轮或触控板双指滚动：移动视口；触控板释放后以摩擦和恒定减速度滑行至停止。普通双指滚动仍交给应用。
- 新窗口在当前可见区域以浮动窗口打开。
- 再次切换退出画布：视口归零，将进入画布前的平铺窗口及画布内新建的普通窗口放回当前平铺引擎；不保证重建原来的列分组和分割树。
- reset-canvas 返回进入画布时的视口原点。

## 实现与范围

采用 Nourish 的二维相机平移及“指数摩擦 + 恒定减速度”模型，适配 Realand 的浮动布局、已有输入路由和渲染系统。Realand 用浮动窗口坐标的统一平移表示相机移动，因此渲染、鼠标命中、弹窗约束和 IPC 屏幕位置共享同一坐标路径。关闭动画也随视口移动。

该移植仅涵盖无限滚动和二维平移，不包含 Nourish 的缩放、世界导航器及分割视口。画布按工作区独立保存于运行时；窗口原有大小约束仍有效。为避免渲染整数溢出，相机偏移具有 ±1000 万逻辑像素的数值保护边界。

## 上游与许可证

参考源：https://github.com/y5-snowies/nourish

固定提交：`d6893ce06974204b04a5aad4521a5847ad58e61d`

相关实现：
- `compositor.expansion/compositor.y5/y5.camera/camera.system/system.base/base.rs`：`PanInertiaTick`，摩擦 3.6/s、恒定减速度 400、停止阈值 8。
- `compositor.orchestration/orchestration.seat/seat.pointer/pointer.pan/pan.rs`：指针与画布坐标的同步原则。

上游 Copyright (c) 2026 Yarden Apelker，MIT OR Apache-2.0。此适配采用 MIT 许可，上游完整许可保存在 `resources/licenses/nourish-MIT.txt`。Realand 集成代码遵循项目许可。
