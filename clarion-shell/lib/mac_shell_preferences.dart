// SPDX-License-Identifier: GPL-3.0-or-later

enum MacDockPosition { bottom, left, right }

enum MacMinimizeEffect { genie, scale }

enum MacDownloadsView { automatic, fan, grid, list }

enum MacDownloadsDisplay { folder, stack }

enum MacDownloadsSort { name, dateAdded, dateModified, dateCreated, kind }

/// Independent macOS appearance choices. Material keeps its own bar settings.
/// Explicit constructor values are bounded; untrusted JSON and [copyWith]
/// normalize numbers and retain usable defaults for malformed values.
class MacShellPreferences {
  const MacShellPreferences({
    this.dockPosition = MacDockPosition.bottom,
    this.minimizeEffect = MacMinimizeEffect.genie,
    this.dockSize = 48,
    this.magnification = true,
    this.magnificationScale = 1.45,
    this.dockAutoHide = false,
    this.showRunningIndicators = true,
    this.animateOpeningApplications = true,
    this.showLaunchpad = true,
    this.showMissionControl = true,
    this.showDownloads = true,
    this.showRecentApplications = true,
    this.downloadsView = MacDownloadsView.automatic,
    this.downloadsDisplay = MacDownloadsDisplay.stack,
    this.downloadsSort = MacDownloadsSort.dateModified,
    this.menuBarAutoHide = false,
    this.menuBarHeight = 22,
    this.menuBarWidthPercent = 100,
    this.showBatteryPercentage = false,
    this.clockShowDate = true,
    this.clockShowSeconds = false,
    this.useOriginalIcons = true,
    this.minimizeIntoApp = true,
  }) : assert(dockSize >= minDockSize && dockSize <= maxDockSize),
       assert(
         magnificationScale >= minMagnificationScale &&
             magnificationScale <= maxMagnificationScale,
       ),
       assert(
         menuBarHeight >= minMenuBarHeight &&
             menuBarHeight <= maxMenuBarHeight &&
             menuBarHeight % 1 == 0,
       ),
       assert(
         menuBarWidthPercent >= minMenuBarWidthPercent &&
             menuBarWidthPercent <= maxMenuBarWidthPercent,
       );

  static const minDockSize = 32.0;
  static const maxDockSize = 64.0;
  static const minMagnificationScale = 1.0;
  static const maxMagnificationScale = 2.0;
  static const minMenuBarHeight = 22.0;
  static const maxMenuBarHeight = 48.0;
  static const minMenuBarWidthPercent = 50.0;
  static const maxMenuBarWidthPercent = 100.0;

  final MacDockPosition dockPosition;
  final MacMinimizeEffect minimizeEffect;
  final double dockSize;
  final bool magnification;
  final double magnificationScale;
  final bool dockAutoHide;
  final bool showRunningIndicators;
  final bool animateOpeningApplications;
  final bool showLaunchpad;
  final bool showMissionControl;
  final bool showDownloads;
  final bool showRecentApplications;
  final MacDownloadsView downloadsView;
  final MacDownloadsDisplay downloadsDisplay;
  final MacDownloadsSort downloadsSort;
  final bool menuBarAutoHide;
  final double menuBarHeight;
  // Retained for existing configuration files; the settings UI edits height.
  final double menuBarWidthPercent;
  final bool showBatteryPercentage;
  final bool clockShowDate;
  final bool clockShowSeconds;
  final bool useOriginalIcons;
  final bool minimizeIntoApp;

