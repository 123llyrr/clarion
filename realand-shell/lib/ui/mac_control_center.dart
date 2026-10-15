// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';

import '../models.dart';
import '../services/media_service.dart';
import '../state/shell_controller.dart';
import 'controller_selector.dart';
import 'quick_settings_panel.dart';

/// A compact control center backed by the shell's existing system operations.
/// The dimensions are Realand layout choices, not measurements from Apple UI.
class MacControlCenter extends StatefulWidget {
  const MacControlCenter({
    super.key,
    required this.controller,
    this.onPreferencesChanged,
  });
  final ShellController controller;
  final ValueChanged<bool>? onPreferencesChanged;
  static const width = 320.0;
  static const preferencesWidth = 620.0;

  @override
  State<MacControlCenter> createState() => _MacControlCenterState();
}

enum MacControlCenterSubPage {
  none,
  wifi,
  bluetooth,
  preferences,
}

class _MacControlCenterState extends State<MacControlCenter> {
  MacControlCenterSubPage _subPage = MacControlCenterSubPage.none;

  @override
  void didUpdateWidget(covariant MacControlCenter oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.controller, widget.controller)) {
      _subPage = MacControlCenterSubPage.none;
    }
  }

  void _setSubPage(MacControlCenterSubPage page) {
    setState(() => _subPage = page);
    widget.onPreferencesChanged?.call(page == MacControlCenterSubPage.preferences);
  }

  @override
  Widget build(BuildContext context) {
    final controller = widget.controller;
    return DefaultTextStyle.merge(
      style: const TextStyle(fontSize: 13),
      child: switch (_subPage) {
        MacControlCenterSubPage.preferences => QuickSettingsPanel(
            key: const ValueKey('mac-control-preferences'),
            controller: controller,
            preferencesOnly: true,
            onBack: () => _setSubPage(MacControlCenterSubPage.none),
          ),
        MacControlCenterSubPage.wifi => _WifiDetailPanel(
            controller: controller,
            onBack: () => _setSubPage(MacControlCenterSubPage.none),
          ),
        MacControlCenterSubPage.bluetooth => _BluetoothDetailPanel(
            controller: controller,
            onBack: () => _setSubPage(MacControlCenterSubPage.none),
          ),
        MacControlCenterSubPage.none => SingleChildScrollView(
            key: const ValueKey('mac-control-center'),
            padding: const EdgeInsets.all(12),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                _TopControls(
                  controller: controller,
                  onOpenWifi: () => _setSubPage(MacControlCenterSubPage.wifi),
                  onOpenBluetooth: () =>
                      _setSubPage(MacControlCenterSubPage.bluetooth),
                ),
                const SizedBox(height: 8),
                _BrightnessControl(controller: controller),
                const SizedBox(height: 8),
                _SoundControl(controller: controller),
                _MediaControl(controller: controller),
                const SizedBox(height: 8),
                _Footer(
                  controller: controller,
                  onPreferences: () =>
                      _setSubPage(MacControlCenterSubPage.preferences),
                ),
                _ControlError(controller: controller),
              ],
            ),
          ),
      },
    );
  }
}

class _MediaControl extends StatelessWidget {
  const _MediaControl({required this.controller});
  final ShellController controller;

