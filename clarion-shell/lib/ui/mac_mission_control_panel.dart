// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:io';
import 'dart:math' as math;

import 'package:flutter/material.dart';

import 'optional_asset_icon.dart';
import 'package:flutter/services.dart';

import '../models.dart';
import '../state/shell_controller.dart';
import 'mac_app_icon.dart';

/// macOS Catalina Mission Control (调度中心)
///
/// Pixel-perfect macOS reproduction featuring:
/// 1. Top Spaces Bar: full-width edge-to-edge translucent bar with Santa Catalina
///    desktop thumbnail cards and official closebox assets.
/// 2. Active workspace 2px #007aff focus halo and one-click workspace switching.
/// 3. Add Space (+) button with macOS interaction physics to dynamically create spaces.
/// 4. Window Spread (Exposé): spread-out window preview cards with authentic macOS
///    traffic lights, simulated sidebar/canvas hierarchy, official Retina app icons,
///    and #007aff hover focus glow.
/// 5. Cross-space window dragging: drag any window card onto a Space thumbnail card.
class MacMissionControlPanel extends StatefulWidget {
  const MacMissionControlPanel({
    super.key,
    required this.controller,
    required this.onClose,
  });

  final ShellController controller;
  final VoidCallback onClose;

  @override
  State<MacMissionControlPanel> createState() => _MacMissionControlPanelState();
}