  factory MacShellPreferences.fromJson(Map<String, dynamic> json) =>
      MacShellPreferences(
        dockPosition: switch (json['dock_position']) {
          'left' => MacDockPosition.left,
          'right' => MacDockPosition.right,
          _ => MacDockPosition.bottom,
        },
        minimizeEffect: json['minimize_effect'] == 'scale'
            ? MacMinimizeEffect.scale
            : MacMinimizeEffect.genie,
        dockSize: _number(json['dock_size'], 48, minDockSize, maxDockSize),
        magnification: _boolean(json['magnification'], true),
        magnificationScale: _number(
          json['magnification_scale'],
          1.45,
          minMagnificationScale,
          maxMagnificationScale,
        ),
        dockAutoHide: _boolean(json['dock_auto_hide'], false),
        showRunningIndicators: _boolean(json['show_running_indicators'], true),
        animateOpeningApplications: _boolean(
          json['animate_opening_applications'],
          true,
        ),
        showLaunchpad: _boolean(json['show_launchpad'], true),
        showMissionControl: _boolean(json['show_mission_control'], true),
        showDownloads: _boolean(json['show_downloads'], true),
        showRecentApplications: _boolean(
          json['show_recent_applications'],
          true,
        ),
        downloadsView: _enumValue(
          json['downloads_view'],
          MacDownloadsView.values,
          MacDownloadsView.automatic,
        ),
        downloadsDisplay: _enumValue(
          json['downloads_display'],
          MacDownloadsDisplay.values,
          MacDownloadsDisplay.stack,
        ),
        downloadsSort: _enumValue(
          json['downloads_sort'],
          MacDownloadsSort.values,
          MacDownloadsSort.dateModified,
        ),
        menuBarAutoHide: _boolean(json['menu_bar_auto_hide'], false),
        menuBarHeight: _menuBarHeight(json['menu_bar_height']),
        menuBarWidthPercent: _number(
          json['menu_bar_width_percent'],
          100,
          minMenuBarWidthPercent,
          maxMenuBarWidthPercent,
        ),
        showBatteryPercentage: _boolean(json['show_battery_percentage'], false),
        clockShowDate: _boolean(json['clock_show_date'], true),
        clockShowSeconds: _boolean(json['clock_show_seconds'], false),
        useOriginalIcons: _boolean(json['use_original_icons'], true),
        minimizeIntoApp: _boolean(json['minimize_into_app'], true),
      );

  MacShellPreferences copyWith({
    MacDockPosition? dockPosition,
    MacMinimizeEffect? minimizeEffect,
    double? dockSize,
    bool? magnification,
    double? magnificationScale,
    bool? dockAutoHide,
    bool? showRunningIndicators,
    bool? animateOpeningApplications,
    bool? showLaunchpad,
    bool? showMissionControl,
    bool? showDownloads,
    bool? showRecentApplications,
    MacDownloadsView? downloadsView,
    MacDownloadsDisplay? downloadsDisplay,
    MacDownloadsSort? downloadsSort,
    bool? menuBarAutoHide,
    double? menuBarHeight,
    double? menuBarWidthPercent,
    bool? showBatteryPercentage,
    bool? clockShowDate,
    bool? clockShowSeconds,
    bool? useOriginalIcons,
    bool? minimizeIntoApp,
  }) => MacShellPreferences(
    dockPosition: dockPosition ?? this.dockPosition,
    minimizeEffect: minimizeEffect ?? this.minimizeEffect,
    dockSize: _number(dockSize ?? this.dockSize, 48, minDockSize, maxDockSize),
    magnification: magnification ?? this.magnification,
    magnificationScale: _number(
      magnificationScale ?? this.magnificationScale,
      1.45,
      minMagnificationScale,
      maxMagnificationScale,
    ),
    dockAutoHide: dockAutoHide ?? this.dockAutoHide,
    showRunningIndicators: showRunningIndicators ?? this.showRunningIndicators,
    animateOpeningApplications:
        animateOpeningApplications ?? this.animateOpeningApplications,
    showLaunchpad: showLaunchpad ?? this.showLaunchpad,
    showMissionControl: showMissionControl ?? this.showMissionControl,
    showDownloads: showDownloads ?? this.showDownloads,
    showRecentApplications:
        showRecentApplications ?? this.showRecentApplications,
    downloadsView: downloadsView ?? this.downloadsView,
    downloadsDisplay: downloadsDisplay ?? this.downloadsDisplay,
    downloadsSort: downloadsSort ?? this.downloadsSort,
    menuBarAutoHide: menuBarAutoHide ?? this.menuBarAutoHide,
    menuBarHeight: _menuBarHeight(menuBarHeight ?? this.menuBarHeight),
    menuBarWidthPercent: _number(
      menuBarWidthPercent ?? this.menuBarWidthPercent,
      100,
      minMenuBarWidthPercent,
      maxMenuBarWidthPercent,
    ),
    showBatteryPercentage: showBatteryPercentage ?? this.showBatteryPercentage,
    clockShowDate: clockShowDate ?? this.clockShowDate,
    clockShowSeconds: clockShowSeconds ?? this.clockShowSeconds,
    useOriginalIcons: useOriginalIcons ?? this.useOriginalIcons,
    minimizeIntoApp: minimizeIntoApp ?? this.minimizeIntoApp,
  );