  @override
  Widget build(BuildContext context) => ControllerSelector(
    controller: controller,
    select: () => (
      controller.media.players,
      controller.media.selected,
      controller.media.busy,
      controller.media.error,
    ),
    builder: (context, state) {
      final player = state.$2;
      if (player == null) return const SizedBox.shrink();
      final media = controller.media;
      final playing = player.status == MediaPlaybackStatus.playing;
      final status = switch (player.status) {
        MediaPlaybackStatus.playing => '正在播放',
        MediaPlaybackStatus.paused => '已暂停',
        MediaPlaybackStatus.stopped => '已停止',
        MediaPlaybackStatus.unknown => '播放状态未知',
      };
      Widget command(MediaCommand command, IconData icon, String tooltip) =>
          IconButton(
            key: ValueKey('mac-media-${command.name}'),
            tooltip: tooltip,
            onPressed: state.$3 || !player.allows(command)
                ? null
                : () => media.invoke(player, command),
            icon: Icon(icon, size: 20),
            padding: const EdgeInsets.all(5),
            constraints: const BoxConstraints.tightFor(width: 34, height: 34),
          );
      return Padding(
        padding: const EdgeInsets.only(top: 8),
        child: _ControlCard(
          key: const ValueKey('mac-control-media'),
          child: Padding(
            padding: const EdgeInsets.all(10),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                PopupMenuButton<MediaPlayer>(
                  key: const ValueKey('mac-media-player-selector'),
                  tooltip: '选择播放器',
                  enabled: state.$1.length > 1,
                  onSelected: media.selectPlayer,
                  itemBuilder: (context) => [
                    for (final candidate in state.$1)
                      PopupMenuItem(
                        value: candidate,
                        child: Text(
                          candidate.identity,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                        ),
                      ),
                  ],
                  child: Padding(
                    padding: const EdgeInsets.only(bottom: 6),
                    child: Row(
                      children: [
                        const Icon(Icons.music_note_rounded, size: 15),
                        const SizedBox(width: 6),
                        Expanded(
                          child: Text(
                            player.identity,
                            key: const ValueKey('mac-media-player-name'),
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: const TextStyle(fontSize: 11),
                          ),
                        ),
                        if (state.$1.length > 1)
                          const Icon(Icons.unfold_more_rounded, size: 14),
                      ],
                    ),
                  ),
                ),
                Text(
                  player.title.isEmpty ? '未提供标题' : player.title,
                  key: const ValueKey('mac-media-title'),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(fontWeight: FontWeight.w600),
                ),
                if (player.artist.isNotEmpty)
                  Text(
                    player.artist,
                    key: const ValueKey('mac-media-artist'),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(fontSize: 11),
                  ),
                Text(
                  status,
                  key: const ValueKey('mac-media-status'),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(fontSize: 10),
                ),
                const SizedBox(height: 2),
                // Keep the theme's accessible hit targets; wrap when a narrow
                // output cannot fit all three controls on the same line.
                Wrap(
                  alignment: WrapAlignment.center,
                  children: [
                    command(
                      MediaCommand.previous,
                      Icons.skip_previous_rounded,
                      '上一首',
                    ),
                    command(
                      MediaCommand.playPause,
                      playing ? Icons.pause_rounded : Icons.play_arrow_rounded,
                      playing ? '暂停' : '播放',
                    ),
                    command(MediaCommand.next, Icons.skip_next_rounded, '下一首'),
                  ],
                ),
                if (state.$4 != null)
                  Text(
                    state.$4!,
                    key: const ValueKey('mac-media-error'),
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      fontSize: 11,
                      color: Theme.of(context).colorScheme.error,
                    ),
                  ),
              ],
            ),
          ),
        ),
      );
    },
  );
}

class _TopControls extends StatelessWidget {
  const _TopControls({
    required this.controller,
    this.onOpenWifi,
    this.onOpenBluetooth,
  });
  final ShellController controller;
  final VoidCallback? onOpenWifi;
  final VoidCallback? onOpenBluetooth;

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) {
      final connectivity = _ConnectivityCard(
        controller: controller,
        onOpenWifi: onOpenWifi,
        onOpenBluetooth: onOpenBluetooth,
      );
      final secondary = Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _DoNotDisturbCard(controller: controller),
          const SizedBox(height: 8),
          _AppearanceCard(controller: controller),
        ],
      );
      // Narrow screens and enlarged text keep every action reachable by
      // scrolling, without squeezing the labels into a fixed-height grid.
      if (constraints.maxWidth < 270 ||
          MediaQuery.textScalerOf(context).scale(13) > 17) {
        return Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [connectivity, const SizedBox(height: 8), secondary],
        );
      }
      return Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Expanded(flex: 3, child: connectivity),
          const SizedBox(width: 8),
          Expanded(flex: 2, child: secondary),
        ],
      );
    },
  );
}

class _ConnectivityCard extends StatelessWidget {
  const _ConnectivityCard({
    required this.controller,
    this.onOpenWifi,
    this.onOpenBluetooth,
  });
  final ShellController controller;
  final VoidCallback? onOpenWifi;
  final VoidCallback? onOpenBluetooth;

  @override
  Widget build(BuildContext context) => ControllerSelector(
    key: const ValueKey('mac-control-connectivity-selector'),
    controller: controller,
    select: () => (
      controller.wifiEnabled,
      controller.bluetoothEnabled,
      controller.busy.contains('wifi'),
      controller.busy.contains('bluetooth'),
      controller.wifiSsid,
    ),
    builder: (context, state) => _ControlCard(
      key: const ValueKey('mac-control-connectivity-card'),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          _ControlAction(
            actionKey: const ValueKey('mac-control-wifi'),
            title: 'Wi-Fi',
            subtitle: _wifiSubtitle(state.$1, state.$5),
            icon: Icons.wifi_rounded,
            active: state.$1 == true,
            busy: state.$3,
            onTap: state.$1 == null ? null : controller.toggleWifi,
            onOpen: onOpenWifi,
            showChevron: true,
          ),
          const Divider(height: 8, thickness: .5, indent: 12, endIndent: 12),
          _ControlAction(
            actionKey: const ValueKey('mac-control-bluetooth'),
            title: '蓝牙',
            subtitle: _availability(state.$2),
            icon: Icons.bluetooth_rounded,
            active: state.$2 == true,
            busy: state.$4,
            onTap: state.$2 == null ? null : controller.toggleBluetooth,
            onOpen: onOpenBluetooth,
            showChevron: true,
          ),
        ],
      ),
    ),
  );

  static String _wifiSubtitle(bool? enabled, String ssid) {
    if (enabled == null) return '不可用';
    if (!enabled) return '已关闭';
    return ssid.isNotEmpty ? ssid : '未连接';
  }

  static String _availability(bool? enabled) => enabled == null
      ? '不可用'
      : enabled
      ? '已开启'
      : '已关闭';
}