class _MacMissionControlPanelState extends State<MacMissionControlPanel>
    with SingleTickerProviderStateMixin {
  late final AnimationController _animController;
  late final Animation<double> _fadeAnim;
  late final Animation<Offset> _spacesSlideAnim;
  final _focusNode = FocusNode(debugLabel: 'mission-control-focus');

  @override
  void initState() {
    super.initState();
    _animController = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 240),
    );
    _fadeAnim = CurvedAnimation(
      parent: _animController,
      curve: Curves.easeOutCubic,
    );
    _spacesSlideAnim =
        Tween<Offset>(begin: const Offset(0, -0.15), end: Offset.zero).animate(
          CurvedAnimation(parent: _animController, curve: Curves.easeOutCubic),
        );
    _animController.forward();

    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) {
        _focusNode.requestFocus();
      }
    });
  }

  @override
  void dispose() {
    _animController.dispose();
    _focusNode.dispose();
    super.dispose();
  }

  void _handleKey(KeyEvent event) {
    if (event is! KeyDownEvent) return;
    if (event.logicalKey == LogicalKeyboardKey.escape) {
      widget.onClose();
    }
  }

  DesktopApp? _findApp(ShellWindow window) {
    for (final app in widget.controller.apps) {
      if (app.id == window.appId ||
          app.desktopFile == window.appId ||
          app.id.toLowerCase() == window.appId.toLowerCase()) {
        return app;
      }
    }
    return null;
  }

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: widget.controller,
      builder: (context, _) {
        final dark = Theme.of(context).brightness == Brightness.dark;
        final workspaces = widget.controller.workspaces;
        final windows = widget.controller.windows;

        final activeWs = workspaces.firstWhere(
          (ws) => ws.isFocused || ws.isActive,
          orElse: () => workspaces.isNotEmpty
              ? workspaces.first
              : const ShellWorkspace(id: 1, index: 1),
        );

        final activeWindows = windows
            .where((w) => w.workspaceId == activeWs.id && !w.isMinimized)
            .toList();
        final displayWindows = activeWindows.isNotEmpty
            ? activeWindows
            : windows.where((w) => !w.isMinimized).toList();

        return KeyboardListener(
          focusNode: _focusNode,
          onKeyEvent: _handleKey,
          child: FadeTransition(
            opacity: _fadeAnim,
            child: GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTap: widget.onClose,
              child: Container(
                decoration: BoxDecoration(
                  gradient: RadialGradient(
                    center: Alignment.center,
                    radius: 1.25,
                    colors: dark
                        ? [
                            Colors.black.withValues(alpha: 0.35),
                            Colors.black.withValues(alpha: 0.60),
                          ]
                        : [
                            Colors.black.withValues(alpha: 0.12),
                            Colors.black.withValues(alpha: 0.32),
                          ],
                  ),
                ),
                child: SafeArea(
                  top: false,
                  child: Column(
                    children: [
                      // 1. Top Spaces Bar (Edge-to-edge macOS full width strip)
                      SlideTransition(
                        position: _spacesSlideAnim,
                        child: _MacSpacesBar(
                          key: const ValueKey('mac-spaces-bar'),
                          controller: widget.controller,
                          workspaces: workspaces,
                          activeWorkspaceId: activeWs.id,
                          windows: windows,
                          dark: dark,
                          onSwitchWorkspace: (id) {
                            widget.controller.switchWorkspace(id);
                            widget.onClose();
                          },
                          onClosePanel: widget.onClose,
                        ),
                      ),

                      // 2. Window Spread (Exposé)
                      Expanded(
                        child: Padding(
                          padding: const EdgeInsets.symmetric(
                            horizontal: 24,
                            vertical: 16,
                          ),
                          child: _WindowSpreadArea(
                            key: const ValueKey('mac-window-spread'),
                            controller: widget.controller,
                            windows: displayWindows,
                            activeWorkspace: activeWs,
                            dark: dark,
                            onSelectWindow: (w) {
                              widget.controller.focusWindow(w.id);
                              widget.onClose();
                            },
                            findApp: _findApp,
                          ),
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}

/// The top Spaces bar showing all virtual desktops with miniature previews.
class _MacSpacesBar extends StatelessWidget {
  const _MacSpacesBar({
    super.key,
    required this.controller,
    required this.workspaces,
    required this.activeWorkspaceId,
    required this.windows,
    required this.dark,
    required this.onSwitchWorkspace,
    required this.onClosePanel,
  });

  final ShellController controller;
  final List<ShellWorkspace> workspaces;
  final int activeWorkspaceId;
  final List<ShellWindow> windows;
  final bool dark;
  final ValueChanged<int> onSwitchWorkspace;
  final VoidCallback onClosePanel;

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: () {}, // Prevent taps on bar background from closing
      child: Container(
        width: double.infinity,
        height: 128,
        decoration: BoxDecoration(
          color: dark ? const Color(0xd916161b) : const Color(0xd8f0f0f4),
          border: Border(
            bottom: BorderSide(
              color: dark
                  ? Colors.white.withValues(alpha: 0.12)
                  : Colors.black.withValues(alpha: 0.10),
              width: 0.5,
            ),
          ),
          boxShadow: [
            BoxShadow(
              color: Colors.black.withValues(alpha: dark ? 0.35 : 0.14),
              blurRadius: 20,
              offset: const Offset(0, 4),
            ),
          ],
        ),
        padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 8),
        alignment: Alignment.center,
        child: SingleChildScrollView(
          scrollDirection: Axis.horizontal,
          child: Row(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.center,
            children: [
              for (final ws in workspaces)
                Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 7),
                  child: _SpaceCard(
                    key: ValueKey('mac-space-card-${ws.id}'),
                    controller: controller,
                    workspace: ws,
                    isActive: ws.id == activeWorkspaceId,
                    windows: windows
                        .where((w) => w.workspaceId == ws.id)
                        .toList(),
                    dark: dark,
                    canRemove: workspaces.length > 1,
                    onTap: () => onSwitchWorkspace(ws.id),
                    onRemove: () => controller.removeWorkspace(ws.id),
                    onWindowDropped: (window) =>
                        controller.moveWindowToWorkspace(window.id, ws.id),
                  ),
                ),
              const SizedBox(width: 6),
              // Add Space Button (+)
              _AddSpaceButton(dark: dark, onTap: controller.createWorkspace),
            ],
          ),
        ),
      ),
    );
  }
}

/// A thumbnail card representing one Space/Desktop.
class _SpaceCard extends StatefulWidget {
  const _SpaceCard({
    super.key,
    this.controller,
    required this.workspace,
    required this.isActive,
    required this.windows,
    required this.dark,
    required this.canRemove,
    required this.onTap,
    required this.onRemove,
    required this.onWindowDropped,
  });

  final ShellController? controller;
  final ShellWorkspace workspace;
  final bool isActive;
  final List<ShellWindow> windows;
  final bool dark;
  final bool canRemove;
  final VoidCallback onTap;
  final VoidCallback onRemove;
  final ValueChanged<ShellWindow> onWindowDropped;

  @override
  State<_SpaceCard> createState() => _SpaceCardState();
}

class _SpaceCardState extends State<_SpaceCard> {
  bool _hovered = false;
  bool _closePressed = false;

  @override
  Widget build(BuildContext context) {
    final title = widget.workspace.name ?? '桌面 ${widget.workspace.index}';

    return DragTarget<ShellWindow>(
      onWillAcceptWithDetails: (_) => true,
      onAcceptWithDetails: (details) {
        widget.onWindowDropped(details.data);
      },
      builder: (context, candidateData, rejectedData) {
        final isDropTarget = candidateData.isNotEmpty;

        return MouseRegion(
          onEnter: (_) => setState(() => _hovered = true),
          onExit: (_) => setState(() => _hovered = false),
          child: GestureDetector(
            onTap: widget.onTap,
            child: AnimatedScale(
              scale: isDropTarget ? 1.06 : (_hovered ? 1.02 : 1.0),
              duration: const Duration(milliseconds: 140),
              curve: Curves.easeOutCubic,
              child: SizedBox(
                width: 140,
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    // Space Title
                    Text(
                      title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontFamily: 'ClarionSystem',
                        fontFamilyFallback: const ['ClarionChinese'],
                        fontSize: 11,
                        fontWeight: FontWeight.w500,
                        color: widget.isActive
                            ? const Color(0xff007aff)
                            : (widget.dark ? Colors.white70 : Colors.black87),
                      ),
                    ),
                    const SizedBox(height: 5),

                    // Space Thumbnail Container
                    Container(
                      width: 140,
                      height: 80,
                      decoration: BoxDecoration(
                        gradient: LinearGradient(
                          begin: Alignment.topLeft,
                          end: Alignment.bottomRight,
                          colors: widget.dark
                              ? const [
                                  Color(0xff12243d),
                                  Color(0xff1c3960),
                                  Color(0xff2a5182),
                                ]
                              : const [
                                  Color(0xff4a82b8),
                                  Color(0xff7ca6d4),
                                  Color(0xffa8c6ea),
                                ],
                        ),
                        borderRadius: BorderRadius.circular(6),
                        border: Border.all(
                          color: isDropTarget
                              ? const Color(0xff007aff)
                              : widget.isActive
                              ? const Color(0xff007aff)
                              : (widget.dark
                                    ? Colors.white.withValues(alpha: 0.16)
                                    : Colors.black.withValues(alpha: 0.12)),
                          width: (isDropTarget || widget.isActive) ? 2.0 : 0.5,
                        ),
                        boxShadow: [
                          if (isDropTarget)
                            BoxShadow(
                              color: const Color(
                                0xff007aff,
                              ).withValues(alpha: 0.65),
                              blurRadius: 16,
                              spreadRadius: 2,
                            )
                          else if (widget.isActive)
                            BoxShadow(
                              color: const Color(
                                0xff007aff,
                              ).withValues(alpha: 0.40),
                              blurRadius: 10,
                              offset: const Offset(0, 2),
                            )
                          else if (_hovered)
                            BoxShadow(
                              color: Colors.black.withValues(alpha: 0.25),
                              blurRadius: 8,
                              offset: const Offset(0, 2),
                            ),
                        ],
                      ),
                      child: Stack(
                        clipBehavior: Clip.none,
                        children: [
                          // Miniature bottom Dock line
                          Positioned(
                            bottom: 2,
                            left: 45,
                            width: 50,
                            height: 2,
                            child: DecoratedBox(
                              decoration: BoxDecoration(
                                color: Colors.white.withValues(alpha: 0.45),
                                borderRadius: BorderRadius.circular(1),
                              ),
                            ),
                          ),

                          // Mini windows inside space card
                          if (widget.windows.isNotEmpty)
                            Positioned.fill(
                              child: Padding(
                                padding: const EdgeInsets.fromLTRB(8, 8, 8, 10),
                                child: Wrap(
                                  spacing: 4,
                                  runSpacing: 4,
                                  children: [
                                    for (final w in widget.windows.take(4))
                                      _buildMiniWindow(w),
                                  ],
                                ),
                              ),
                            )
                          else
                            Center(
                              child: Icon(
                                Icons.desktop_windows_rounded,
                                size: 20,
                                color: widget.dark
                                    ? Colors.white.withValues(alpha: 0.22)
                                    : Colors.black.withValues(alpha: 0.16),
                              ),
                            ),

                          // Drop target indicator overlay
                          if (isDropTarget)
                            Positioned.fill(
                              child: Container(
                                decoration: BoxDecoration(
                                  color: const Color(
                                    0xff007aff,
                                  ).withValues(alpha: 0.28),
                                  borderRadius: BorderRadius.circular(5.5),
                                ),
                                child: const Center(
                                  child: Icon(
                                    Icons.file_download_rounded,
                                    color: Colors.white,
                                    size: 24,
                                  ),
                                ),
                              ),
                            ),

                          // Close (×) button on hover using official Catalina closebox
                          if (_hovered && widget.canRemove && !isDropTarget)
                            Positioned(
                              top: -4,
                              left: -4,
                              child: GestureDetector(
                                onTap: widget.onRemove,
                                onTapDown: (_) =>
                                    setState(() => _closePressed = true),
                                onTapUp: (_) =>
                                    setState(() => _closePressed = false),
                                onTapCancel: () =>
                                    setState(() => _closePressed = false),
                                child: OptionalAssetIcon(
                                  fallback: Icons.cancel_rounded,
                                  fallbackColor: Colors.white,
                                  asset: _closePressed
                                      ? 'assets/shell/closebox-pressed@2x.png'
                                      : 'assets/shell/closebox@2x.png',
                                  width: 18,
                                  height: 18,
                                ),
                              ),
                            ),
                        ],
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        );
      },
    );
  }

  Widget _buildMiniWindow(ShellWindow w) {
    final previewPath = widget.controller?.windowPreviews[w.id];

    if (previewPath != null) {
      return Container(
        width: 28,
        height: 20,
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(3),
          border: Border.all(
            color: Colors.white.withValues(alpha: 0.50),
            width: 0.5,
          ),
          boxShadow: const [
            BoxShadow(
              color: Colors.black26,
              blurRadius: 3,
              offset: Offset(0, 1),
            ),
          ],
        ),
        child: ClipRRect(
          borderRadius: BorderRadius.circular(2.5),
          child: Image.file(
            File(previewPath),
            cacheWidth: 112,
            key: ValueKey(
              'space-mini-${w.id}-${widget.controller?.windowPreviewRevision ?? 0}',
            ),
            fit: BoxFit.cover,
            filterQuality: FilterQuality.low,
            errorBuilder: (context, error, stackTrace) =>
                _buildFallbackMiniWindow(),
          ),
        ),
      );
    }
    return _buildFallbackMiniWindow();
  }

  Widget _buildFallbackMiniWindow() {
    return Container(
      width: 28,
      height: 20,
      decoration: BoxDecoration(
        color: Colors.white.withValues(alpha: 0.28),
        borderRadius: BorderRadius.circular(3),
        border: Border.all(
          color: Colors.white.withValues(alpha: 0.45),
          width: 0.5,
        ),
        boxShadow: const [
          BoxShadow(color: Colors.black12, blurRadius: 2, offset: Offset(0, 1)),
        ],
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Container(
            height: 4,
            decoration: BoxDecoration(
              color: Colors.white.withValues(alpha: 0.35),
              borderRadius: const BorderRadius.vertical(
                top: Radius.circular(2.5),
              ),
            ),
          ),
          Expanded(
            child: Center(
              child: Container(
                width: 5,
                height: 5,
                decoration: const BoxDecoration(
                  color: Colors.white,
                  shape: BoxShape.circle,
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// The "+" button to create a new space.
class _AddSpaceButton extends StatefulWidget {
  const _AddSpaceButton({required this.dark, required this.onTap});

  final bool dark;
  final VoidCallback onTap;

  @override
  State<_AddSpaceButton> createState() => _AddSpaceButtonState();
}

class _AddSpaceButtonState extends State<_AddSpaceButton> {
  bool _hovered = false;
  bool _pressed = false;

  @override
  Widget build(BuildContext context) {
    return MouseRegion(
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        key: const ValueKey('mac-space-add-button'),
        onTap: widget.onTap,
        onTapDown: (_) => setState(() => _pressed = true),
        onTapUp: (_) => setState(() => _pressed = false),
        onTapCancel: () => setState(() => _pressed = false),
        child: Padding(
          padding: const EdgeInsets.only(top: 20),
          child: AnimatedScale(
            scale: _pressed ? 0.95 : (_hovered ? 1.04 : 1.0),
            duration: const Duration(milliseconds: 120),
            child: Container(
              width: 44,
              height: 80,
              decoration: BoxDecoration(
                color: _hovered
                    ? (widget.dark
                          ? Colors.white.withValues(alpha: 0.16)
                          : Colors.black.withValues(alpha: 0.08))
                    : (widget.dark
                          ? Colors.white.withValues(alpha: 0.08)
                          : Colors.black.withValues(alpha: 0.04)),
                borderRadius: BorderRadius.circular(8),
                border: Border.all(
                  color: widget.dark
                      ? Colors.white.withValues(alpha: 0.20)
                      : Colors.black.withValues(alpha: 0.12),
                  width: 0.5,
                ),
                boxShadow: _hovered
                    ? [
                        BoxShadow(
                          color: Colors.black.withValues(alpha: 0.15),
                          blurRadius: 8,
                          offset: const Offset(0, 2),
                        ),
                      ]
                    : null,
              ),
              child: Center(
                child: Icon(
                  Icons.add_rounded,
                  size: 22,
                  color: widget.dark
                      ? (_hovered ? Colors.white : Colors.white70)
                      : (_hovered ? Colors.black87 : Colors.black54),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// The main area below the spaces bar showing spread out windows on the active space.
class _WindowSpreadArea extends StatelessWidget {
  const _WindowSpreadArea({
    super.key,
    required this.controller,
    required this.windows,
    required this.activeWorkspace,
    required this.dark,
    required this.onSelectWindow,
    required this.findApp,
  });

  final ShellController controller;
  final List<ShellWindow> windows;
  final ShellWorkspace activeWorkspace;
  final bool dark;
  final ValueChanged<ShellWindow> onSelectWindow;
  final DesktopApp? Function(ShellWindow) findApp;

  @override
  Widget build(BuildContext context) {
    if (windows.isEmpty) {
      return Center(
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 28, vertical: 22),
          decoration: BoxDecoration(
            color: dark ? const Color(0xd0202026) : const Color(0xd8ffffff),
            borderRadius: BorderRadius.circular(14),
            border: Border.all(
              color: dark
                  ? Colors.white.withValues(alpha: 0.14)
                  : Colors.black.withValues(alpha: 0.08),
              width: 0.5,
            ),
            boxShadow: [
              BoxShadow(
                color: Colors.black.withValues(alpha: 0.22),
                blurRadius: 24,
                offset: const Offset(0, 8),
              ),
            ],
          ),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              OptionalAssetIcon(
                asset: 'assets/icons/mission-control.svg',
                fallback: Icons.dashboard_rounded,
                width: 52,
                height: 52,
              ),
              const SizedBox(height: 14),
              Text(
                '调度中心',
                style: TextStyle(
                  fontFamily: 'ClarionSystem',
                  fontFamilyFallback: const ['ClarionChinese'],
                  fontSize: 15,
                  fontWeight: FontWeight.w600,
                  color: dark ? Colors.white : Colors.black87,
                ),
              ),
              const SizedBox(height: 4),
              Text(
                '此桌面无打开的窗口',
                style: TextStyle(
                  fontFamily: 'ClarionSystem',
                  fontFamilyFallback: const ['ClarionChinese'],
                  fontSize: 12,
                  color: dark ? Colors.white60 : Colors.black54,
                ),
              ),
            ],
          ),
        ),
      );
    }

    return LayoutBuilder(
      builder: (context, constraints) {
        final count = windows.length;
        // Dynamically compute card size based on count and available space
        final cardWidth = count == 1
            ? math.min(540.0, constraints.maxWidth - 48)
            : count <= 4
            ? math.min(360.0, (constraints.maxWidth - 64) / 2)
            : math.min(290.0, (constraints.maxWidth - 88) / 3);
        final cardHeight = cardWidth * 0.64;

        return SingleChildScrollView(
          child: Center(
            child: Wrap(
              alignment: WrapAlignment.center,
              spacing: 28,
              runSpacing: 28,
              children: [
                for (final w in windows)
                  _SpreadWindowCard(
                    key: ValueKey('mac-window-card-${w.id}'),
                    controller: controller,
                    window: w,
                    app: findApp(w),
                    width: cardWidth,
                    height: cardHeight,
                    dark: dark,
                    onTap: () => onSelectWindow(w),
                  ),
              ],
            ),
          ),
        );
      },
    );
  }
}

/// A draggable window preview card with macOS traffic lights, app icon, and shadow.
class _SpreadWindowCard extends StatefulWidget {
  const _SpreadWindowCard({
    super.key,
    required this.controller,
    required this.window,
    required this.app,
    required this.width,
    required this.height,
    required this.dark,
    required this.onTap,
  });

  final ShellController controller;
  final ShellWindow window;
  final DesktopApp? app;
  final double width;
  final double height;
  final bool dark;
  final VoidCallback onTap;

  @override
  State<_SpreadWindowCard> createState() => _SpreadWindowCardState();
}

class _SpreadWindowCardState extends State<_SpreadWindowCard> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) {
    final title = widget.window.title.isNotEmpty
        ? widget.window.title
        : (widget.app?.name ?? widget.window.appId);

    final cardContent = Container(
      width: widget.width,
      height: widget.height,
      decoration: BoxDecoration(
        color: widget.dark
            ? const Color(0xff222227).withValues(alpha: 0.96)
            : const Color(0xfff7f7fa).withValues(alpha: 0.96),
        borderRadius: BorderRadius.circular(10),
        border: Border.all(
          color: _hovered
              ? const Color(0xff007aff)
              : (widget.dark
                    ? Colors.white.withValues(alpha: 0.16)
                    : Colors.black.withValues(alpha: 0.12)),
          width: _hovered ? 1.5 : 0.5,
        ),
        boxShadow: [
          if (_hovered) ...[
            BoxShadow(
              color: const Color(0xff007aff).withValues(alpha: 0.35),
              blurRadius: 24,
              spreadRadius: 1.5,
            ),
            BoxShadow(
              color: Colors.black.withValues(alpha: 0.40),
              blurRadius: 36,
              offset: const Offset(0, 14),
            ),
          ] else
            BoxShadow(
              color: Colors.black.withValues(alpha: widget.dark ? 0.32 : 0.18),
              blurRadius: 20,
              offset: const Offset(0, 8),
            ),
        ],
      ),
      child: Column(
        children: [
          // macOS Title Bar with Traffic Lights
          Container(
            height: 30,
            padding: const EdgeInsets.symmetric(horizontal: 12),
            decoration: BoxDecoration(
              color: widget.dark
                  ? const Color(0xff2d2d32)
                  : const Color(0xffececef),
              borderRadius: const BorderRadius.vertical(
                top: Radius.circular(9.5),
              ),
              border: Border(
                bottom: BorderSide(
                  color: widget.dark
                      ? Colors.white.withValues(alpha: 0.08)
                      : Colors.black.withValues(alpha: 0.08),
                  width: 0.5,
                ),
              ),
            ),
            child: Row(
              children: [
                // Traffic light dots with 0.5px subtle border
                Container(
                  width: 10,
                  height: 10,
                  decoration: BoxDecoration(
                    color: const Color(0xffff5f56),
                    shape: BoxShape.circle,
                    border: Border.all(
                      color: const Color(0xffe0443e),
                      width: 0.5,
                    ),
                  ),
                ),
                const SizedBox(width: 6),
                Container(
                  width: 10,
                  height: 10,
                  decoration: BoxDecoration(
                    color: const Color(0xffffbd2e),
                    shape: BoxShape.circle,
                    border: Border.all(
                      color: const Color(0xffdea123),
                      width: 0.5,
                    ),
                  ),
                ),
                const SizedBox(width: 6),
                Container(
                  width: 10,
                  height: 10,
                  decoration: BoxDecoration(
                    color: const Color(0xff27c93f),
                    shape: BoxShape.circle,
                    border: Border.all(
                      color: const Color(0xff1aab29),
                      width: 0.5,
                    ),
                  ),
                ),
                const SizedBox(width: 12),
                // Small proxy app icon in title bar
                if (widget.app != null) ...[
                  MacAppIcon(app: widget.app!, size: 14),
                  const SizedBox(width: 6),
                ],
                Expanded(
                  child: Text(
                    title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    textAlign: TextAlign.center,
                    style: TextStyle(
                      fontFamily: 'ClarionSystem',
                      fontFamilyFallback: const ['ClarionChinese'],
                      fontSize: 11,
                      fontWeight: FontWeight.w500,
                      color: widget.dark ? Colors.white70 : Colors.black87,
                    ),
                  ),
                ),
                const SizedBox(width: 44), // balance traffic lights width
              ],
            ),
          ),

          // Window Body: Live preview snapshot if available, else simulated fallback
          Expanded(child: _buildWindowBody(title)),
        ],
      ),
    );

    return Draggable<ShellWindow>(
      data: widget.window,
      feedback: Transform.rotate(
        angle: -0.02,
        child: Material(
          color: Colors.transparent,
          child: Opacity(
            opacity: 0.92,
            child: SizedBox(
              width: widget.width * 0.85,
              height: widget.height * 0.85,
              child: cardContent,
            ),
          ),
        ),
      ),
      childWhenDragging: Opacity(opacity: 0.20, child: cardContent),
      child: MouseRegion(
        onEnter: (_) => setState(() => _hovered = true),
        onExit: (_) => setState(() => _hovered = false),
        child: GestureDetector(
          onTap: widget.onTap,
          child: AnimatedScale(
            scale: _hovered ? 1.025 : 1.0,
            duration: const Duration(milliseconds: 140),
            curve: Curves.easeOutCubic,
            child: cardContent,
          ),
        ),
      ),
    );
  }

  Widget _buildWindowBody(String title) {
    final previewPath = widget.controller.windowPreviews[widget.window.id];

    if (previewPath == null) {
      return _buildSimulatedBody(title);
    }

    return Stack(
      fit: StackFit.expand,
      children: [
        // Solid window backdrop matching macOS dark/light window tone
        DecoratedBox(
          decoration: BoxDecoration(
            color: widget.dark
                ? const Color(0xff18181c)
                : const Color(0xfff0f0f4),
            borderRadius: const BorderRadius.vertical(
              bottom: Radius.circular(9.5),
            ),
          ),
        ),

        // Real window live render snapshot
        ClipRRect(
          borderRadius: const BorderRadius.vertical(
            bottom: Radius.circular(9.5),
          ),
          child: Image.file(
            File(previewPath),
            cacheWidth: 768,
            key: ValueKey(
              'win-preview-${widget.window.id}-${widget.controller.windowPreviewRevision}',
            ),
            fit: BoxFit.contain,
            alignment: Alignment.center,
            filterQuality: FilterQuality.medium,
            errorBuilder: (context, error, stackTrace) =>
                _buildSimulatedBody(title),
          ),
        ),

        // Floating macOS Exposé App Badge at bottom-center
        Positioned(
          bottom: 10,
          left: 12,
          right: 12,
          child: Center(
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
              decoration: BoxDecoration(
                color: widget.dark
                    ? const Color(0xd91e1e22)
                    : const Color(0xd9f5f5f7),
                borderRadius: BorderRadius.circular(14),
                border: Border.all(
                  color: widget.dark
                      ? Colors.white.withValues(alpha: 0.16)
                      : Colors.black.withValues(alpha: 0.12),
                  width: 0.5,
                ),
                boxShadow: [
                  BoxShadow(
                    color: Colors.black.withValues(
                      alpha: widget.dark ? 0.35 : 0.18,
                    ),
                    blurRadius: 10,
                    offset: const Offset(0, 3),
                  ),
                ],
              ),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  if (widget.app != null) ...[
                    MacAppIcon(app: widget.app!, size: 18),
                    const SizedBox(width: 6),
                  ],
                  Flexible(
                    child: Text(
                      widget.app?.name ?? title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontFamily: 'ClarionSystem',
                        fontFamilyFallback: const ['ClarionChinese'],
                        fontSize: 11,
                        fontWeight: FontWeight.w600,
                        color: widget.dark ? Colors.white : Colors.black87,
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ],
    );
  }

  Widget _buildSimulatedBody(String title) {
    return Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // Left simulated sidebar
        Container(
          width: math.max(48.0, widget.width * 0.22),
          decoration: BoxDecoration(
            color: widget.dark
                ? Colors.white.withValues(alpha: 0.03)
                : Colors.black.withValues(alpha: 0.02),
            border: Border(
              right: BorderSide(
                color: widget.dark
                    ? Colors.white.withValues(alpha: 0.06)
                    : Colors.black.withValues(alpha: 0.06),
                width: 0.5,
              ),
            ),
            borderRadius: const BorderRadius.only(
              bottomLeft: Radius.circular(9.5),
            ),
          ),
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 12),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              for (var i = 0; i < 3; i++) ...[
                Container(
                  height: 6,
                  width: (30 + i * 12).toDouble(),
                  margin: const EdgeInsets.only(bottom: 8),
                  decoration: BoxDecoration(
                    color: widget.dark
                        ? Colors.white.withValues(alpha: 0.08)
                        : Colors.black.withValues(alpha: 0.06),
                    borderRadius: BorderRadius.circular(3),
                  ),
                ),
              ],
            ],
          ),
        ),

        // Main Content Canvas
        Expanded(
          child: Center(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                if (widget.app != null)
                  DecoratedBox(
                    decoration: BoxDecoration(
                      boxShadow: [
                        BoxShadow(
                          color: Colors.black.withValues(alpha: 0.22),
                          blurRadius: 14,
                          offset: const Offset(0, 4),
                        ),
                      ],
                    ),
                    child: MacAppIcon(app: widget.app!, size: 52),
                  )
                else
                  Icon(
                    Icons.desktop_windows_rounded,
                    size: 48,
                    color: widget.dark ? Colors.white60 : Colors.black45,
                  ),
                const SizedBox(height: 10),
                Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 16),
                  child: Text(
                    widget.app?.name ?? title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      fontFamily: 'ClarionSystem',
                      fontFamilyFallback: const ['ClarionChinese'],
                      fontSize: 13,
                      fontWeight: FontWeight.w600,
                      color: widget.dark ? Colors.white : Colors.black87,
                    ),
                  ),
                ),
                if (widget.app?.name != null && title != widget.app!.name) ...[
                  const SizedBox(height: 2),
                  Padding(
                    padding: const EdgeInsets.symmetric(horizontal: 16),
                    child: Text(
                      title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontFamily: 'ClarionSystem',
                        fontFamilyFallback: const ['ClarionChinese'],
                        fontSize: 11,
                        color: widget.dark ? Colors.white60 : Colors.black54,
                      ),
                    ),
                  ),
                ],
              ],
            ),
          ),
        ),
      ],
    );
  }
}
