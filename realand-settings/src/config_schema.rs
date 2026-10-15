//! Configuration editor presentation using the public configuration schema.
pub use realand_config::schema::{fields, spec, FieldSpec};

#[derive(Clone, Copy)]
pub struct Section {
    pub name: &'static str,
    pub title: &'static str,
    pub ty: &'static str,
    pub sample: &'static str,
    pub repeated: bool,
}
macro_rules! sections {
    ($(($name:literal,$title:literal,$ty:literal,$sample:literal,$many:literal)),* $(,)?) => {
        pub const SECTIONS: &[Section] = &[$(Section {name:$name,title:$title,ty:$ty,sample:$sample,repeated:$many}),*];
    };
}
sections![
    ("input", "输入设备与键盘", "InputPart", "input {\n}", false),
    (
        "output",
        "显示器与缩放",
        "Output",
        "output \"eDP-1\" {\n}",
        true
    ),
    (
        "layout",
        "平铺与窗口布局",
        "LayoutPart",
        "layout {\n}",
        false
    ),
    (
        "renderer",
        "Vulkan / GLES 后端",
        "RendererPart",
        "renderer \"vulkan\"",
        false
    ),
    (
        "animations",
        "动画、立方体与窗口特效",
        "AnimationsPart",
        "animations {\n}",
        false
    ),
    ("blur", "背景模糊", "BlurPart", "blur {\n}", false),
    (
        "gestures",
        "手势与热角",
        "GesturesPart",
        "gestures {\n}",
        false
    ),
    (
        "overview",
        "概览风格与外观",
        "OverviewPart",
        "overview {\n}",
        false
    ),
    ("binds", "快捷键与操作", "Binds", "binds {\n}", false),
    (
        "window-rule",
        "窗口匹配与规则",
        "WindowRule",
        "window-rule {\n    match app-id=\"^your-app$\"\n}",
        true
    ),
    (
        "layer-rule",
        "面板与图层规则",
        "LayerRule",
        "layer-rule {\n    match namespace=\"^your-panel$\"\n}",
        true
    ),
    (
        "workspace",
        "命名工作区",
        "Workspace",
        "workspace \"新工作区\"",
        true
    ),
    (
        "spawn-at-startup",
        "自启动程序",
        "SpawnAtStartup",
        "spawn-at-startup \"true\"",
        true
    ),
    (
        "spawn-sh-at-startup",
        "自启动命令",
        "SpawnShAtStartup",
        "spawn-sh-at-startup \"true\"",
        true
    ),
    (
        "environment",
        "环境变量",
        "Environment",
        "environment {\n}",
        false
    ),
    ("cursor", "鼠标光标", "CursorPart", "cursor {\n}", false),
    (
        "clipboard",
        "剪贴板",
        "ClipboardPart",
        "clipboard {\n}",
        false
    ),
    (
        "screenshot-path",
        "截图保存路径",
        "ScreenshotPath",
        "screenshot-path \"~/Pictures/Screenshots/%Y-%m-%d-%H-%M-%S.png\"",
        false
    ),
    (
        "prefer-no-csd",
        "客户端窗口装饰",
        "Flag",
        "prefer-no-csd",
        false
    ),
    (
        "recent-windows",
        "最近窗口切换器",
        "RecentWindowsPart",
        "recent-windows {\n}",
        false
    ),
    (
        "hotkey-overlay",
        "快捷键提示面板",
        "HotkeyOverlayPart",
        "hotkey-overlay {\n}",
        false
    ),
    (
        "config-notification",
        "配置错误通知",
        "ConfigNotificationPart",
        "config-notification {\n}",
        false
    ),
    (
        "switch-events",
        "合盖与开关事件",
        "SwitchBinds",
        "switch-events {\n}",
        false
    ),
    (
        "xwayland-satellite",
        "X11 应用兼容",
        "XwaylandSatellitePart",
        "xwayland-satellite {\n}",
        false
    ),
    ("debug", "高级与调试", "DebugPart", "debug {\n}", false),
    (
        "include",
        "导入其他配置文件",
        "Include",
        "include \"extra.kdl\" optional=true",
        true
    ),
];
pub fn section(name: &str) -> Option<&'static Section> {
    SECTIONS.iter().find(|s| {
        s.name
            == if name == "renderer-backend" {
                "renderer"
            } else {
                name
            }
    })
}
pub fn scalar_default(module: &str, ty: &str) -> String {
    match ty {
        "bool" => "true".into(),
        "String" | "PathBuf" | "RegexEq" => "\"\"".into(),
        "Color" => "\"#ffffff\"".into(),
        "f32" | "f64" => "1.0".into(),
        "ColumnDisplay" => "\"normal\"".into(),
        "FloatOrInt" | "u8" | "u16" | "u32" | "u64" | "usize" | "i32" | "i64" => "1".into(),
        "Transform" => "\"normal\"".into(),
        "Curve" => "\"ease-out-cubic\"".into(),
        _ => spec(module, ty)
            .and_then(|s| s.variants.first())
            .map(|v| format!("\"{v}\""))
            .unwrap_or_else(|| "\"\"".into()),
    }
}
pub fn value_default(name: &str, module: &str, ty: &str) -> String {
    match name {
        "mode" => "\"1920x1080@60\"".into(),
        "scale" | "opacity" => "1.0".into(),
        "proportion" => "0.5".into(),
        "fixed" => "800".into(),
        "max-bpc" => "8".into(),
        "center-focused-column" => "\"never\"".into(),
        "rounding-power" => "2".into(),
        "epsilon" => "0.0001".into(),
        "stiffness" => "1000".into(),
        _ => scalar_default(module, ty),
    }
}
pub fn sample(module: &str, field: FieldSpec) -> String {
    let name = field.name;
    if field.ty == "SpringParams" {
        return format!("{name} damping-ratio=1.0 stiffness=1000 epsilon=0.0001");
    }
    if name == "mesh" && module == "animations" {
        return format!("{name} 16");
    }
    if field.ty == "Modeline" {
        return format!(
            "{name} 148.5 1920 2008 2052 2200 1080 1084 1089 1125 \"+hsync\" \"+vsync\""
        );
    }
    if field.ty == "CornerRadius" {
        return format!("{name} 8.0");
    }
    if field.ty == "SwitchAction" {
        return format!("{name} {{ spawn \"true\"; }}");
    }
    if matches!(field.ty, "Flag" | "bool") && !field.scalar {
        return name.into();
    }
    if matches!(
        name,
        "mode"
            | "scale"
            | "opacity"
            | "proportion"
            | "fixed"
            | "max-bpc"
            | "center-focused-column"
            | "rounding-power"
            | "epsilon"
            | "stiffness"
    ) {
        return format!("{name} {}", value_default(name, module, field.ty));
    }
    if field.scalar {
        return format!("{name} {}", value_default(name, module, field.ty));
    }
    if field.ty == "Color" {
        return format!("{name} \"#ffffff\"");
    }
    let fields = fields(module, field.ty);
    let mut result = name.to_string();
    for f in &fields {
        if !f.optional && f.kind == "argument" {
            result.push_str(&format!(" {}", value_default(f.name, module, f.ty)));
        }
        if !f.optional && f.kind == "property" {
            result.push_str(&format!(
                " {}={}",
                f.name,
                value_default(f.name, module, f.ty)
            ));
        }
    }
    if fields.iter().any(|f| f.kind == "child")
        || field.ty.ends_with("Anim")
        || matches!(field.ty, "DefaultPresetSize" | "PresetSize")
    {
        result.push_str(" {\n}");
    }
    result
}
pub fn label(name: &str) -> &str {
    match name {
        "keyboard" => "键盘",
        "touchpad" => "触摸板",
        "mouse" => "鼠标",
        "tablet" => "数位板",
        "touch" => "触摸屏",
        "trackpoint" => "指点杆",
        "trackball" => "轨迹球",
        "mode" => "分辨率与刷新率",
        "scale" => "缩放比例",
        "transform" => "旋转与翻转",
        "position" => "显示器位置",
        "variable-refresh-rate" => "可变刷新率",
        "gaps" => "窗口间距",
        "border" => "窗口边框",
        "focus-ring" => "焦点高亮",
        "shadow" => "窗口阴影",
        "off" => "关闭",
        "on" => "开启",
        "repeat-rate" => "按键重复速率",
        "repeat-delay" => "按键重复延迟",
        "xkb" => "键盘布局",
        "match" => "匹配条件",
        "content-type" => "应用内容类型",
        "exclude" => "排除条件",
        "window-wobble" => "窗口弹性",
        "workspace-switch" => "工作区切换",
        "window-open" => "窗口打开",
        "window-close" => "窗口关闭",
        "cube" => "工作区立方体",
        "glide" => "窗口滑翔",
        _ => name,
    }
}