class _DoNotDisturbCard extends StatelessWidget {
  const _DoNotDisturbCard({required this.controller});
  final ShellController controller;

  @override
  Widget build(BuildContext context) => ControllerSelector(
    key: const ValueKey('mac-control-dnd-selector'),
    controller: controller,
    select: () => controller.doNotDisturb,
    builder: (context, enabled) => _ControlCard(
      child: Tooltip(
        message: '暂停通知计数提醒，保留通知历史',
        child: _ControlAction(
          actionKey: const ValueKey('mac-control-dnd'),
          title: '勿扰模式',
          subtitle: enabled ? '已开启' : '已关闭',
          icon: Icons.bedtime_rounded,
          active: enabled,
          compact: true,
          onTap: () => controller.setDoNotDisturb(!enabled),
        ),
      ),
    ),
  );
}

class _AppearanceCard extends StatelessWidget {
  const _AppearanceCard({required this.controller});
  final ShellController controller;

  @override
  Widget build(BuildContext context) => ControllerSelector(
    key: const ValueKey('mac-control-appearance-selector'),
    controller: controller,
    select: () => controller.themeMode,
    builder: (context, mode) {
      final dark = Theme.of(context).brightness == Brightness.dark;
      final appearance = dark ? '深色' : '浅色';
      return _ControlCard(
        key: const ValueKey('mac-control-appearance-card'),
        child: Tooltip(
          message: '切换为${dark ? '浅色' : '深色'}桌面外观',
          child: _ControlAction(
            actionKey: const ValueKey('mac-control-appearance'),
            title: '显示外观',
            subtitle: mode == ThemeMode.system
                ? '自动 · $appearance'
                : appearance,
            icon: Icons.contrast_rounded,
            active: dark,
            compact: true,
            onTap: () => controller.setThemeMode(
              dark ? ThemeMode.light : ThemeMode.dark,
            ),
          ),
        ),
      );
    },
  );
}

class _BrightnessControl extends StatelessWidget {
  const _BrightnessControl({required this.controller});
  final ShellController controller;

  @override
  Widget build(BuildContext context) => ControllerSelector(
    key: const ValueKey('mac-control-brightness-selector'),
    controller: controller,
    select: () => controller.brightness,
    builder: (context, value) => RepaintBoundary(
      child: _SliderCard(
        cardKey: const ValueKey('mac-control-brightness-card'),
        title: '显示器',
        status: value == null ? '亮度不可用' : '${(value * 100).round()}%',
        leading: const Icon(Icons.brightness_6_outlined, size: 17),
        slider: Slider(
          key: const ValueKey('brightness-slider'),
          min: .01,
          value: (value ?? .01).clamp(.01, 1).toDouble(),
          semanticFormatterCallback: (value) => '亮度 ${(value * 100).round()}%',
          onChangeStart: value == null
              ? null
              : (_) => controller.beginBrightnessAdjustment(),
          onChanged: value == null ? null : controller.setBrightness,
          onChangeEnd: value == null
              ? null
              : controller.endBrightnessAdjustment,
        ),
      ),
    ),
  );
}

class _SoundControl extends StatelessWidget {
  const _SoundControl({required this.controller});
  final ShellController controller;

  @override
  Widget build(BuildContext context) => ControllerSelector(
    key: const ValueKey('mac-control-volume-selector'),
    controller: controller,
    select: () => (
      controller.volume,
      controller.muted,
      controller.busy.contains('mute'),
      controller.busy.contains('audio-output'),
    ),
    builder: (context, state) {
      final available = state.$1 != null;
      final sliderEnabled = available && !state.$4;
      return RepaintBoundary(
        child: _SliderCard(
          cardKey: const ValueKey('mac-control-volume-card'),
          title: '声音',
          openKey: const ValueKey('mac-control-sound-output'),
          onOpen: () => controller.selectPanel(ShellPanel.volume),
          status: !available
              ? '音量不可用'
              : state.$4
              ? '正在切换输出…'
              : state.$2
              ? '已静音'
              : '${(state.$1! * 100).round()}%',
          leading: SizedBox(
            width: 26,
            height: 26,
            child: IconButton(
              key: const ValueKey('mac-control-mute'),
              tooltip: state.$2 ? '取消静音' : '静音',
              padding: EdgeInsets.zero,
              iconSize: 17,
              onPressed: !available || state.$3 || state.$4
                  ? null
                  : controller.toggleMute,
              icon: Icon(
                state.$2 ? Icons.volume_off_rounded : Icons.volume_up_rounded,
              ),
            ),
          ),
          // Own queued writes must not disable the slider mid-gesture. Only
          // output-device changes suspend interaction with this device's level.
          slider: Slider(
            key: const ValueKey('volume-slider'),
            value: (state.$1 ?? 0).clamp(0, 1).toDouble(),
            semanticFormatterCallback: (value) =>
                '音量 ${(value * 100).round()}%',
            onChangeStart: sliderEnabled
                ? (_) => controller.beginVolumeAdjustment()
                : null,
            onChanged: sliderEnabled ? controller.setVolume : null,
            onChangeEnd: sliderEnabled ? controller.endVolumeAdjustment : null,
          ),
        ),
      );
    },
  );
}

