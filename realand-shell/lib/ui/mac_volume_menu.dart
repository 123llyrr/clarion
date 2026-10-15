// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';

import '../state/shell_controller.dart';
import 'controller_selector.dart';

/// Catalina's Volume.menu labels, connected to the shared real audio controls.
class MacVolumeMenu extends StatelessWidget {
  const MacVolumeMenu({super.key, required this.controller});
  final ShellController controller;
  static const width = 260.0;

  @override
  Widget build(BuildContext context) => SingleChildScrollView(
    key: const ValueKey('mac-volume-menu'),
    padding: const EdgeInsets.symmetric(vertical: 6),
    child: DefaultTextStyle.merge(
      style: const TextStyle(fontSize: 13),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          const Padding(
            padding: EdgeInsets.fromLTRB(14, 2, 14, 0),
            child: Text('音量：'),
          ),
          _VolumeControl(controller: controller),
          const Divider(height: 9, thickness: .5),
          Padding(
            padding: const EdgeInsets.fromLTRB(14, 1, 14, 5),
            child: Text(
              '输出设备：',
              style: TextStyle(color: Theme.of(context).disabledColor),
            ),
          ),
          _Outputs(controller: controller),
          const Divider(height: 9, thickness: .5),
          ControllerSelector(
            controller: controller,
            select: () => (
              controller.busy.contains('sound-preferences'),
              controller.error,
            ),
            builder: (context, state) => Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                _VolumeMenuRow(
                  rowKey: const ValueKey('mac-sound-preferences'),
                  label: '声音偏好设置…',
                  onTap: state.$1 || controller.demo
                      ? null
                      : controller.openSoundPreferences,
                ),
                if (state.$2 != null) _ErrorText(state.$2!),
              ],
            ),
          ),
        ],
      ),
    ),
  );
}

class _VolumeControl extends StatelessWidget {
  const _VolumeControl({required this.controller});
  final ShellController controller;
  @override
  Widget build(BuildContext context) => ControllerSelector(
    key: const ValueKey('mac-volume-control-selector'),
    controller: controller,
    select: () => (
      controller.volume,
      controller.muted,
      controller.busy.contains('mute'),
      controller.busy.contains('audio-output'),
    ),
    builder: (context, state) => SizedBox(
      height: 38,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 10),
        child: Row(
          children: [
            SizedBox(
              width: 26,
              child: IconButton(
                key: const ValueKey('mac-volume-mute'),
                tooltip: state.$2 ? '取消静音' : '静音',
                padding: EdgeInsets.zero,
                iconSize: 17,
                onPressed: state.$1 == null || state.$3 || state.$4
                    ? null
                    : controller.toggleMute,
                icon: Icon(
                  state.$2
                      ? Icons.volume_off_rounded
                      : Icons.volume_down_rounded,
                ),
              ),
            ),
            Expanded(
              child: Slider(
                key: const ValueKey('mac-volume-slider'),
                value: state.$1 ?? 0,
                semanticFormatterCallback: (value) =>
                    '${(value * 100).round()}%',
                onChangeStart: state.$1 == null || state.$4
                    ? null
                    : (_) => controller.beginVolumeAdjustment(),
                onChanged: state.$1 == null || state.$4
                    ? null
                    : controller.setVolume,
                onChangeEnd: state.$1 == null || state.$4
                    ? null
                    : controller.endVolumeAdjustment,
              ),
            ),
            const Icon(Icons.volume_up_rounded, size: 17),
          ],
        ),
      ),
    ),
  );
}

class _Outputs extends StatelessWidget {
  const _Outputs({required this.controller});
  final ShellController controller;
  @override
  Widget build(BuildContext context) => ControllerSelector(
    key: const ValueKey('mac-volume-output-selector'),
    controller: controller.audioOutputs,
    select: () => (
      controller.audioOutputs.outputs,
      controller.audioOutputs.selected,
      controller.audioOutputs.loading,
      controller.audioOutputs.busy,
      controller.audioOutputs.error,
    ),
    builder: (context, state) => Column(
      key: const ValueKey('mac-volume-output-rows'),
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (state.$3) const _VolumeMenuRow(label: '正在读取…'),
        if (!state.$3 && state.$1.isEmpty && state.$5 == null)
          const _VolumeMenuRow(label: '没有可用的输出设备'),
        for (final output in state.$1)
          _VolumeMenuRow(
            rowKey: ValueKey('mac-audio-output-${output.name}'),
            label: output.description,
            selected: state.$2 == output.name,
            onTap: state.$4 || !output.available
                ? null
                : () => controller.selectAudioOutput(output.name),
          ),
        if (state.$5 != null) _ErrorText(state.$5!),
      ],
    ),
  );
}

class _ErrorText extends StatelessWidget {
  const _ErrorText(this.message);
  final String message;
  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 6),
    child: Text(
      message,
      style: TextStyle(
        fontSize: 12,
        color: Theme.of(context).colorScheme.error,
      ),
    ),
  );
}

class _VolumeMenuRow extends StatefulWidget {
  const _VolumeMenuRow({
    this.rowKey,
    required this.label,
    this.selected = false,
    this.onTap,
  });
  final Key? rowKey;
  final String label;
  final bool selected;
  final VoidCallback? onTap;
  @override
  State<_VolumeMenuRow> createState() => _VolumeMenuRowState();
}

class _VolumeMenuRowState extends State<_VolumeMenuRow> {
  bool _hovered = false;
  bool _focused = false;
  @override
  Widget build(BuildContext context) {
    final highlighted = widget.onTap != null && (_hovered || _focused);
    final foreground = highlighted
        ? Colors.white
        : widget.onTap == null
        ? Theme.of(context).disabledColor
        : Theme.of(context).colorScheme.onSurface;
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 4),
      child: Semantics(
        selected: widget.selected,
        child: Material(
          color: highlighted ? const Color(0xff007aff) : Colors.transparent,
          borderRadius: BorderRadius.circular(3),
          child: InkWell(
            key: widget.rowKey,
            onTap: widget.onTap,
            onHover: (value) => setState(() => _hovered = value),
            onFocusChange: (value) => setState(() => _focused = value),
            splashFactory: NoSplash.splashFactory,
            overlayColor: const WidgetStatePropertyAll(Colors.transparent),
            child: SizedBox(
              height: 24,
              child: Row(
                children: [
                  SizedBox(
                    width: 22,
                    child: widget.selected
                        ? Icon(Icons.check, size: 12, color: foreground)
                        : null,
                  ),
                  Expanded(
                    child: Text(
                      widget.label,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(color: foreground),
                    ),
                  ),
                  const SizedBox(width: 10),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}
