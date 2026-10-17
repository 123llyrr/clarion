// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../models.dart';
import '../state/shell_controller.dart';
import 'mac_app_icon.dart';

/// macOS Catalina Spotlight — centred floating pill with instant results.
class MacSpotlightPanel extends StatefulWidget {
  const MacSpotlightPanel({
    super.key,
    required this.controller,
    required this.onClose,
  });
  final ShellController controller;
  final VoidCallback onClose;

  @override
  State<MacSpotlightPanel> createState() => _MacSpotlightPanelState();
}

class _MacSpotlightPanelState extends State<MacSpotlightPanel> {
  final _query = TextEditingController();
  final _focus = FocusNode(debugLabel: 'spotlight-field');
  int _selectedIndex = -1;
  List<_SpotlightResult> _results = const [];
  Timer? _debounce;

  @override
  void initState() {
    super.initState();
    _focus.onKeyEvent = _handleKey;
    _query.addListener(_onQueryChanged);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _focus.requestFocus();
    });
  }

  @override
  void dispose() {
    _debounce?.cancel();
    _query.removeListener(_onQueryChanged);
    _query.dispose();
    _focus.dispose();
    super.dispose();
  }

  void _onQueryChanged() {
    _debounce?.cancel();
    _debounce = Timer(const Duration(milliseconds: 80), _search);
  }

  void _search() {
    final text = _query.text.trim();
    if (text.isEmpty) {
      setState(() {
        _results = const [];
        _selectedIndex = -1;
      });
      return;
    }

    final results = <_SpotlightResult>[];

    // Calculator: simple arithmetic
    final calcResult = _tryCalc(text);
    if (calcResult != null) {
      results.add(
        _SpotlightResult(
          icon: Icons.calculate_rounded,
          title: calcResult,
          subtitle: '计算器',
          kind: _ResultKind.calculator,
        ),
      );
    }

    // App search
    final apps = widget.controller.apps;
    final lower = text.toLowerCase();
    final scored = <(DesktopApp, int)>[];
    for (final app in apps) {
      final name = app.name.toLowerCase();
      final desc = app.description.toLowerCase();
      final kw = app.keywords.join(' ').toLowerCase();
      if (name == lower) {
        scored.add((app, 100));
      } else if (name.startsWith(lower)) {
        scored.add((app, 80));
      } else if (name.contains(lower)) {
        scored.add((app, 60));
      } else if (desc.contains(lower) || kw.contains(lower)) {
        scored.add((app, 40));
      } else if (app.id.toLowerCase().contains(lower)) {
        scored.add((app, 20));
      }
    }
    scored.sort((a, b) => b.$2.compareTo(a.$2));
    for (final (app, _) in scored.take(8)) {
      results.add(
        _SpotlightResult(
          icon: Icons.apps_rounded,
          title: app.name,
          subtitle: app.description.isNotEmpty ? app.description : app.id,
          kind: _ResultKind.app,
          appId: app.id,
          app: app,
        ),
      );
    }

    // System settings shortcuts
    for (final entry in _systemEntries) {
      if (entry.title.toLowerCase().contains(lower) ||
          entry.subtitle.toLowerCase().contains(lower)) {
        results.add(entry);
      }
    }

    setState(() {
      _results = results;
      _selectedIndex = results.isEmpty ? -1 : 0;
    });
  }

  String? _tryCalc(String expr) {
    // Sanitise: only allow digits, operators, parens, dots, spaces
    final sanitised = expr.replaceAll(RegExp(r'[^0-9+\-*/().%^ ×÷]'), '');
    if (sanitised.isEmpty) return null;
    // Normalise Unicode operators
    var clean = expr
        .replaceAll('×', '*')
        .replaceAll('÷', '/')
        .replaceAll('^', '');
    // Very simple: try evaluating as Dart double expression
    try {
      // Only allow basic arithmetic
      if (!RegExp(r'^[\d+\-*/.() ]+$').hasMatch(clean)) return null;
      // Use a simple recursive-descent evaluator
      final result = _eval(clean);
      if (result == null || !result.isFinite) return null;
      // Format: remove trailing .0
      final formatted = result == result.roundToDouble() && result.abs() < 1e15
          ? result.toInt().toString()
          : result
                .toStringAsFixed(6)
                .replaceAll(RegExp(r'0+$'), '')
                .replaceAll(RegExp(r'\.$'), '');
      return '= $formatted';
    } on Object {
      return null;
    }
  }

  double? _eval(String expr) {
    expr = expr.trim();
    if (expr.isEmpty) return null;
    try {
      final tokens = _tokenize(expr);
      if (tokens == null) return null;
      var pos = 0;

      late double? Function() parseExpr;
      late double? Function() parseTerm;
      late double? Function() parseFactor;

      parseExpr = () {
        var left = parseTerm();
        if (left == null) return null;
        while (pos < tokens.length &&
            (tokens[pos] == '+' || tokens[pos] == '-')) {
          final op = tokens[pos++];
          final right = parseTerm();
          if (right == null) return null;
          left = op == '+' ? left! + right : left! - right;
        }
        return left;
      };

      parseTerm = () {
        var left = parseFactor();
        if (left == null) return null;
        while (pos < tokens.length &&
            (tokens[pos] == '*' || tokens[pos] == '/')) {
          final op = tokens[pos++];
          final right = parseFactor();
          if (right == null) return null;
          left = op == '*' ? left! * right : left! / right;
        }
        return left;
      };

      parseFactor = () {
        if (pos >= tokens.length) return null;
        // Unary minus
        if (tokens[pos] == '-') {
          pos++;
          final val = parseFactor();
          return val == null ? null : -val;
        }
        if (tokens[pos] == '(') {
          pos++;
          final val = parseExpr();
          if (pos < tokens.length && tokens[pos] == ')') pos++;
          return val;
        }
        final num = double.tryParse(tokens[pos]);
        if (num != null) {
          pos++;
          return num;
        }
        return null;
      };

      final result = parseExpr();
      return (pos == tokens.length) ? result : null;
    } on Object {
      return null;
    }
  }

  List<String>? _tokenize(String expr) {
    final tokens = <String>[];
    var i = 0;
    while (i < expr.length) {
      final c = expr[i];
      if (c == ' ') {
        i++;
        continue;
      }
      if ('+-*/()'.contains(c)) {
        tokens.add(c);
        i++;
      } else if (c == '.' || (c.codeUnitAt(0) >= 48 && c.codeUnitAt(0) <= 57)) {
        final start = i;
        while (i < expr.length &&
            (expr[i] == '.' ||
                (expr[i].codeUnitAt(0) >= 48 && expr[i].codeUnitAt(0) <= 57))) {
          i++;
        }
        tokens.add(expr.substring(start, i));
      } else {
        return null;
      }
    }
    return tokens.isEmpty ? null : tokens;
  }

  void _activate(_SpotlightResult result) {
    switch (result.kind) {
      case _ResultKind.app:
        if (result.appId != null) {
          widget.controller.focusOrLaunchApp(result.appId!);
        }
        widget.onClose();
      case _ResultKind.calculator:
        // Copy to clipboard
        final value = result.title.replaceFirst('= ', '');
        Clipboard.setData(ClipboardData(text: value));
      case _ResultKind.systemSetting:
        widget.controller.openSettings();
        widget.onClose();
    }
  }

  KeyEventResult _handleKey(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent && event is! KeyRepeatEvent) {
      return KeyEventResult.ignored;
    }
    if (event.logicalKey == LogicalKeyboardKey.arrowDown) {
      if (_results.isNotEmpty) {
        setState(() {
          _selectedIndex = (_selectedIndex + 1).clamp(0, _results.length - 1);
        });
        return KeyEventResult.handled;
      }
    } else if (event.logicalKey == LogicalKeyboardKey.arrowUp) {
      if (_results.isNotEmpty) {
        setState(() {
          _selectedIndex = (_selectedIndex - 1).clamp(0, _results.length - 1);
        });
        return KeyEventResult.handled;
      }
    } else if (event.logicalKey == LogicalKeyboardKey.enter) {
      if (_selectedIndex >= 0 && _selectedIndex < _results.length) {
        _activate(_results[_selectedIndex]);
        return KeyEventResult.handled;
      }
    } else if (event.logicalKey == LogicalKeyboardKey.escape) {
      widget.onClose();
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  static final _systemEntries = [
    _SpotlightResult(
      icon: Icons.settings_rounded,
      title: '系统设置',
      subtitle: 'System Preferences',
      kind: _ResultKind.systemSetting,
    ),
    _SpotlightResult(
      icon: Icons.wifi_rounded,
      title: 'Wi-Fi',
      subtitle: '网络设置',
      kind: _ResultKind.systemSetting,
    ),
    _SpotlightResult(
      icon: Icons.bluetooth_rounded,
      title: '蓝牙',
      subtitle: 'Bluetooth',
      kind: _ResultKind.systemSetting,
    ),
    _SpotlightResult(
      icon: Icons.display_settings_rounded,
      title: '显示器',
      subtitle: 'Display Settings',
      kind: _ResultKind.systemSetting,
    ),
    _SpotlightResult(
      icon: Icons.volume_up_rounded,
      title: '声音',
      subtitle: 'Sound Settings',
      kind: _ResultKind.systemSetting,
    ),
    _SpotlightResult(
      icon: Icons.keyboard_rounded,
      title: '键盘',
      subtitle: 'Keyboard Settings',
      kind: _ResultKind.systemSetting,
    ),
  ];

  @override
  Widget build(BuildContext context) {
    final dark = Theme.of(context).brightness == Brightness.dark;
    final hasResults = _results.isNotEmpty;
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: widget.onClose,
      child: Center(
        child: Padding(
          padding: const EdgeInsets.only(bottom: 160),
          child: GestureDetector(
            onTap: () {}, // absorb taps on the pill
            child: Material(
              color: Colors.transparent,
              child: Container(
                width: 680,
                constraints: const BoxConstraints(maxHeight: 520),
                decoration: BoxDecoration(
                  color: dark
                      ? const Color(0xff2a2a2e).withValues(alpha: .96)
                      : const Color(0xfff5f5f7).withValues(alpha: .96),
                  borderRadius: BorderRadius.circular(12),
                  boxShadow: [
                    BoxShadow(
                      color: Colors.black.withValues(alpha: .35),
                      blurRadius: 40,
                      offset: const Offset(0, 12),
                    ),
                  ],
                  border: Border.all(
                    color: dark
                        ? Colors.white.withValues(alpha: .12)
                        : Colors.black.withValues(alpha: .08),
                    width: 0.5,
                  ),
                ),
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    // Search bar
                    SizedBox(
                      height: 52,
                      child: Row(
                        children: [
                          const SizedBox(width: 16),
                          Icon(
                            Icons.search_rounded,
                            size: 22,
                            color: dark ? Colors.white54 : Colors.black45,
                          ),
                          const SizedBox(width: 10),
                          Expanded(
                            child: TextField(
                              controller: _query,
                              focusNode: _focus,
                              style: TextStyle(
                                fontSize: 20,
                                fontWeight: FontWeight.w300,
                                color: dark ? Colors.white : Colors.black87,
                              ),
                              decoration: InputDecoration(
                                hintText: 'Spotlight 搜索',
                                hintStyle: TextStyle(
                                  fontSize: 20,
                                  fontWeight: FontWeight.w300,
                                  color: dark ? Colors.white38 : Colors.black26,
                                ),
                                border: InputBorder.none,
                                contentPadding: EdgeInsets.zero,
                                isDense: true,
                              ),
                              textInputAction: TextInputAction.search,
                              onSubmitted: (_) {
                                if (_selectedIndex >= 0 &&
                                    _selectedIndex < _results.length) {
                                  _activate(_results[_selectedIndex]);
                                }
                              },
                            ),
                          ),
                          if (_query.text.isNotEmpty)
                            IconButton(
                              icon: Icon(
                                Icons.close_rounded,
                                size: 18,
                                color: dark ? Colors.white54 : Colors.black45,
                              ),
                              onPressed: () {
                                _query.clear();
                                _focus.requestFocus();
                              },
                            ),
                          const SizedBox(width: 8),
                        ],
                      ),
                    ),
                    // Results
                    if (hasResults) ...[
                      Container(
                        height: 0.5,
                        color: dark
                            ? Colors.white.withValues(alpha: .10)
                            : Colors.black.withValues(alpha: .06),
                      ),
                      Flexible(
                        child: ListView.builder(
                          padding: const EdgeInsets.symmetric(vertical: 6),
                          shrinkWrap: true,
                          itemCount: _results.length,
                          itemBuilder: (context, index) {
                            final result = _results[index];
                            final selected = index == _selectedIndex;
                            return _SpotlightResultTile(
                              result: result,
                              selected: selected,
                              dark: dark,
                              onTap: () => _activate(result),
                              onHover: () {
                                if (_selectedIndex != index) {
                                  setState(() => _selectedIndex = index);
                                }
                              },
                            );
                          },
                        ),
                      ),
                    ],
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _SpotlightResultTile extends StatelessWidget {
  const _SpotlightResultTile({
    required this.result,
    required this.selected,
    required this.dark,
    required this.onTap,
    required this.onHover,
  });
  final _SpotlightResult result;
  final bool selected;
  final bool dark;
  final VoidCallback onTap;
  final VoidCallback onHover;

  @override
  Widget build(BuildContext context) {
    return MouseRegion(
      onEnter: (_) => onHover(),
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: onTap,
        child: Container(
          height: 40,
          margin: const EdgeInsets.symmetric(horizontal: 8, vertical: 1),
          padding: const EdgeInsets.symmetric(horizontal: 10),
          decoration: BoxDecoration(
            color: selected
                ? const Color(0xff007aff).withValues(alpha: .85)
                : Colors.transparent,
            borderRadius: BorderRadius.circular(6),
          ),
          child: Row(
            children: [
              if (result.app != null)
                SizedBox(
                  width: 26,
                  height: 26,
                  child: MacAppIcon(app: result.app!, size: 26),
                )
              else
                Icon(
                  result.icon,
                  size: 20,
                  color: selected
                      ? Colors.white
                      : dark
                      ? Colors.white70
                      : Colors.black54,
                ),
              const SizedBox(width: 10),
              Expanded(
                child: Text(
                  result.title,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontSize: 13,
                    fontWeight: FontWeight.w500,
                    color: selected
                        ? Colors.white
                        : dark
                        ? Colors.white
                        : Colors.black87,
                  ),
                ),
              ),
              const SizedBox(width: 8),
              Flexible(
                child: Text(
                  result.subtitle,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  textAlign: TextAlign.end,
                  style: TextStyle(
                    fontSize: 12,
                    color: selected
                        ? Colors.white70
                        : dark
                        ? Colors.white38
                        : Colors.black38,
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

enum _ResultKind { app, calculator, systemSetting }

class _SpotlightResult {
  const _SpotlightResult({
    required this.icon,
    required this.title,
    required this.subtitle,
    required this.kind,
    this.appId,
    this.app,
  });
  final IconData icon;
  final String title;
  final String subtitle;
  final _ResultKind kind;
  final String? appId;
  final DesktopApp? app;
}