class _SliderCard extends StatelessWidget {
  const _SliderCard({
    required this.cardKey,
    required this.title,
    required this.status,
    required this.leading,
    required this.slider,
    this.openKey,
    this.onOpen,
  });
  final Key cardKey;
  final String title;
  final String status;
  final Widget leading;
  final Widget slider;
  final Key? openKey;
  final VoidCallback? onOpen;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final dark = theme.brightness == Brightness.dark;
    final colors = theme.colorScheme;
    return _ControlCard(
      key: cardKey,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(8, 8, 8, 5),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 4),
              child: Row(
                children: [
                  SizedBox(
                    width: 26,
                    height: 26,
                    child: Center(child: leading),
                  ),
                  const SizedBox(width: 5),
                  Expanded(
                    child: onOpen == null
                        ? Text(
                            title,
                            style: const TextStyle(fontWeight: FontWeight.w600),
                          )
                        : Tooltip(
                            message: '显示声音输出设备',
                            child: InkWell(
                              key: openKey,
                              onTap: onOpen,
                              borderRadius: BorderRadius.circular(4),
                              child: Padding(
                                padding: const EdgeInsets.symmetric(
                                  vertical: 4,
                                ),
                                child: Row(
                                  children: [
                                    Expanded(
                                      child: Text(
                                        title,
                                        style: const TextStyle(
                                          fontWeight: FontWeight.w600,
                                        ),
                                      ),
                                    ),
                                    const Icon(
                                      Icons.chevron_right_rounded,
                                      size: 15,
                                    ),
                                  ],
                                ),
                              ),
                            ),
                          ),
                  ),
                  const SizedBox(width: 8),
                  Flexible(
                    child: Text(
                      status,
                      textAlign: TextAlign.end,
                      style: TextStyle(
                        fontSize: 11,
                        color: colors.onSurfaceVariant,
                      ),
                    ),
                  ),
                ],
              ),
            ),
            SliderTheme(
              data: theme.sliderTheme.copyWith(
                trackHeight: 24,
                trackShape: _CapsuleTrackShape(
                  dark ? const Color(0x28ffffff) : const Color(0x33000000),
                ),
                activeTrackColor: Colors.white,
                inactiveTrackColor: dark
                    ? const Color(0x22ffffff)
                    : const Color(0x18000000),
                disabledActiveTrackColor: colors.surfaceContainerHighest,
                disabledInactiveTrackColor: colors.surfaceContainerHighest,
                thumbShape: const _CapsuleThumbShape(),
                overlayShape: const RoundSliderOverlayShape(overlayRadius: 0),
                overlayColor: Colors.transparent,
                trackGap: 0,
                showValueIndicator: ShowValueIndicator.never,
              ),
              child: SizedBox(height: 34, child: slider),
            ),
          ],
        ),
      ),
    );
  }
}

class _Footer extends StatelessWidget {
  const _Footer({required this.controller, required this.onPreferences});
  final ShellController controller;
  final VoidCallback onPreferences;

  @override
  Widget build(BuildContext context) => ControllerSelector(
    controller: controller,
    select: () => controller.busy.contains('settings'),
    builder: (context, busy) => Column(
      key: const ValueKey('mac-control-footer'),
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _FooterAction(
          actionKey: const ValueKey('mac-control-system-preferences'),
          label: busy ? '正在打开系统偏好设置…' : '系统偏好设置…',
          onTap: busy ? null : controller.openSettings,
        ),
        _FooterAction(
          actionKey: const ValueKey('mac-control-desktop-preferences'),
          label: '桌面偏好设置…',
          onTap: onPreferences,
        ),
      ],
    ),
  );
}

class _FooterAction extends StatelessWidget {
  const _FooterAction({
    required this.actionKey,
    required this.label,
    this.onTap,
  });
  final Key actionKey;
  final String label;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) => Material(
    color: Colors.transparent,
    borderRadius: BorderRadius.circular(6),
    child: InkWell(
      key: actionKey,
      onTap: onTap,
      borderRadius: BorderRadius.circular(6),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 7),
        child: Row(
          children: [
            Expanded(
              child: Text(
                label,
                style: TextStyle(
                  color: onTap == null ? Theme.of(context).disabledColor : null,
                ),
              ),
            ),
            const SizedBox(width: 6),
            const Icon(Icons.chevron_right_rounded, size: 15),
          ],
        ),
      ),
    ),
  );
}

