// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import '../theme/font_fallback.dart';

import 'desktop_entry.dart';
import 'desktop_file_details.dart';
import 'desktop_service.dart';

/// Returns an entry only for an explicit Open click. The desktop's caller keeps
/// its existing launcher trust confirmation and native interaction lifecycle.
Future<DesktopEntry?> showDesktopInfoDialog({
  required BuildContext context,
  required DesktopService service,
  required Iterable<DesktopEntry> entries,
  DesktopEntry? current,
  bool useRootNavigator = false,
}) =>
    _showInspector(context, service, entries, current, false, useRootNavigator);

Future<DesktopEntry?> showDesktopQuickLookDialog({
  required BuildContext context,
  required DesktopService service,
  required Iterable<DesktopEntry> entries,
  DesktopEntry? current,
  bool useRootNavigator = false,
}) =>
    _showInspector(context, service, entries, current, true, useRootNavigator);

Future<DesktopEntry?> _showInspector(
  BuildContext context,
  DesktopService service,
  Iterable<DesktopEntry> entries,
  DesktopEntry? current,
  bool quickLook,
  bool useRootNavigator,
) {
  final snapshot = List<DesktopEntry>.unmodifiable(
    {for (final entry in entries) entry.id: entry}.values,
  );
  if (snapshot.isEmpty) return Future<DesktopEntry?>.value();
  final selected = snapshot.indexWhere((entry) => entry.id == current?.id);
  return showDialog<DesktopEntry>(
    context: context,
    useRootNavigator: useRootNavigator,
    builder: (_) => _DesktopInspector(
      service: service,
      entries: snapshot,
      initialIndex: selected < 0 ? 0 : selected,
      quickLook: quickLook,
    ),
  );
}

class _DesktopInspector extends StatefulWidget {
  const _DesktopInspector({
    required this.service,
    required this.entries,
    required this.initialIndex,
    required this.quickLook,
  });
  final DesktopService service;
  final List<DesktopEntry> entries;
  final int initialIndex;
  final bool quickLook;
  @override
  State<_DesktopInspector> createState() => _DesktopInspectorState();
}

class _DesktopInspectorState extends State<_DesktopInspector> {
  late int _index = widget.initialIndex;
  int _generation = 0;
  DesktopInspectionCancellation? _cancellation;
  DesktopFileDetails? _details;
  DesktopFilePreview? _preview;
  ui.Image? _image;
  String? _error;
  bool _loading = true;
  List<String> _textLines = const [];
  bool _longLines = false;
  final _focus = FocusNode(debugLabel: 'desktop-inspector');

  DesktopEntry get _entry => widget.entries[_index];

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  @override
  void dispose() {
    _generation++;
    _cancellation?.cancel();
    _image?.dispose();
    _focus.dispose();
    super.dispose();
  }

  void _navigate(int direction) {
    final next = (_index + direction)
        .clamp(0, widget.entries.length - 1)
        .toInt();
    if (next == _index) return;
    setState(() => _index = next);
    unawaited(_load());
  }

  Future<void> _load() async {
    final generation = ++_generation;
    _cancellation?.cancel();
    final cancellation = _cancellation = DesktopInspectionCancellation();
    final previous = _image;
    setState(() {
      _details = null;
      _preview = null;
      _image = null;
      _error = null;
      _loading = true;
      _textLines = const [];
      _longLines = false;
    });
    if (previous != null) {
      WidgetsBinding.instance.addPostFrameCallback((_) => previous.dispose());
    }
    DesktopFileDetails? details;
    ui.Image? decoded;
    try {
      DesktopFilePreview? preview;
      if (widget.quickLook) {
        preview = await widget.service.previewEntry(
          _entry,
          cancellation: cancellation,
        );
        details = preview.details;
        if (preview.kind == DesktopPreviewKind.image) {
          decoded = await _decodeImage(preview, cancellation);
        }
      } else {
        details = await widget.service.inspectEntry(
          _entry,
          cancellation: cancellation,
        );
      }
      cancellation.check();
      if (!mounted || generation != _generation) {
        decoded?.dispose();
        return;
      }
      setState(() {
        _details = details;
        _preview = preview;
        _image = decoded;
        _textLines = preview?.text?.split('\n') ?? const [];
        _longLines = _textLines.any((line) => line.length > 4096);
        _loading = false;
      });
    } on DesktopInspectionCancelled {
      decoded?.dispose();
    } on Object catch (error) {
      decoded?.dispose();
      if (!mounted || generation != _generation) return;
      setState(() {
        _details = details;
        _error = '$error';
        _loading = false;
      });
    }
  }

