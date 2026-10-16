// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/mac_app_icons.dart';
import 'package:clarion_shell/models.dart';

DesktopApp _app(String id, {String name = '本地化应用名称', String? iconName}) =>
    DesktopApp(
      id: id,
      name: name,
      desktopFile: '/fixture/$id',
      iconName: iconName,
    );

void main() {
  test('exact desktop identities share original Catalina role icons', () {
    const aliases = <String, List<String>>{
      'finder': [
        'files',
        'org.kde.dolphin',
        'org.gnome.Nautilus',
        'thunar',
        'nemo',
        'pcmanfm-qt',
      ],
      'terminal': [
        'terminal',
        'Alacritty',
        'org.gnome.Terminal',
        'org.kde.konsole',
        'kitty',
        'foot',
        'xterm',
      ],
      'textedit': [
        'org.kde.kate',
        'org.gnome.TextEditor',
        'gedit',
        'mousepad',
        'kwrite',
      ],
      'calculator': ['org.gnome.Calculator', 'org.kde.kcalc'],
      'calendar': ['org.gnome.Calendar', 'com.apple.iCal'],
      'preview': ['org.kde.gwenview', 'org.gnome.Evince', 'org.kde.okular'],
      'screenshot': ['org.kde.spectacle', 'org.flameshot.Flameshot'],
      'font-book': ['org.gnome.font-viewer', 'gfontview', 'kfontview'],
      'activity-monitor': [
        'org.gnome.SystemMonitor',
        'ksysguard',
        'org.kde.plasma-systemmonitor',
      ],
      'safari': ['browser', 'Safari', 'com.apple.Safari'],
    };
    for (final entry in aliases.entries) {
      for (final alias in entry.value) {
        expect(
          MacAppIcons.assetFor(_app('$alias.desktop')),
          'assets/macos/${entry.key}.png',
          reason: alias,
        );
      }
    }
  });

  test('only a desktop suffix is removed and matching ignores letter case', () {
    expect(
      MacAppIcons.assetFor(_app('  ORG.KDE.DOLPHIN.DESKTOP  ')),
      'assets/macos/finder.png',
    );
    for (final id in [
      'org.kde.dolphin.desktop.bak',
      'dolphin.desktop.desktop',
      '/tmp/dolphin.desktop',
      'vendor.dolphin',
      'super-terminal',
      'my-settings',
    ]) {
      expect(MacAppIcons.assetFor(_app(id)), isNull, reason: id);
    }
  });

  test('translated titles and incidental substrings never change identity', () {
    for (final name in [
      'Finder',
      'Safari',
      'Terminal',
      '文本编辑',
      'System Preferences',
    ]) {
      expect(MacAppIcons.assetFor(_app('unknown.desktop', name: name)), isNull);
    }
    expect(
      MacAppIcons.assetFor(_app('com.vendor.TerminalNotebook.desktop')),
      isNull,
    );
  });

  test('freedesktop semantic icon names resolve without an app title', () {
    const names = <String, String>{
      'system-file-manager': 'finder',
      'utilities-terminal': 'terminal',
      'accessories-text-editor': 'textedit',
      'accessories-calculator': 'calculator',
      'x-office-calendar': 'calendar',
      'accessories-screenshot': 'screenshot',
      'utilities-system-monitor': 'activity-monitor',
      'web-browser': 'safari',
      'ORG.KDE.DOLPHIN': 'finder',
    };
    for (final entry in names.entries) {
      expect(
        MacAppIcons.assetFor(
          _app('unregistered-role.desktop', iconName: entry.key),
        ),
        'assets/macos/${entry.value}.png',
      );
    }
    for (final icon in ['my-web-browser', 'terminal-notebook', 'dolphin.svg']) {
      expect(
        MacAppIcons.assetFor(_app('unknown.desktop', iconName: icon)),
        isNull,
      );
    }
  });

  test('a known ID takes precedence over a conflicting generic icon', () {
    expect(
      MacAppIcons.assetFor(
        _app('org.kde.kate.desktop', iconName: 'utilities-terminal'),
      ),
      'assets/macos/textedit.png',
    );
  });

  test(
    'settings retain native icons without attempting an unavailable asset',
    () {
      for (final id in [
        'settings',
        'clarion-settings',
        'systemsettings',
        'org.kde.systemsettings',
        'gnome-control-center',
        'org.gnome.Settings',
        'com.apple.systempreferences',
      ]) {
        expect(
          MacAppIcons.assetFor(
            _app('$id.desktop', iconName: 'preferences-system'),
          ),
          isNull,
          reason: id,
        );
      }
      for (final iconName in [
        'preferences-system',
        'systemsettings',
        'org.kde.systemsettings',
        'org.gnome.settings',
        'com.apple.systempreferences',
      ]) {
        expect(
          MacAppIcons.assetFor(_app('unknown.desktop', iconName: iconName)),
          isNull,
          reason: iconName,
        );
      }
    },
  );

  test('Finder ordering follows the same exact identity as its asset', () {
    expect(MacAppIcons.isFileManager(_app('org.kde.dolphin.desktop')), isTrue);
    expect(
      MacAppIcons.isFileManager(
        _app('unregistered.desktop', iconName: 'system-file-manager'),
      ),
      isTrue,
    );
    expect(
      MacAppIcons.isFileManager(_app('com.vendor.DolphinGame.desktop')),
      isFalse,
    );
    expect(MacAppIcons.isFileManager(_app('org.kde.kate.desktop')), isFalse);
  });

  test('third-party brands remain native even with a generic browser icon', () {
    for (final id in [
      'firefox',
      'firefox-esr',
      'org.mozilla.firefox',
      'google-chrome',
      'com.google.chrome',
      'chromium',
      'org.chromium.Chromium',
      'wechat',
      'com.tencent.WeChat',
      'qq',
      'chatgpt',
    ]) {
      expect(
        MacAppIcons.assetFor(_app('$id.desktop', iconName: 'web-browser')),
        isNull,
        reason: id,
      );
    }
    expect(
      MacAppIcons.assetFor(
        _app('Clash Verge.desktop', iconName: 'clash-verge'),
      ),
      isNull,
    );
  });

  test('the generic fallback identifies a real bundled application asset', () {
    expect(
      MacAppIcons.genericApplication,
      'assets/macos/generic-application.png',
    );
  });
}
