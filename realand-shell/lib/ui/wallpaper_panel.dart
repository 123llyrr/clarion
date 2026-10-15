// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart' show ScrollCacheExtent;

import '../models.dart';
import '../state/shell_controller.dart';
import '../services/wallpaper_thumbnail_service.dart';
import 'controller_selector.dart';
import 'panel_components.dart';

class WallpaperPanel extends StatefulWidget {
  const WallpaperPanel({super.key, required this.controller, this.thumbnails});
  final ShellController controller;
  final WallpaperThumbnailService? thumbnails;

  @override
  State<WallpaperPanel> createState() => _WallpaperPanelState();
}

class _WallpaperPanelState extends State<WallpaperPanel> {
  final _query = TextEditingController();
  final _searchFocus = FocusNode();
  final _scroll = ScrollController();
  late WallpaperThumbnailService _thumbnails;

  @override
  void initState() {
    super.initState();
    _thumbnails = widget.thumbnails ?? WallpaperThumbnailService();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _searchFocus.requestFocus();
    });
  }

  @override
  void dispose() {
    if (widget.thumbnails == null) _thumbnails.dispose();
    _query.dispose();
    _searchFocus.dispose();
    _scroll.dispose();
    super.dispose();
  }

  @override
  void didUpdateWidget(covariant WallpaperPanel oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.thumbnails, widget.thumbnails)) {
      if (oldWidget.thumbnails == null) _thumbnails.dispose();
      _thumbnails = widget.thumbnails ?? WallpaperThumbnailService();
    }
  }

  void _changed(String _) {
    setState(() {});
    if (_scroll.hasClients) _scroll.jumpTo(0);
  }

  @override
  Widget build(BuildContext context) => ControllerSelector(
    controller: widget.controller,
    select: () => (
      widget.controller.wallpapers,
      widget.controller.wallpapersLoading,
      widget.controller.busy.contains('wallpaper'),
      widget.controller.selectedWallpaperPath,
      widget.controller.wallpaperError,
      widget.controller.wallpaperAvailable,
    ),
    builder: (context, _) => _content(context),
  );

  Widget _content(BuildContext context) {
    final controller = widget.controller;
    final query = _query.text.trim().toLowerCase();
    final wallpapers = controller.wallpapers
        .where((wallpaper) => wallpaper.name.toLowerCase().contains(query))
        .toList();
    final applying = controller.busy.contains('wallpaper');
    final loading = controller.wallpapersLoading;
    final available = controller.wallpaperAvailable;
    final colors = Theme.of(context).colorScheme;
    return LayoutBuilder(
      builder: (context, constraints) {
        final columns = math.max(
          1,
          ((constraints.maxWidth - 40) / 180).floor(),
        );
        final tileWidth =
            (constraints.maxWidth - 40 - (columns - 1) * 12) / columns;
        return CustomScrollView(
          key: const ValueKey('wallpaper-scroll'),
          controller: _scroll,
          shrinkWrap: true,
          scrollCacheExtent: const ScrollCacheExtent.pixels(160),
          slivers: [
            SliverPadding(
              padding: const EdgeInsets.fromLTRB(20, 20, 20, 16),
              sliver: SliverToBoxAdapter(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    PanelHeading(
                      title: '壁纸',
                      onClose: controller.closePanel,
                      actions: [
                        IconButton(
                          tooltip: '返回控制中心',
                          onPressed: () =>
                              controller.selectPanel(ShellPanel.quickSettings),
                          icon: const Icon(Icons.arrow_back_rounded),
                        ),
                        IconButton(
                          tooltip: '刷新壁纸',
                          onPressed: loading || applying
                              ? null
                              : controller.refreshWallpapers,
                          icon: const Icon(Icons.refresh_rounded),
                        ),
                      ],
                    ),
                    const SizedBox(height: 8),
                    Tooltip(
                      message: controller.wallpaperDirectory,
                      child: Text(
                        controller.wallpaperDirectory,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: Theme.of(context).textTheme.bodySmall?.copyWith(
                          color: colors.onSurfaceVariant,
                        ),
                      ),
                    ),
                    const SizedBox(height: 16),
                    TextField(
                      key: const ValueKey('wallpaper-search'),
                      controller: _query,
                      focusNode: _searchFocus,
                      autofocus: true,
                      onChanged: _changed,
                      decoration: InputDecoration(
                        hintText: '搜索文件名',
                        prefixIcon: constraints.maxWidth > 220
                            ? const Icon(Icons.search_rounded)
                            : null,
                        suffixIcon: _query.text.isEmpty
                            ? null
                            : IconButton(
                                tooltip: '清除壁纸搜索',
                                onPressed: () {
                                  _query.clear();
                                  _changed('');
                                  _searchFocus.requestFocus();
                                },
                                icon: const Icon(Icons.close_rounded),
                              ),
                      ),
                    ),
                    const SizedBox(height: 12),
                    Text(
                      controller.demo
                          ? '演示选择 · 不更改桌面壁纸'
                          : available
                          ? '点击图片应用到桌面'
                          : '可浏览图片，暂时无法应用壁纸',
                      style: Theme.of(context).textTheme.bodyMedium,
                    ),
                    const SizedBox(height: 4),
                    Text(
                      query.isEmpty
                          ? '${controller.wallpapers.length} 张静态壁纸 · 支持 JPG、PNG、WebP'
                          : '找到 ${wallpapers.length} 张匹配壁纸',
                      style: Theme.of(context).textTheme.bodySmall?.copyWith(
                        color: colors.onSurfaceVariant,
                      ),
                    ),
                    if (loading || applying) ...[
                      const SizedBox(height: 12),
                      LinearProgressIndicator(
                        semanticsLabel: loading ? '正在读取壁纸' : '正在应用壁纸',
                      ),
                    ],
                    if (!available && !controller.demo)
                      const ShellStatusMessage(text: '壁纸服务不可用，当前可浏览图片'),
                    if (controller.wallpaperError != null)
                      ShellStatusMessage(
                        text: controller.wallpaperError!,
                        error: true,
                      ),
                  ],
                ),
              ),
            ),
            if (wallpapers.isEmpty && !loading)
              SliverPadding(
                padding: const EdgeInsets.fromLTRB(20, 12, 20, 40),
                sliver: SliverToBoxAdapter(
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Icon(
                        query.isEmpty
                            ? Icons.wallpaper_outlined
                            : Icons.search_off_rounded,
                        size: 48,
                        color: colors.onSurfaceVariant,
                      ),
                      const SizedBox(height: 12),
                      Text(
                        controller.wallpaperError != null
                            ? '暂时无法读取壁纸'
                            : query.isEmpty
                            ? '文件夹里还没有可用壁纸'
                            : '没有匹配的壁纸',
                        textAlign: TextAlign.center,
                        style: Theme.of(context).textTheme.titleMedium,
                      ),
                      const SizedBox(height: 8),
                      Text(
                        controller.wallpaperError != null
                            ? '请检查目录后，再刷新列表。'
                            : query.isEmpty
                            ? '将静态图片放入这个目录，再刷新列表。视频暂不在此列表中。'
                            : '试试文件名里的其他关键词。',
                        textAlign: TextAlign.center,
                      ),
                    ],
                  ),
                ),
              )
            else
              SliverPadding(
                padding: const EdgeInsets.fromLTRB(20, 0, 20, 20),
                sliver: SliverGrid.builder(
                  gridDelegate: SliverGridDelegateWithFixedCrossAxisCount(
                    crossAxisCount: columns,
                    crossAxisSpacing: 12,
                    mainAxisSpacing: 12,
                    mainAxisExtent: tileWidth * 9 / 16 + 56,
                  ),
                  itemCount: wallpapers.length,
                  itemBuilder: (context, index) => _WallpaperTile(
                    key: ValueKey(wallpapers[index].path),
                    wallpaper: wallpapers[index],
                    thumbnails: _thumbnails,
                    selected:
                        wallpapers[index].path ==
                        controller.selectedWallpaperPath,
                    enabled: !applying && (available || controller.demo),
                    onPressed: () =>
                        controller.applyWallpaper(wallpapers[index]),
                  ),
                ),
              ),
          ],
        );
      },
    );
  }
}

