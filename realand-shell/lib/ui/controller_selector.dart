// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/widgets.dart';

/// Rebuild a controller-driven region only when its selected value changes.
///
/// Select immutable values with meaningful equality, such as a record of the
/// fields the builder actually displays. Inherited theme and layout updates
/// still reach the region normally.
class ControllerSelector<T> extends StatefulWidget {
  const ControllerSelector({
    super.key,
    required this.controller,
    required this.select,
    required this.builder,
  });

  final Listenable controller;
  final T Function() select;
  final Widget Function(BuildContext context, T value) builder;

  @override
  State<ControllerSelector<T>> createState() => _ControllerSelectorState<T>();
}

class _ControllerSelectorState<T> extends State<ControllerSelector<T>> {
  late T _selected;

  @override
  void initState() {
    super.initState();
    _selected = widget.select();
    widget.controller.addListener(_changed);
  }

  void _changed() {
    if (!mounted) return;
    final selected = widget.select();
    if (selected == _selected) return;
    setState(() => _selected = selected);
  }

  @override
  void didUpdateWidget(covariant ControllerSelector<T> oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.controller, widget.controller)) {
      oldWidget.controller.removeListener(_changed);
      widget.controller.addListener(_changed);
    }
    // Parent updates may replace the selector as well as its controller.
    _selected = widget.select();
  }

  @override
  void dispose() {
    widget.controller.removeListener(_changed);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.builder(context, _selected);
}