  Future<ui.Image> _decodeImage(
    DesktopFilePreview preview,
    DesktopInspectionCancellation cancellation,
  ) async {
    final bytes = preview.imageBytes;
    if (bytes == null || bytes.length > DesktopService.previewImageByteLimit) {
      throw StateError('图片超出预览读取限制');
    }
    final buffer = await ui.ImmutableBuffer.fromUint8List(bytes);
    ui.ImageDescriptor? descriptor;
    ui.Codec? codec;
    try {
      cancellation.check();
      descriptor = await ui.ImageDescriptor.encoded(buffer);
      cancellation.check();
      final width = descriptor.width;
      final height = descriptor.height;
      // Inspect source dimensions before decoding, since some codecs also need
      // intermediate source pixels. Only the first frame reaches a 1600px texture.
      if (width <= 0 ||
          height <= 0 ||
          width > 16384 ||
          height > 16384 ||
          width * height > 16 * 1024 * 1024) {
        throw StateError('图片像素尺寸超出预览限制；可选择打开');
      }
      final scale = math.min(1.0, 1600 / math.max(width, height));
      codec = await descriptor.instantiateCodec(
        targetWidth: math.max(1, (width * scale).round()),
        targetHeight: math.max(1, (height * scale).round()),
      );
      cancellation.check();
      final frame = await codec.getNextFrame();
      if (cancellation.isCancelled) {
        frame.image.dispose();
        throw const DesktopInspectionCancelled();
      }
      return frame.image;
    } finally {
      codec?.dispose();
      descriptor?.dispose();
      buffer.dispose();
    }
  }

