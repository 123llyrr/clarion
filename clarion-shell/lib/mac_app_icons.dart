// SPDX-License-Identifier: GPL-3.0-or-later
import 'models.dart';

/// Shell role icons for applications with the same desktop role, drawn by
/// the WhiteSur icon theme (see assets/icons/README.md).
///
/// Matching uses desktop IDs and freedesktop icon names, never translated
/// display names. An unrelated application whose name contains "terminal" or
/// "finder" must keep its own identity.
class MacAppIcons {
  MacAppIcons._();

  static const genericApplication = 'assets/icons/generic-application.svg';

  // flutter_svg cannot render these upstream SVGs: each one uses a <filter>,
  // so the shipped form is a librsvg-rasterized PNG (see assets/icons).
  static const _rasterIcons = {'activity-monitor', 'preview', 'screenshot'};

  static String _assetPath(String icon) =>
      'assets/icons/$icon.${_rasterIcons.contains(icon) ? 'png' : 'svg'}';

  static const _desktopIds = <String, String>{
    'files': 'home',
    'finder': 'home',
    'com.apple.finder': 'home',
    'dolphin': 'home',
    'org.kde.dolphin': 'home',
    'nautilus': 'home',
    'org.gnome.nautilus': 'home',
    'thunar': 'home',
    'nemo': 'home',
    'pcmanfm': 'home',
    'pcmanfm-qt': 'home',
    'org.lxqt.pcmanfm-qt': 'home',
    'terminal': 'terminal',
    'com.apple.terminal': 'terminal',
    'alacritty': 'terminal',
    'org.alacritty.alacritty': 'terminal',
    'org.gnome.terminal': 'terminal',
    'gnome-terminal': 'terminal',
    'konsole': 'terminal',
    'org.kde.konsole': 'terminal',
    'kitty': 'terminal',
    'foot': 'terminal',
    'footclient': 'terminal',
    'xterm': 'terminal',
    'xfce4-terminal': 'terminal',
    'org.xfce.terminal': 'terminal',
    'textedit': 'textedit',
    'com.apple.textedit': 'textedit',
    'kate': 'textedit',
    'org.kde.kate': 'textedit',
    'org.gnome.texteditor': 'textedit',
    'gedit': 'textedit',
    'org.gnome.gedit': 'textedit',
    'mousepad': 'textedit',
    'org.xfce.mousepad': 'textedit',
    'kwrite': 'textedit',
    'org.kde.kwrite': 'textedit',
    'calculator': 'calculator',
    'com.apple.calculator': 'calculator',
    'gnome-calculator': 'calculator',
    'org.gnome.calculator': 'calculator',
    'kcalc': 'calculator',
    'org.kde.kcalc': 'calculator',
    'galculator': 'calculator',
    'calendar': 'calendar',
    'com.apple.ical': 'calendar',
    'gnome-calendar': 'calendar',
    'org.gnome.calendar': 'calendar',
    'preview': 'preview',
    'com.apple.preview': 'preview',
    'gwenview': 'preview',
    'org.kde.gwenview': 'preview',
    'evince': 'preview',
    'org.gnome.evince': 'preview',
    'okular': 'preview',
    'org.kde.okular': 'preview',
    'eog': 'preview',
    'org.gnome.eog': 'preview',
    'org.gnome.loupe': 'preview',
    'screenshot': 'screenshot',
    'com.apple.screencaptureui': 'screenshot',
    'spectacle': 'screenshot',
    'org.kde.spectacle': 'screenshot',
    'flameshot': 'screenshot',
    'org.flameshot.flameshot': 'screenshot',
    'org.gnome.screenshot': 'screenshot',
    'gnome-screenshot': 'screenshot',
    'fontbook': 'font-book',
    'com.apple.fontbook': 'font-book',
    'gnome-font-viewer': 'font-book',
    'org.gnome.font-viewer': 'font-book',
    'gfontview': 'font-book',
    'kfontview': 'font-book',
    'org.kde.kfontview': 'font-book',
    'activity-monitor': 'activity-monitor',
    'com.apple.activitymonitor': 'activity-monitor',
    'gnome-system-monitor': 'activity-monitor',
    'org.gnome.systemmonitor': 'activity-monitor',
    'ksysguard': 'activity-monitor',
    'org.kde.ksysguard': 'activity-monitor',
    'plasma-systemmonitor': 'activity-monitor',
    'org.kde.plasma-systemmonitor': 'activity-monitor',
    'browser': 'safari',
    'safari': 'safari',
    'com.apple.safari': 'safari',
  };

  // Settings retain their native icon: the recoverable image has no genuine
  // System Preferences application or generic preference-pane icon. A sidebar
  // glyph cannot substitute for either application's original resource.
  static const _iconNames = <String, String>{
    'system-file-manager': 'home',
    'utilities-terminal': 'terminal',
    'accessories-text-editor': 'textedit',
    'accessories-calculator': 'calculator',
    'x-office-calendar': 'calendar',
    'accessories-screenshot': 'screenshot',
    'gnome-font-viewer': 'font-book',
    'utilities-system-monitor': 'activity-monitor',
    'web-browser': 'safari',
  };

  // Some third-party launchers use a generic freedesktop icon. Their stable ID
  // still takes priority: Firefox must never silently become Safari.
  static const _preservedBrands = <String>{
    'firefox',
    'firefox-esr',
    'firefox-nightly',
    'org.mozilla.firefox',
    'org.mozilla.firefoxnightly',
    'google-chrome',
    'google-chrome-stable',
    'google-chrome-beta',
    'google-chrome-unstable',
    'com.google.chrome',
    'chromium',
    'chromium-browser',
    'org.chromium.chromium',
    'brave-browser',
    'com.brave.browser',
    'microsoft-edge',
    'com.microsoft.edge',
    'wechat',
    'com.tencent.wechat',
    'com.tencent.wechatclient',
    'qq',
    'com.qq.qq',
    'com.tencent.qq',
    'chatgpt',
    'com.openai.chatgpt',
  };

  static String _desktopId(String value) {
    final id = value.trim().toLowerCase();
    return id.endsWith('.desktop') ? id.substring(0, id.length - 8) : id;
  }

  /// Returns a bundled asset only when the application's identity is known.
  /// Explicit application IDs take precedence over generic icon names.
  static String? assetFor(DesktopApp app) {
    final id = _desktopId(app.id);
    final desktopIcon = _desktopIds[id];
    if (desktopIcon != null) return _assetPath(desktopIcon);
    if (_preservedBrands.contains(id)) return null;
    final iconName = app.iconName?.trim().toLowerCase();
    final icon = _desktopIds[iconName] ?? _iconNames[iconName];
    return icon == null ? null : _assetPath(icon);
  }

  /// Dock ordering uses the same exact identity rules as its presentation.
  static bool isFileManager(DesktopApp app) =>
      assetFor(app) == 'assets/icons/home.svg';
}