class _WallpaperTile extends StatefulWidget {
  const _WallpaperTile({
    super.key,
    required this.wallpaper,
    required this.thumbnails,
    required this.selected,
    required this.enabled,
    required this.onPressed,
  });
  final ShellWallpaper wallpaper;
  final WallpaperThumbnailService thumbnails;
  final bool selected;
  final bool enabled;
  final VoidCallback onPressed;
  @override
  State<_WallpaperTile> createState() => _WallpaperTileState();
}

/// FileImage's default path-only key would retain a replaced file's old pixels.
final class _WallpaperFileImage extends FileImage {
  _WallpaperFileImage(ShellWallpaper wallpaper, {File? preview})
    : sizeBytes = wallpaper.sizeBytes,
      modified = wallpaper.modified,
      super(preview ?? File(wallpaper.path));

  final int sizeBytes;
  final DateTime? modified;

  @override
  bool operator ==(Object other) =>
      other is _WallpaperFileImage &&
      other.file.path == file.path &&
      other.scale == scale &&
      other.sizeBytes == sizeBytes &&
      other.modified == modified;

  @override
  int get hashCode => Object.hash(file.path, scale, sizeBytes, modified);
}

class _WallpaperTileState extends State<_WallpaperTile> {
  bool _focused = false;
  bool _previewFailed = false;
  bool _previewFailureScheduled = false;
  int _request = 0;
  WallpaperThumbnail? _thumbnail;

  @override
  void initState() {
    super.initState();
    _loadThumbnail();
  }