class _ControlError extends StatelessWidget {
  const _ControlError({required this.controller});
  final ShellController controller;

  @override
  Widget build(BuildContext context) => ControllerSelector(
    controller: controller,
    select: () => controller.error,
    builder: (context, message) => message == null
        ? const SizedBox.shrink()
        : Semantics(
            liveRegion: true,
            child: Padding(
              padding: const EdgeInsets.fromLTRB(6, 8, 6, 2),
              child: Text(
                message,
                key: const ValueKey('mac-control-error'),
                style: TextStyle(
                  fontSize: 12,
                  color: Theme.of(context).colorScheme.error,
                ),
              ),
            ),
          ),
  );
}

class _ControlCard extends StatelessWidget {
  const _ControlCard({super.key, required this.child});
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final dark = Theme.of(context).brightness == Brightness.dark;
    return Material(
      color: dark ? const Color(0xf03b3b3f) : const Color(0xf7ffffff),
      textStyle: Theme.of(context).textTheme.bodyMedium?.copyWith(fontSize: 13),
      elevation: 1,
      shadowColor: Colors.black.withValues(alpha: .12),
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(12),
        side: BorderSide(
          color: dark ? const Color(0x25ffffff) : const Color(0x18000000),
          width: .5,
        ),
      ),
      clipBehavior: Clip.antiAlias,
      child: child,
    );
  }
}