  Map<String, dynamic> toJson() => {
    'dock_position': dockPosition.name,
    'minimize_effect': minimizeEffect.name,
    'dock_size': dockSize,
    'magnification': magnification,
    'magnification_scale': magnificationScale,
    'dock_auto_hide': dockAutoHide,
    'show_running_indicators': showRunningIndicators,
    'animate_opening_applications': animateOpeningApplications,
    'show_launchpad': showLaunchpad,
    'show_mission_control': showMissionControl,
    'show_downloads': showDownloads,
    'show_recent_applications': showRecentApplications,
    'downloads_view': downloadsView.name,
    'downloads_display': downloadsDisplay.name,
    'downloads_sort': downloadsSort.name,
    'menu_bar_auto_hide': menuBarAutoHide,
    'menu_bar_height': menuBarHeight,
    'menu_bar_width_percent': menuBarWidthPercent,
    'show_battery_percentage': showBatteryPercentage,
    'clock_show_date': clockShowDate,
    'clock_show_seconds': clockShowSeconds,
    'use_original_icons': useOriginalIcons,
    'minimize_into_app': minimizeIntoApp,
  };

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is MacShellPreferences &&
          dockPosition == other.dockPosition &&
          minimizeEffect == other.minimizeEffect &&
          dockSize == other.dockSize &&
          magnification == other.magnification &&
          magnificationScale == other.magnificationScale &&
          dockAutoHide == other.dockAutoHide &&
          showRunningIndicators == other.showRunningIndicators &&
          animateOpeningApplications == other.animateOpeningApplications &&
          showLaunchpad == other.showLaunchpad &&
          showMissionControl == other.showMissionControl &&
          showDownloads == other.showDownloads &&
          showRecentApplications == other.showRecentApplications &&
          downloadsView == other.downloadsView &&
          downloadsDisplay == other.downloadsDisplay &&
          downloadsSort == other.downloadsSort &&
          menuBarAutoHide == other.menuBarAutoHide &&
          menuBarHeight == other.menuBarHeight &&
          menuBarWidthPercent == other.menuBarWidthPercent &&
          showBatteryPercentage == other.showBatteryPercentage &&
          clockShowDate == other.clockShowDate &&
          clockShowSeconds == other.clockShowSeconds &&
          useOriginalIcons == other.useOriginalIcons &&
          minimizeIntoApp == other.minimizeIntoApp;

  @override
  int get hashCode => Object.hashAll([
    dockPosition,
    minimizeEffect,
    dockSize,
    magnification,
    magnificationScale,
    dockAutoHide,
    showRunningIndicators,
    animateOpeningApplications,
    showLaunchpad,
    showMissionControl,
    showDownloads,
    showRecentApplications,
    downloadsView,
    downloadsDisplay,
    downloadsSort,
    menuBarAutoHide,
    menuBarHeight,
    menuBarWidthPercent,
    showBatteryPercentage,
    clockShowDate,
    clockShowSeconds,
    useOriginalIcons,
    minimizeIntoApp,
  ]);
}

T _enumValue<T extends Enum>(Object? value, List<T> values, T fallback) {
  for (final candidate in values) {
    if (candidate.name == value) return candidate;
  }
  return fallback;
}

bool _boolean(Object? value, bool fallback) => value is bool ? value : fallback;

double _menuBarHeight(Object? value) => _number(
  value,
  22,
  MacShellPreferences.minMenuBarHeight,
  MacShellPreferences.maxMenuBarHeight,
).roundToDouble();

double _number(Object? value, double fallback, double minimum, double maximum) {
  if (value is! num) return fallback;
  final number = value.toDouble();
  return number.isFinite ? number.clamp(minimum, maximum).toDouble() : fallback;
}
