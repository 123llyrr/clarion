// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../services/system_information_service.dart';
import '../state/shell_controller.dart';
import 'optional_asset_icon.dart';

/// Clarion system information in the selected desktop appearance.
class MacAboutDialog extends StatefulWidget {
  const MacAboutDialog({super.key, required this.information, this.controller});

  final Future<SystemInformation> information;
  final ShellController? controller;

  @override
  State<MacAboutDialog> createState() => _MacAboutDialogState();
}

class _MacAboutDialogState extends State<MacAboutDialog> {
  int _selectedTab = 0;
  bool _closeHovered = false;

  static const _tabs = ['概览', '显示器', '储存空间'];

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final dark = theme.brightness == Brightness.dark;

    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.escape): () =>
            Navigator.pop(context),
      },
      child: LayoutBuilder(
        builder: (context, constraints) {
          final isSmallScreen = constraints.maxHeight < 340;
          final dialogWidth = math.min(540.0, constraints.maxWidth - 24);
          final dialogMaxHeight = math.min(420.0, constraints.maxHeight - 24);

          return AlertDialog(
            backgroundColor: dark
                ? const Color(0xff2d2d2f)
                : const Color(0xfff6f6f8),
            shape: RoundedRectangleBorder(
              borderRadius: BorderRadius.circular(10),
              side: BorderSide(
                color: dark
                    ? Colors.white.withValues(alpha: 0.12)
                    : Colors.black.withValues(alpha: 0.10),
                width: 0.8,
              ),
            ),
            elevation: 20,
            shadowColor: Colors.black.withValues(alpha: 0.4),
            insetPadding: const EdgeInsets.symmetric(
              horizontal: 12,
              vertical: 12,
            ),
            contentPadding: EdgeInsets.zero,
            titlePadding: EdgeInsets.zero,
            actionsPadding: EdgeInsets.zero,
            clipBehavior: Clip.antiAlias,
            content: ConstrainedBox(
              constraints: BoxConstraints(
                maxWidth: dialogWidth,
                maxHeight: dialogMaxHeight,
              ),
              child: FutureBuilder<SystemInformation>(
                future: widget.information,
                builder: (context, snapshot) {
                  final info = snapshot.data;
                  final isLoading =
                      snapshot.connectionState == ConnectionState.waiting;
                  final isError = snapshot.hasError;

                  return Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      // Title bar with traffic lights & segmented tabs
                      _buildHeader(dark, isSmallScreen),

                      // Dialog Content
                      Flexible(
                        child: SingleChildScrollView(
                          padding: EdgeInsets.symmetric(
                            horizontal: isSmallScreen ? 14 : 22,
                            vertical: isSmallScreen ? 8 : 14,
                          ),
                          child: isLoading
                              ? _buildLoading(dark)
                              : isError
                              ? _buildError(dark)
                              : _buildTabContent(info!, dark, isSmallScreen),
                        ),
                      ),

                      // Dialog footer
                      _buildFooter(dark, isSmallScreen),
                    ],
                  );
                },
              ),
            ),
          );
        },
      ),
    );
  }

  Widget _buildHeader(bool dark, bool isSmallScreen) {
    return Container(
      height: 38,
      padding: const EdgeInsets.symmetric(horizontal: 14),
      decoration: BoxDecoration(
        color: dark ? const Color(0xff262628) : const Color(0xffececef),
        border: Border(
          bottom: BorderSide(
            color: dark
                ? Colors.white.withValues(alpha: 0.08)
                : Colors.black.withValues(alpha: 0.08),
            width: 0.5,
          ),
        ),
      ),
      child: Row(
        children: [
          // Traffic Lights
          MouseRegion(
            onEnter: (_) => setState(() => _closeHovered = true),
            onExit: (_) => setState(() => _closeHovered = false),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                GestureDetector(
                  onTap: () => Navigator.pop(context),
                  child: Container(
                    width: 12,
                    height: 12,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      color: const Color(0xffff5f56),
                      border: Border.all(
                        color: const Color(0xffe0443e),
                        width: 0.5,
                      ),
                    ),
                    alignment: Alignment.center,
                    child: _closeHovered
                        ? const Icon(
                            Icons.close,
                            size: 8,
                            color: Color(0xff4c0000),
                          )
                        : null,
                  ),
                ),
                const SizedBox(width: 8),
                Container(
                  width: 12,
                  height: 12,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: const Color(0xffffbd2e),
                    border: Border.all(
                      color: const Color(0xffdea123),
                      width: 0.5,
                    ),
                  ),
                ),
                const SizedBox(width: 8),
                Container(
                  width: 12,
                  height: 12,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: const Color(0xff27c93f),
                    border: Border.all(
                      color: const Color(0xff1aab29),
                      width: 0.5,
                    ),
                  ),
                ),
              ],
            ),
          ),

          const Spacer(),

          // Segmented Tabs
          Container(
            height: 24,
            padding: const EdgeInsets.all(2),
            decoration: BoxDecoration(
              color: dark ? const Color(0xff1e1e20) : const Color(0xffdbdbdf),
              borderRadius: BorderRadius.circular(6),
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                for (var i = 0; i < _tabs.length; i++)
                  GestureDetector(
                    behavior: HitTestBehavior.opaque,
                    onTap: () => setState(() => _selectedTab = i),
                    child: Container(
                      padding: const EdgeInsets.symmetric(horizontal: 10),
                      alignment: Alignment.center,
                      decoration: BoxDecoration(
                        color: _selectedTab == i
                            ? (dark ? const Color(0xff3a3a3c) : Colors.white)
                            : Colors.transparent,
                        borderRadius: BorderRadius.circular(4.5),
                        boxShadow: _selectedTab == i
                            ? [
                                BoxShadow(
                                  color: Colors.black.withValues(alpha: 0.12),
                                  blurRadius: 2,
                                  offset: const Offset(0, 1),
                                ),
                              ]
                            : null,
                      ),
                      child: Text(
                        _tabs[i],
                        style: TextStyle(
                          fontSize: 11,
                          fontWeight: _selectedTab == i
                              ? FontWeight.w600
                              : FontWeight.w400,
                          color: _selectedTab == i
                              ? (dark ? Colors.white : Colors.black87)
                              : (dark ? Colors.white60 : Colors.black54),
                        ),
                      ),
                    ),
                  ),
              ],
            ),
          ),

          const Spacer(),
          const SizedBox(width: 52), // Balance traffic lights
        ],
      ),
    );
  }

  Widget _buildLoading(bool dark) {
    return const Padding(
      padding: EdgeInsets.symmetric(vertical: 40),
      child: Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            SizedBox(
              width: 24,
              height: 24,
              child: CircularProgressIndicator(strokeWidth: 2),
            ),
            SizedBox(height: 12),
            Text('正在读取系统信息…', style: TextStyle(fontSize: 12)),
          ],
        ),
      ),
    );
  }

  Widget _buildError(bool dark) {
    return const Padding(
      padding: EdgeInsets.symmetric(vertical: 40),
      child: Center(
        child: Text(
          '无法读取系统信息',
          style: TextStyle(fontSize: 13, color: Colors.redAccent),
        ),
      ),
    );
  }

  Widget _buildTabContent(
    SystemInformation info,
    bool dark,
    bool isSmallScreen,
  ) {
    return switch (_selectedTab) {
      0 => _buildOverviewTab(info, dark, isSmallScreen),
      1 => _buildDisplaysTab(info, dark, isSmallScreen),
      2 => _buildStorageTab(info, dark, isSmallScreen),
      _ => _buildOverviewTab(info, dark, isSmallScreen),
    };
  }

  Widget _buildOverviewTab(
    SystemInformation info,
    bool dark,
    bool isSmallScreen,
  ) {
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        // Clarion identity uses no proprietary artwork.
        SizedBox(
          width: isSmallScreen ? 110 : 140,
          child: Center(
            child: Container(
              width: isSmallScreen ? 94 : 120,
              height: isSmallScreen ? 94 : 120,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                boxShadow: [
                  BoxShadow(
                    color: Colors.black.withValues(alpha: 0.26),
                    blurRadius: 14,
                    offset: const Offset(0, 4),
                  ),
                ],
              ),
              child: Center(
                child: Image.asset(
                  dark
                      ? 'assets/shell/clarion-mark-white.png'
                      : 'assets/shell/clarion-mark-black.png',
                  width: isSmallScreen ? 64 : 84,
                  height: isSmallScreen ? 64 : 84,
                  filterQuality: FilterQuality.high,
                  errorBuilder: (_, _, _) =>
                      ClarionMark(size: isSmallScreen ? 64 : 84),
                ),
              ),
            ),
          ),
        ),

        const SizedBox(width: 16),

        // Right Column: Hardware & OS Specifications
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              // OS Title & Version
              Text(
                'Clarion',
                style: TextStyle(
                  fontSize: isSmallScreen ? 19 : 23,
                  fontWeight: FontWeight.w700,
                  letterSpacing: -0.3,
                  color: dark ? Colors.white : const Color(0xff1d1d1f),
                ),
              ),
              const SizedBox(height: 2),
              Text(
                '版本  ${info.compositorVersion}',
                style: TextStyle(
                  fontSize: 11.5,
                  fontWeight: FontWeight.w400,
                  color: dark ? Colors.white70 : const Color(0xff6e6e73),
                ),
              ),

              Divider(
                height: 16,
                thickness: 0.5,
                color: dark ? Colors.white12 : Colors.black12,
              ),

              // Hardware Model Header
              if (info.modelName.isNotEmpty)
                Text(
                  info.modelName,
                  style: TextStyle(
                    fontSize: 12,
                    fontWeight: FontWeight.w600,
                    color: dark ? Colors.white : const Color(0xff1d1d1f),
                  ),
                ),
              const SizedBox(height: 4),

              // Specs Grid
              _buildSpecRow('处理器', info.processor, dark),
              _buildSpecRow('内存', info.memory, dark),
              _buildSpecRow('系统', info.operatingSystem, dark),
            ],
          ),
        ),
      ],
    );
  }

  Widget _buildSpecRow(String label, String value, bool dark) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 1.5),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 58,
            child: Text(
              label,
              textAlign: TextAlign.right,
              style: TextStyle(
                fontSize: 11,
                fontWeight: FontWeight.w600,
                color: dark ? Colors.white60 : Colors.black54,
              ),
            ),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Text(
              value,
              style: TextStyle(
                fontSize: 11,
                fontWeight: FontWeight.w400,
                color: dark ? Colors.white : Colors.black87,
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildDisplaysTab(
    SystemInformation info,
    bool dark,
    bool isSmallScreen,
  ) {
    if (info.displays.isEmpty) {
      return _buildUnavailableTab(
        icon: Icons.monitor_rounded,
        message: '尚未读取显示器信息',
        button: '显示器偏好设置…',
        dark: dark,
      );
    }
    return Column(
      children: [
        const SizedBox(height: 16),
        for (final display in info.displays) ...[
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 6),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Icon(
                  Icons.monitor_rounded,
                  size: 28,
                  color: const Color(0xff007aff),
                ),
                const SizedBox(width: 10),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        display.name,
                        style: TextStyle(
                          fontSize: 12.5,
                          fontWeight: FontWeight.w600,
                          color: dark ? Colors.white : Colors.black87,
                        ),
                      ),
                      const SizedBox(height: 2),
                      Text(
                        [
                          if (display.resolution.isNotEmpty)
                            display.resolution,
                          if (display.refresh.isNotEmpty) display.refresh,
                          if (display.physical.isNotEmpty) display.physical,
                        ].join(' · '),
                        style: TextStyle(
                          fontSize: 11,
                          color: dark ? Colors.white70 : Colors.black54,
                        ),
                      ),
                      const SizedBox(height: 2),
                      Text(
                        [
                          '缩放 ${display.scale == display.scale.roundToDouble() ? display.scale.round() : display.scale.toStringAsFixed(1)}x',
                          if (display.vrr) '可变刷新率',
                          if (display.hdr) 'HDR',
                        ].join(' · '),
                        style: TextStyle(
                          fontSize: 11,
                          color: dark ? Colors.white60 : Colors.black54,
                        ),
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(height: 8),
        ],
      ],
    );
  }

  Widget _buildStorageTab(
    SystemInformation info,
    bool dark,
    bool isSmallScreen,
  ) {
    final total = info.storageTotalGb;
    if (total <= 0) {
      return _buildUnavailableTab(
        icon: Icons.storage_rounded,
        message: '尚未读取存储信息',
        button: '管理…',
        dark: dark,
      );
    }
    final free = info.storageFreeGb.clamp(0.0, total);
    final used = total - free;
    final fraction = (used / total).clamp(0.0, 1.0);
    return Column(
      children: [
        const SizedBox(height: 20),
        Icon(Icons.storage_rounded, size: 44, color: const Color(0xff007aff)),
        const SizedBox(height: 10),
        Text(
          info.startupDisk,
          style: TextStyle(
            fontSize: 13,
            fontWeight: FontWeight.w600,
            color: dark ? Colors.white : Colors.black87,
          ),
        ),
        const SizedBox(height: 4),
        Text(
          '已使用 ${used.toStringAsFixed(1)} GB · 可用 ${free.toStringAsFixed(1)} GB'
          '（共 ${total.toStringAsFixed(1)} GB）',
          style: TextStyle(
            fontSize: 11.5,
            color: dark ? Colors.white70 : Colors.black54,
          ),
        ),
        const SizedBox(height: 12),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 48),
          child: ClipRRect(
            borderRadius: BorderRadius.circular(3),
            child: SizedBox(
              height: 6,
              child: Stack(
                children: [
                  ColoredBox(
                    color: dark
                        ? Colors.white.withValues(alpha: .12)
                        : Colors.black.withValues(alpha: .10),
                    child: const SizedBox.expand(),
                  ),
                  FractionallySizedBox(
                    widthFactor: fraction,
                    child: const ColoredBox(
                      color: Color(0xff007aff),
                      child: SizedBox.expand(),
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

  Widget _buildUnavailableTab({
    required IconData icon,
    required String message,
    required String button,
    required bool dark,
  }) => Column(
    children: [
      const SizedBox(height: 16),
      Icon(icon, size: 52, color: const Color(0xff007aff)),
      const SizedBox(height: 12),
      Text(
        message,
        style: TextStyle(color: dark ? Colors.white70 : Colors.black54),
      ),
      const SizedBox(height: 16),
      _buildMacButton(
        label: button,
        onPressed: () => widget.controller?.openSettings(),
        dark: dark,
      ),
    ],
  );

  Widget _buildMacButton({
    required String label,
    required VoidCallback? onPressed,
    required bool dark,
  }) {
    return OutlinedButton(
      style: OutlinedButton.styleFrom(
        minimumSize: const Size(0, 22),
        padding: const EdgeInsets.symmetric(horizontal: 10),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(5)),
        side: BorderSide(
          color: dark ? Colors.white24 : Colors.black26,
          width: 0.7,
        ),
        backgroundColor: dark
            ? const Color(0xff3a3a3c)
            : const Color(0xfff7f7f9),
        foregroundColor: dark ? Colors.white : Colors.black87,
        textStyle: const TextStyle(fontSize: 11, fontWeight: FontWeight.w500),
      ),
      onPressed: onPressed,
      child: Text(label),
    );
  }

  Widget _buildFooter(bool dark, bool isSmallScreen) {
    return Container(
      height: 38,
      padding: const EdgeInsets.symmetric(horizontal: 16),
      decoration: BoxDecoration(
        color: dark ? const Color(0xff242426) : const Color(0xffeaeaea),
        border: Border(
          top: BorderSide(
            color: dark
                ? Colors.white.withValues(alpha: 0.08)
                : Colors.black.withValues(alpha: 0.08),
            width: 0.5,
          ),
        ),
      ),
      child: Row(
        children: [
          Expanded(
            child: Text(
              'Clarion · Wayland 桌面 · GPL-3.0-or-later',
              style: TextStyle(
                fontSize: 9.5,
                color: dark ? Colors.white38 : Colors.black38,
              ),
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
            ),
          ),
          const SizedBox(width: 8),
          TextButton(
            key: const ValueKey('mac-about-ok-button'),
            autofocus: true,
            style: TextButton.styleFrom(
              minimumSize: const Size(56, 22),
              padding: const EdgeInsets.symmetric(horizontal: 14),
              backgroundColor: const Color(0xff007aff),
              foregroundColor: Colors.white,
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(5),
              ),
              textStyle: const TextStyle(
                fontSize: 11.5,
                fontWeight: FontWeight.w600,
              ),
            ),
            onPressed: () => Navigator.pop(context),
            child: const Text('好'),
          ),
        ],
      ),
    );
  }
}