class _ControlAction extends StatelessWidget {
  const _ControlAction({
    required this.actionKey,
    required this.title,
    required this.subtitle,
    required this.icon,
    required this.active,
    this.busy = false,
    this.compact = false,
    this.onTap,
    this.onOpen,
    this.showChevron = false,
  });
  final Key actionKey;
  final String title;
  final String subtitle;
  final IconData icon;
  final bool active;
  final bool busy;
  final bool compact;
  final VoidCallback? onTap;
  final VoidCallback? onOpen;
  final bool showChevron;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    final enabled = onTap != null && !busy;
    final iconSize = compact ? 27.0 : 32.0;
    final iconColor = active ? Colors.white : colors.onSurfaceVariant;
    final detailKey = actionKey is ValueKey
        ? ValueKey('${(actionKey as ValueKey).value}-detail')
        : null;
    return Semantics(
      button: true,
      toggled: active,
      enabled: enabled,
      onTap: enabled ? onTap : null,
      label: '$title，${busy ? '正在处理' : subtitle}',
      child: ExcludeSemantics(
        child: InkWell(
          key: actionKey,
          onTap: enabled ? onTap : null,
          child: Padding(
            padding: EdgeInsets.symmetric(
              horizontal: compact ? 8 : 10,
              vertical: 10,
            ),
            child: Row(
              children: [
                Container(
                  width: iconSize,
                  height: iconSize,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: active
                        ? const Color(0xff007aff)
                        : colors.surfaceContainerHighest,
                  ),
                  child: busy
                      ? Padding(
                          padding: const EdgeInsets.all(7),
                          child: CircularProgressIndicator(
                            strokeWidth: 1.5,
                            color: iconColor,
                          ),
                        )
                      : Icon(icon, size: compact ? 16 : 18, color: iconColor),
                ),
                SizedBox(width: compact ? 6 : 9),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Text(
                        title,
                        style: const TextStyle(fontWeight: FontWeight.w600),
                      ),
                      const SizedBox(height: 2),
                      Text(
                        busy ? '正在处理…' : subtitle,
                        style: TextStyle(
                          fontSize: 11,
                          color: colors.onSurfaceVariant,
                        ),
                      ),
                    ],
                  ),
                ),
                if (showChevron && onOpen != null) ...[
                  const SizedBox(width: 4),
                  InkResponse(
                    key: detailKey,
                    radius: 14,
                    onTap: busy ? null : onOpen,
                    child: Padding(
                      padding: const EdgeInsets.all(4),
                      child: Icon(
                        Icons.chevron_right_rounded,
                        size: 16,
                        color: colors.onSurfaceVariant,
                      ),
                    ),
                  ),
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// Knobless capsule thumb shape matching macOS Big Sur / Monterey.
class _CapsuleThumbShape extends SliderComponentShape {
  const _CapsuleThumbShape();

  @override
  Size getPreferredSize(bool isEnabled, bool isDiscrete) => Size.zero;

  @override
  void paint(
    PaintingContext context,
    Offset center, {
    required Animation<double> activationAnimation,
    required Animation<double> enableAnimation,
    required bool isDiscrete,
    required TextPainter labelPainter,
    required RenderBox parentBox,
    required SliderThemeData sliderTheme,
    required TextDirection textDirection,
    required double value,
    required double textScaleFactor,
    required Size sizeWithOverflow,
  }) {
    // Knobless capsule: the progress fill is rendered cleanly by _CapsuleTrackShape.
  }
}

/// Keep the whole capsule visible against a light module, including its fill.
class _CapsuleTrackShape extends SliderTrackShape {
  const _CapsuleTrackShape(this.outline);
  final Color outline;

  @override
  Rect getPreferredRect({
    required RenderBox parentBox,
    Offset offset = Offset.zero,
    required SliderThemeData sliderTheme,
    bool isEnabled = false,
    bool isDiscrete = false,
  }) {
    final double trackHeight = sliderTheme.trackHeight ?? 24.0;
    final double trackLeft = offset.dx;
    final double trackTop =
        offset.dy + (parentBox.size.height - trackHeight) / 2;
    final double trackWidth = parentBox.size.width;
    return Rect.fromLTWH(trackLeft, trackTop, trackWidth, trackHeight);
  }

  @override
  void paint(
    PaintingContext context,
    Offset offset, {
    required RenderBox parentBox,
    required SliderThemeData sliderTheme,
    required Animation<double> enableAnimation,
    required TextDirection textDirection,
    required Offset thumbCenter,
    Offset? secondaryOffset,
    bool isDiscrete = false,
    bool isEnabled = false,
    double additionalActiveTrackHeight = 0,
  }) {
    final rect = getPreferredRect(
      parentBox: parentBox,
      offset: offset,
      sliderTheme: sliderTheme,
      isEnabled: isEnabled,
      isDiscrete: isDiscrete,
    );
    final canvas = context.canvas;
    final rrect = RRect.fromRectAndRadius(
      rect,
      Radius.circular(rect.height / 2),
    );

    canvas.save();
    canvas.clipRRect(rrect);

    // Inactive background track
    final inactivePaint = Paint()
      ..color = (isEnabled
              ? sliderTheme.inactiveTrackColor
              : sliderTheme.disabledInactiveTrackColor) ??
          const Color(0x33ffffff);
    canvas.drawRect(rect, inactivePaint);

    // Active track fill up to thumbCenter.dx
    final activeRight = thumbCenter.dx.clamp(rect.left, rect.right);
    if (activeRight > rect.left) {
      final activePaint = Paint()
        ..color = (isEnabled
                ? sliderTheme.activeTrackColor
                : sliderTheme.disabledActiveTrackColor) ??
            Colors.white;
      canvas.drawRect(
        Rect.fromLTRB(rect.left, rect.top, activeRight, rect.bottom),
        activePaint,
      );
    }
    canvas.restore();

    // Subtle hairline outline border
    canvas.drawRRect(
      rrect,
      Paint()
        ..color = outline
        ..style = PaintingStyle.stroke
        ..strokeWidth = .5,
    );
  }
}

class _WifiDetailPanel extends StatefulWidget {
  const _WifiDetailPanel({
    required this.controller,
    required this.onBack,
  });
  final ShellController controller;
  final VoidCallback onBack;

  @override
  State<_WifiDetailPanel> createState() => _WifiDetailPanelState();
}

class _WifiDetailPanelState extends State<_WifiDetailPanel> {
  @override
  void initState() {
    super.initState();
    widget.controller.scanWifiNetworks();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colors = theme.colorScheme;
    final controller = widget.controller;

    return ControllerSelector(
      controller: controller,
      select: () => (
        controller.wifiEnabled,
        controller.wifiSsid,
        controller.wifiNetworks,
        controller.wifiScanning,
        controller.busy.contains('wifi'),
        controller.busy.contains('wifi-connect'),
      ),
      builder: (context, state) {
        final enabled = state.$1 ?? false;
        final currentSsid = state.$2;
        final networks = state.$3;
        final scanning = state.$4;
        final busyWifi = state.$5;
        final busyConnect = state.$6;

        return SingleChildScrollView(
          padding: const EdgeInsets.all(12),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Row(
                children: [
                  IconButton(
                    key: const ValueKey('mac-wifi-back'),
                    icon: const Icon(Icons.chevron_left_rounded, size: 20),
                    tooltip: '返回',
                    padding: EdgeInsets.zero,
                    constraints: const BoxConstraints.tightFor(
                      width: 28,
                      height: 28,
                    ),
                    onPressed: widget.onBack,
                  ),
                  const SizedBox(width: 4),
                  const Text(
                    'Wi-Fi',
                    style: TextStyle(fontSize: 14, fontWeight: FontWeight.w600),
                  ),
                  const Spacer(),
                  Transform.scale(
                    scale: 0.8,
                    child: Switch.adaptive(
                      key: const ValueKey('mac-wifi-switch'),
                      value: enabled,
                      onChanged: busyWifi
                          ? null
                          : (_) => controller.toggleWifi(),
                    ),
                  ),
                ],
              ),
              const SizedBox(height: 8),
              if (!enabled)
                _ControlCard(
                  child: Padding(
                    padding: const EdgeInsets.symmetric(
                      horizontal: 14,
                      vertical: 12,
                    ),
                    child: Text(
                      'Wi-Fi：关闭',
                      style: TextStyle(color: colors.onSurfaceVariant),
                    ),
                  ),
                )
              else ...[
                if (currentSsid.isNotEmpty) ...[
                  Padding(
                    padding: const EdgeInsets.fromLTRB(6, 6, 6, 4),
                    child: Text(
                      '已知网络',
                      style: TextStyle(
                        fontSize: 11,
                        fontWeight: FontWeight.w600,
                        color: colors.onSurfaceVariant,
                      ),
                    ),
                  ),
                  _ControlCard(
                    child: _NetworkRow(
                      ssid: currentSsid,
                      signalBars: 3,
                      isSecured: true,
                      isConnected: true,
                      onTap: null,
                    ),
                  ),
                  const SizedBox(height: 8),
                ],
                Padding(
                  padding: const EdgeInsets.fromLTRB(6, 6, 6, 4),
                  child: Row(
                    children: [
                      Text(
                        '其他网络',
                        style: TextStyle(
                          fontSize: 11,
                          fontWeight: FontWeight.w600,
                          color: colors.onSurfaceVariant,
                        ),
                      ),
                      const Spacer(),
                      if (scanning)
                        const SizedBox.square(
                          dimension: 12,
                          child: CircularProgressIndicator(strokeWidth: 1.5),
                        ),
                    ],
                  ),
                ),
                _ControlCard(
                  child: networks.where((n) => n.ssid != currentSsid).isEmpty
                      ? Padding(
                          padding: const EdgeInsets.symmetric(
                            horizontal: 14,
                            vertical: 12,
                          ),
                          child: Text(
                            scanning ? '正在查找网络…' : '未发现其他网络',
                            style: TextStyle(
                              fontSize: 12,
                              color: colors.onSurfaceVariant,
                            ),
                          ),
                        )
                      : Column(
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            for (final (index, net) in networks
                                .where((n) => n.ssid != currentSsid)
                                .indexed) ...[
                              if (index > 0)
                                const Divider(
                                  height: 1,
                                  thickness: 0.5,
                                  indent: 12,
                                  endIndent: 12,
                                ),
                              _NetworkRow(
                                ssid: net.ssid,
                                signalBars: net.signalBars,
                                isSecured: net.isSecured,
                                isConnected: false,
                                busy: busyConnect,
                                onTap: () => controller.connectWifi(net.ssid),
                              ),
                            ],
                          ],
                        ),
                ),
              ],
              const SizedBox(height: 8),
              const Divider(height: 1, thickness: 0.5),
              const SizedBox(height: 4),
              _FooterAction(
                actionKey: const ValueKey('mac-wifi-preferences'),
                label: '打开“网络”偏好设置…',
                onTap: controller.openNetworkPreferences,
              ),
            ],
          ),
        );
      },
    );
  }
}

class _NetworkRow extends StatelessWidget {
  const _NetworkRow({
    required this.ssid,
    required this.signalBars,
    required this.isSecured,
    required this.isConnected,
    this.busy = false,
    this.onTap,
  });
  final String ssid;
  final int signalBars;
  final bool isSecured;
  final bool isConnected;
  final bool busy;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    final icon = switch (signalBars) {
      3 => Icons.wifi_rounded,
      2 => Icons.wifi_2_bar_rounded,
      _ => Icons.wifi_1_bar_rounded,
    };
    return InkWell(
      onTap: busy ? null : onTap,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
        child: Row(
          children: [
            if (isConnected)
              const Icon(Icons.check, size: 14, color: Color(0xff007aff))
            else
              const SizedBox(width: 14),
            const SizedBox(width: 8),
            Expanded(
              child: Text(
                ssid,
                style: TextStyle(
                  fontWeight: isConnected ? FontWeight.w600 : FontWeight.normal,
                ),
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
            ),
            if (isSecured) ...[
              const SizedBox(width: 6),
              Icon(
                Icons.lock_outline_rounded,
                size: 13,
                color: colors.onSurfaceVariant,
              ),
            ],
            const SizedBox(width: 6),
            Icon(
              icon,
              size: 16,
              color: isConnected
                  ? const Color(0xff007aff)
                  : colors.onSurfaceVariant,
            ),
          ],
        ),
      ),
    );
  }
}