  KeyEventResult _key(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent ||
        HardwareKeyboard.instance.isControlPressed ||
        HardwareKeyboard.instance.isMetaPressed ||
        HardwareKeyboard.instance.isAltPressed) {
      return KeyEventResult.ignored;
    }
    if (event.logicalKey == LogicalKeyboardKey.escape ||
        (widget.quickLook && event.logicalKey == LogicalKeyboardKey.space)) {
      Navigator.of(context).pop();
    } else if (event.logicalKey == LogicalKeyboardKey.arrowLeft) {
      _navigate(-1);
    } else if (event.logicalKey == LogicalKeyboardKey.arrowRight) {
      _navigate(1);
    } else {
      return KeyEventResult.ignored;
    }
    return KeyEventResult.handled;
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colors = theme.colorScheme;
    final height = MediaQuery.sizeOf(context).height;
    return Focus(
      autofocus: true,
      focusNode: _focus,
      onKeyEvent: _key,
      child: Dialog(
        key: ValueKey(
          widget.quickLook
              ? 'desktop-quick-look-dialog'
              : 'desktop-info-dialog',
        ),
        insetPadding: const EdgeInsets.all(16),
        backgroundColor: colors.surface,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(12)),
        clipBehavior: Clip.antiAlias,
        child: SizedBox(
          width: widget.quickLook ? 720 : 430,
          child: ConstrainedBox(
            constraints: BoxConstraints(maxHeight: math.max(48.0, height - 32)),
            child: SingleChildScrollView(
              child: Padding(
                padding: const EdgeInsets.all(14),
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    Row(
                      children: [
                        Expanded(
                          child: Text(
                            widget.quickLook
                                ? _entry.displayName
                                : '${_entry.displayName} 简介',
                            style: theme.textTheme.titleMedium,
                            maxLines: 2,
                            overflow: TextOverflow.ellipsis,
                          ),
                        ),
                        IconButton(
                          key: const ValueKey('desktop-inspector-close'),
                          tooltip: '关闭',
                          onPressed: () => Navigator.of(context).pop(),
                          icon: const Icon(Icons.close, size: 18),
                        ),
                      ],
                    ),
                    if (widget.entries.length > 1)
                      Row(
                        children: [
                          IconButton(
                            key: const ValueKey('desktop-inspector-previous'),
                            tooltip: '上一个项目',
                            onPressed: _index == 0 ? null : () => _navigate(-1),
                            icon: const Icon(Icons.chevron_left),
                          ),
                          Expanded(
                            child: Text(
                              '${_index + 1} / ${widget.entries.length}',
                              textAlign: TextAlign.center,
                            ),
                          ),
                          IconButton(
                            key: const ValueKey('desktop-inspector-next'),
                            tooltip: '下一个项目',
                            onPressed: _index + 1 == widget.entries.length
                                ? null
                                : () => _navigate(1),
                            icon: const Icon(Icons.chevron_right),
                          ),
                        ],
                      ),
                    const Divider(height: 16),
                    if (_loading)
                      const Padding(
                        padding: EdgeInsets.all(24),
                        child: Center(
                          child: CircularProgressIndicator(
                            key: ValueKey('desktop-inspector-loading'),
                          ),
                        ),
                      ),
                    if (_error != null)
                      Padding(
                        padding: const EdgeInsets.symmetric(vertical: 12),
                        child: Text(
                          _error!,
                          key: const ValueKey('desktop-inspector-error'),
                          style: TextStyle(color: colors.error),
                        ),
                      ),
                    if (_preview != null)
                      _previewBody(
                        context,
                        math.min(360.0, math.max(120.0, height * .45)),
                      ),
                    if (_details case final details?)
                      _DetailsFields(details: details),
                    const SizedBox(height: 14),
                    Align(
                      alignment: Alignment.centerRight,
                      child: ListenableBuilder(
                        listenable: widget.service,
                        builder: (context, _) => FilledButton(
                          key: const ValueKey('desktop-inspector-open'),
                          onPressed:
                              _loading ||
                                  _details == null ||
                                  widget.service.busy
                              ? null
                              : () => Navigator.of(context).pop(_entry),
                          child: const Text('打开'),
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _previewBody(BuildContext context, double height) {
    final preview = _preview!;
    final theme = Theme.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (preview.kind == DesktopPreviewKind.image && _image != null)
          SizedBox(
            height: height,
            child: RawImage(
              key: const ValueKey('desktop-preview-image'),
              image: _image,
              fit: BoxFit.contain,
            ),
          ),
        if (preview.kind == DesktopPreviewKind.image)
          const Padding(
            padding: EdgeInsets.symmetric(vertical: 6),
            child: Text('图片预览 · 动画仅显示首帧', style: TextStyle(fontSize: 11)),
          ),
        if (preview.kind == DesktopPreviewKind.text)
          Container(
            height: height,
            decoration: BoxDecoration(
              color: theme.colorScheme.surfaceContainerLow,
              borderRadius: BorderRadius.circular(6),
            ),
            child: SelectionArea(
              child: ListView.builder(
                key: const ValueKey('desktop-preview-text'),
                padding: const EdgeInsets.all(10),
                itemCount: _textLines.length,
                itemBuilder: (context, index) {
                  final line = _textLines[index];
                  var end = math.min(line.length, 4096);
                  if (end < line.length &&
                      end > 0 &&
                      line.codeUnitAt(end - 1) >= 0xd800 &&
                      line.codeUnitAt(end - 1) <= 0xdbff) {
                    end--;
                  }
                  return Text(
                    line.isEmpty
                        ? ' '
                        : line.length > 4096
                        ? '${line.substring(0, end)}…'
                        : line,
                    style: const TextStyle(
                      fontFamily: 'monospace',
                      fontFamilyFallback: shellFontFallback,
                      fontSize: 12,
                      height: 1.4,
                    ),
                  );
                },
              ),
            ),
          ),
        if (preview.message != null)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 8),
            child: Text(preview.message!),
          ),
        if (_longLines)
          const Text('较长行仅显示前 4096 个字符。', style: TextStyle(fontSize: 11)),
        const SizedBox(height: 12),
      ],
    );
  }
}

class _DetailsFields extends StatelessWidget {
  const _DetailsFields({required this.details});
  final DesktopFileDetails details;

  @override
  Widget build(BuildContext context) {
    final modified = details.modified.toLocal();
    String two(int value) => '$value'.padLeft(2, '0');
    return SelectionArea(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _row(context, '名称', details.name),
          _row(context, '种类', details.kind.label),
          _row(
            context,
            details.kind == DesktopFileKind.directory ? '目录项大小' : '大小',
            '${details.sizeBytes} 字节',
          ),
          if (details.kind == DesktopFileKind.directory)
            const Text('不包含文件夹内项目的总大小。', style: TextStyle(fontSize: 11)),
          _row(context, '位置', details.path),
          _row(
            context,
            '修改时间',
            '${modified.year}-${two(modified.month)}-${two(modified.day)} ${two(modified.hour)}:${two(modified.minute)}:${two(modified.second)}',
          ),
          _row(context, '权限', details.permissions),
          if (details.linkTarget != null)
            _row(context, '链接目标', details.linkTarget!),
        ],
      ),
    );
  }

  Widget _row(BuildContext context, String title, String value) => Padding(
    padding: const EdgeInsets.symmetric(vertical: 5),
    child: LayoutBuilder(
      builder: (context, constraints) {
        final label = Text(
          title,
          style: TextStyle(
            color: Theme.of(context).colorScheme.onSurfaceVariant,
          ),
        );
        final text = Text(value, softWrap: true);
        return constraints.maxWidth < 300
            ? Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [label, const SizedBox(height: 3), text],
              )
            : Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  SizedBox(width: 78, child: label),
                  Expanded(child: text),
                ],
              );
      },
    ),
  );
}