  void _loadThumbnail() {
    final request = ++_request;
    if (!widget.thumbnails.enabled) {
      _thumbnail = const WallpaperThumbnail.fallback();
      return;
    }
    _thumbnail = null;
    unawaited(
      widget.thumbnails
          .resolve(
            widget.wallpaper,
            stillWanted: () => mounted && _request == request,
          )
          .then((result) {
            if (!mounted || _request != request) return;
            setState(() {
              _thumbnail = result;
              _previewFailed = result.invalid;
            });
          }),
    );
  }

  @override
  void didUpdateWidget(covariant _WallpaperTile oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.thumbnails, widget.thumbnails) ||
        _WallpaperFileImage(oldWidget.wallpaper) !=
            _WallpaperFileImage(widget.wallpaper)) {
      _previewFailed = false;
      _previewFailureScheduled = false;
      _loadThumbnail();
    }
  }

  void _failedPreview(int request, WallpaperThumbnail thumbnail) {
    if (request != _request) return;
    if (_previewFailed || _previewFailureScheduled) return;
    _previewFailureScheduled = true;
    unawaited(
      widget.thumbnails.reportFailure(
        thumbnail,
        original: thumbnail.file == null,
      ),
    );
    final failedRevision = _WallpaperFileImage(widget.wallpaper);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted ||
          _request != request ||
          _WallpaperFileImage(widget.wallpaper) != failedRevision) {
        return;
      }
      _previewFailureScheduled = false;
      setState(() {
        if (thumbnail.file != null) {
          // The disk cache may have been evicted or damaged. Try the original
          // once; a cache error must never disable an otherwise valid image.
          _thumbnail = thumbnail.original;
        } else {
          _previewFailed = true;
        }
      });
    });
  }

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    final enabled = widget.enabled && !_previewFailed;
    final thumbnail = _thumbnail;
    final request = _request;
    return Semantics(
      selected: widget.selected,
      button: true,
      enabled: enabled,
      label:
          '${widget.wallpaper.name}${widget.selected ? '，当前壁纸' : ''}'
          '${_previewFailed ? '，图片无法预览' : ''}',
      child: Tooltip(
        message: widget.wallpaper.name,
        child: Material(
          color: widget.selected
              ? colors.secondaryContainer
              : colors.surfaceContainerLow,
          borderRadius: BorderRadius.circular(16),
          clipBehavior: Clip.antiAlias,
          child: Stack(
            fit: StackFit.expand,
            children: [
              Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  AspectRatio(
                    aspectRatio: 16 / 9,
                    child: thumbnail == null
                        ? ColoredBox(
                            color: colors.surfaceContainerHighest,
                            child: const Center(
                              child: Icon(Icons.image_outlined),
                            ),
                          )
                        : thumbnail.invalid
                        ? _previewError(context)
                        : Image(
                            key: ValueKey(
                              _WallpaperFileImage(
                                widget.wallpaper,
                                preview: thumbnail.file,
                              ),
                            ),
                            image: ResizeImage.resizeIfNeeded(
                              320,
                              null,
                              _WallpaperFileImage(
                                widget.wallpaper,
                                preview: thumbnail.file,
                              ),
                            ),
                            fit: BoxFit.cover,
                            filterQuality: FilterQuality.low,
                            errorBuilder: (context, _, _) {
                              _failedPreview(request, thumbnail);
                              return _previewError(context);
                            },
                          ),
                  ),
                  Expanded(
                    child: Padding(
                      padding: const EdgeInsets.symmetric(
                        horizontal: 10,
                        vertical: 6,
                      ),
                      child: Align(
                        alignment: Alignment.centerLeft,
                        child: Text(
                          widget.wallpaper.name,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: Theme.of(context).textTheme.labelMedium,
                        ),
                      ),
                    ),
                  ),
                ],
              ),
              if (widget.selected)
                Positioned(
                  top: 8,
                  right: 8,
                  child: DecoratedBox(
                    decoration: BoxDecoration(
                      color: colors.primary,
                      shape: BoxShape.circle,
                    ),
                    child: Padding(
                      padding: const EdgeInsets.all(4),
                      child: Icon(
                        Icons.check_rounded,
                        color: colors.onPrimary,
                        size: 20,
                      ),
                    ),
                  ),
                ),
              Positioned.fill(
                child: Material(
                  color: Colors.transparent,
                  child: InkWell(
                    key: ValueKey('wallpaper-${widget.wallpaper.path}'),
                    borderRadius: BorderRadius.circular(16),
                    onTap: enabled ? widget.onPressed : null,
                    onFocusChange: (value) => setState(() => _focused = value),
                  ),
                ),
              ),
              if (widget.selected || _focused)
                Positioned.fill(
                  child: IgnorePointer(
                    child: DecoratedBox(
                      decoration: BoxDecoration(
                        borderRadius: BorderRadius.circular(16),
                        border: Border.all(color: colors.primary, width: 2),
                      ),
                    ),
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _previewError(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    return ColoredBox(
      color: colors.surfaceContainerHighest,
      child: Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.broken_image_outlined, color: colors.onSurfaceVariant),
            const SizedBox(height: 4),
            Text('图片无法预览', style: Theme.of(context).textTheme.labelSmall),
          ],
        ),
      ),
    );
  }
}