class _BluetoothDetailPanel extends StatefulWidget {
  const _BluetoothDetailPanel({
    required this.controller,
    required this.onBack,
  });
  final ShellController controller;
  final VoidCallback onBack;

  @override
  State<_BluetoothDetailPanel> createState() => _BluetoothDetailPanelState();
}

class _BluetoothDetailPanelState extends State<_BluetoothDetailPanel> {
  @override
  void initState() {
    super.initState();
    widget.controller.refreshBluetoothDevices();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colors = theme.colorScheme;
    final controller = widget.controller;

    return ControllerSelector(
      controller: controller,
      select: () => (
        controller.bluetoothEnabled,
        controller.bluetoothDevices,
        controller.bluetoothScanning,
        controller.busy.contains('bluetooth'),
        controller.busy.contains('bluetooth-device'),
      ),
      builder: (context, state) {
        final enabled = state.$1 ?? false;
        final devices = state.$2;
        final scanning = state.$3;
        final busyBt = state.$4;
        final busyDevice = state.$5;

        return SingleChildScrollView(
          padding: const EdgeInsets.all(12),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Row(
                children: [
                  IconButton(
                    key: const ValueKey('mac-bluetooth-back'),
                    icon: const Icon(Icons.chevron_left_rounded, size: 20),
                    tooltip: '返回',
                    padding: EdgeInsets.zero,
                    constraints: const BoxConstraints.tightFor(
                      width: 28,
                      height: 28,
                    ),
                    onPressed: widget.onBack,
                  ),
                  const SizedBox(width: 4),
                  const Text(
                    '蓝牙',
                    style: TextStyle(fontSize: 14, fontWeight: FontWeight.w600),
                  ),
                  const Spacer(),
                  Transform.scale(
                    scale: 0.8,
                    child: Switch.adaptive(
                      key: const ValueKey('mac-bluetooth-switch'),
                      value: enabled,
                      onChanged: busyBt
                          ? null
                          : (_) => controller.toggleBluetooth(),
                    ),
                  ),
                ],
              ),
              const SizedBox(height: 8),
              if (!enabled)
                _ControlCard(
                  child: Padding(
                    padding: const EdgeInsets.symmetric(
                      horizontal: 14,
                      vertical: 12,
                    ),
                    child: Text(
                      '蓝牙：关闭',
                      style: TextStyle(color: colors.onSurfaceVariant),
                    ),
                  ),
                )
              else ...[
                Padding(
                  padding: const EdgeInsets.fromLTRB(6, 6, 6, 4),
                  child: Row(
                    children: [
                      Text(
                        '设备',
                        style: TextStyle(
                          fontSize: 11,
                          fontWeight: FontWeight.w600,
                          color: colors.onSurfaceVariant,
                        ),
                      ),
                      const Spacer(),
                      if (scanning)
                        const SizedBox.square(
                          dimension: 12,
                          child: CircularProgressIndicator(strokeWidth: 1.5),
                        ),
                    ],
                  ),
                ),
                _ControlCard(
                  child: devices.isEmpty
                      ? Padding(
                          padding: const EdgeInsets.symmetric(
                            horizontal: 14,
                            vertical: 12,
                          ),
                          child: Text(
                            scanning ? '正在查找设备…' : '未找到蓝牙设备',
                            style: TextStyle(
                              fontSize: 12,
                              color: colors.onSurfaceVariant,
                            ),
                          ),
                        )
                      : Column(
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            for (final (index, dev) in devices.indexed) ...[
                              if (index > 0)
                                const Divider(
                                  height: 1,
                                  thickness: 0.5,
                                  indent: 12,
                                  endIndent: 12,
                                ),
                              _BluetoothDeviceRow(
                                device: dev,
                                busy: busyDevice,
                                onTap: () =>
                                    controller.toggleBluetoothDevice(dev),
                              ),
                            ],
                          ],
                        ),
                ),
              ],
              const SizedBox(height: 8),
              const Divider(height: 1, thickness: 0.5),
              const SizedBox(height: 4),
              _FooterAction(
                actionKey: const ValueKey('mac-bluetooth-preferences'),
                label: '打开“蓝牙”偏好设置…',
                onTap: controller.openBluetoothPreferences,
              ),
            ],
          ),
        );
      },
    );
  }
}

