// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/foundation.dart';

enum AppMenuProtocol { gtk, dbusmenu }

/// Metadata exported by the focused application, rather than inferred XIDs.
@immutable
class AppMenuExport {
  const AppMenuExport({
    required this.busName,
    required this.menuPath,
    this.protocol = AppMenuProtocol.gtk,
    this.applicationActionsPath,
    this.windowActionsPath,
  });
  final String busName;
  final String menuPath;
  final AppMenuProtocol protocol;
  final String? applicationActionsPath;
  final String? windowActionsPath;
}

@immutable
class AppMenuTarget {
  const AppMenuTarget({
    this.pid,
    this.appId = '',
    this.windowId,
    this.x11WindowId,
    this.export,
  });
  final int? pid;
  final String appId;

  /// Compositor identity, used only to cancel work when focus changes.
  final int? windowId;

  /// Only an explicitly known XWindow ID may be sent to an X11 registrar.
  final int? x11WindowId;
  final AppMenuExport? export;
}

@immutable
class GlobalMenuItem {
  GlobalMenuItem({
    required this.id,
    required this.label,
    this.enabled = true,
    this.visible = true,
    this.separator = false,
    this.checked,
    this.radio = false,
    this.shortcut = '',
    this.submenu = false,
    Iterable<GlobalMenuItem> children = const [],
    this.onAction,
  }) : children = List.unmodifiable(children);
  final String id;
  final String label;
  final bool enabled;
  final bool visible;
  final bool separator;
  final bool? checked;
  final bool radio;
  final String shortcut;
  final bool submenu;
  final List<GlobalMenuItem> children;
  final VoidCallback? onAction;
  bool get isSubmenu => submenu || children.isNotEmpty;

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is GlobalMenuItem &&
          id == other.id &&
          label == other.label &&
          enabled == other.enabled &&
          visible == other.visible &&
          separator == other.separator &&
          checked == other.checked &&
          radio == other.radio &&
          shortcut == other.shortcut &&
          submenu == other.submenu &&
          listEquals(children, other.children);

  @override
  int get hashCode => Object.hash(
        id,
        label,
        enabled,
        visible,
        separator,
        checked,
        radio,
        shortcut,
        submenu,
        Object.hashAll(children),
      );
}
