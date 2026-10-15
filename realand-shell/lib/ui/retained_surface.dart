// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';

/// Keeps content and its native surface alive until the last exit frame.
/// Reversing direction reuses both the content and the current motion value.
class RetainedSurface extends StatefulWidget {
  const RetainedSurface({
    super.key,
    required this.visible,
    required this.reducedMotion,
    required this.duration,
    required this.reverseDuration,
    required this.builder,
    required this.transitionBuilder,
    this.onSurfaceVisible,
  });

  final bool visible;
  final bool reducedMotion;
  final Duration duration;
  final Duration reverseDuration;
  final Widget Function(BuildContext, BoxConstraints) builder;
  final Widget Function(BuildContext, double, Widget) transitionBuilder;
  final ValueChanged<bool>? onSurfaceVisible;

  @override
  State<RetainedSurface> createState() => _RetainedSurfaceState();
}

class _RetainedSurfaceState extends State<RetainedSurface>
    with SingleTickerProviderStateMixin {
  late final AnimationController _motion;
  bool _retained = false;
  bool _layoutReady = false;
  bool _layoutQueued = false;

  @override
  void initState() {
    super.initState();
    _motion = AnimationController(
      vsync: this,
      duration: widget.duration,
      reverseDuration: widget.reverseDuration,
    )..addStatusListener(_status);
    if (widget.visible) _open();
  }

  void _open() {
    if (!_retained) {
      _retained = true;
      widget.onSurfaceVisible?.call(true);
    }
    if (widget.reducedMotion) {
      _motion.value = 1;
    } else if (_layoutReady) {
      _motion.forward();
    }
  }

  void _finishClose() {
    if (!_retained || widget.visible || !mounted) return;
    setState(() {
      _retained = false;
      _layoutReady = false;
    });
    widget.onSurfaceVisible?.call(false);
  }

  void _status(AnimationStatus status) {
    if (status == AnimationStatus.dismissed) _finishClose();
  }

  @override
  void didUpdateWidget(covariant RetainedSurface oldWidget) {
    super.didUpdateWidget(oldWidget);
    _motion.duration = widget.duration;
    _motion.reverseDuration = widget.reverseDuration;
    if (widget.visible) {
      if (!oldWidget.visible || widget.reducedMotion) _open();
    } else if (_retained) {
      if (widget.reducedMotion || _motion.value == 0) {
        _motion.value = 0;
        _finishClose();
      } else if (oldWidget.visible) {
        _motion.reverse();
      }
    }
  }

  @override
  void dispose() {
    _motion.dispose();
    if (_retained) widget.onSurfaceVisible?.call(false);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (!_retained) return const SizedBox.shrink();
    return LayoutBuilder(
      builder: (context, constraints) {
        // Start only after the native surface has usable layout dimensions.
        if (constraints.maxHeight < 1 || constraints.maxWidth < 1) {
          return const SizedBox.shrink();
        }
        if (!_layoutReady && !_layoutQueued) {
          _layoutQueued = true;
          WidgetsBinding.instance.addPostFrameCallback((_) {
            _layoutQueued = false;
            if (!mounted || !_retained || _layoutReady) return;
            _layoutReady = true;
            if (widget.visible) _open();
          });
        }
        return IgnorePointer(
          ignoring: !widget.visible,
          child: ExcludeFocus(
            excluding: !widget.visible,
            child: AnimatedBuilder(
              animation: _motion,
              child: widget.builder(context, constraints),
              builder: (context, child) => widget.transitionBuilder(
                context,
                Curves.easeOutCubic.transform(_motion.value),
                child!,
              ),
            ),
          ),
        );
      },
    );
  }
}