class _BluetoothDeviceRow extends StatelessWidget {
  const _BluetoothDeviceRow({
    required this.device,
    this.busy = false,
    this.onTap,
  });
  final BluetoothDevice device;
  final bool busy;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    final icon = switch (device.icon) {
      'audio-headset' || 'audio-card' => Icons.headphones_rounded,
      'input-keyboard' => Icons.keyboard_rounded,
      'input-mouse' => Icons.mouse_rounded,
      'phone' => Icons.smartphone_rounded,
      _ => Icons.bluetooth_rounded,
    };
    return InkWell(
      onTap: busy ? null : onTap,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
        child: Row(
          children: [
            Icon(
              icon,
              size: 18,
              color: device.connected
                  ? const Color(0xff007aff)
                  : colors.onSurfaceVariant,
            ),
            const SizedBox(width: 8),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text(
                    device.name,
                    style: TextStyle(
                      fontWeight: device.connected
                          ? FontWeight.w600
                          : FontWeight.normal,
                    ),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                  const SizedBox(height: 1),
                  Text(
                    device.connected ? '已连接' : '未连接',
                    style: TextStyle(
                      fontSize: 10,
                      color: colors.onSurfaceVariant,
                    ),
                  ),
                ],
              ),
            ),
            if (device.connected)
              const Icon(Icons.check, size: 14, color: Color(0xff007aff)),
          ],
        ),
      ),
    );
  }
}

