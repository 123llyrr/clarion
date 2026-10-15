// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:math' as math;

import 'package:flutter/scheduler.dart';
import 'package:flutter/widgets.dart';

/// A frame-rate-independent, critically damped Dock transition.
///
/// Retargeting preserves both position and velocity. The response is Realand's
/// tuning parameter, not a recovered private macOS animation constant.
class DockMotionState {
  const DockMotionState({
    required this.value,
    this.velocity = 0,
    required this.target,
  });

  final double value;
  final double velocity;
  final double target;

  DockMotionState retarget(double target) =>
      DockMotionState(value: value, velocity: velocity, target: target);

  DockMotionState advance(
    double seconds, {
    required double rate,
    double lowerBound = double.negativeInfinity,
    double upperBound = double.infinity,
  }) {
    if (seconds <= 0) return this;
    final displacement = value - target;
    final coefficient = velocity + rate * displacement;
    final decay = math.exp(-rate * seconds);
    final next = target + (displacement + coefficient * seconds) * decay;
    final speed = (velocity - rate * coefficient * seconds) * decay;
    final bounded = next.clamp(lowerBound, upperBound).toDouble();
    return DockMotionState(
      value: bounded,
      velocity: bounded == next ? speed : 0,
      target: target,
    );
  }

  bool get settled => (value - target).abs() < .0001 && velocity.abs() < .01;
}

/// Keeps a static interactive/icon child outside the per-frame builder.
class DockMotionBuilder extends StatefulWidget {
  const DockMotionBuilder({
    super.key,
    required this.target,
    required this.builder,
    this.response = const Duration(milliseconds: 120),
    this.instant = false,
    this.lowerBound = double.negativeInfinity,
    this.upperBound = double.infinity,
    this.child,
    this.onEnd,
  });

  final double target;
  final Duration response;
  final bool instant;
  final double lowerBound;
  final double upperBound;
  final ValueWidgetBuilder<double> builder;
  final Widget? child;
  final VoidCallback? onEnd;

  @override
  State<DockMotionBuilder> createState() => _DockMotionBuilderState();
}

class _DockMotionBuilderState extends State<DockMotionBuilder>
    with SingleTickerProviderStateMixin {
  late final Ticker _ticker;
  late DockMotionState _motion;
  Duration _elapsed = Duration.zero;
  double _settlingScale = 1;

  @override
  void initState() {
    super.initState();
    _motion = DockMotionState(value: widget.target, target: widget.target);
    _ticker = createTicker(_tick);
  }

  void _tick(Duration elapsed) {
    final seconds = (elapsed - _elapsed).inMicroseconds / 1000000;
    _elapsed = elapsed;
    final response = widget.response.inMicroseconds / 1000000;
    final next = _motion.advance(
      seconds,
      rate: 9 / response,
      lowerBound: widget.lowerBound,
      upperBound: widget.upperBound,
    );
    // Pixel lengths and unitless scales share one response. Normalize the
    // stopping tolerance so a glass width does not outlive its icon motion.
    final settled =
        (next.value - next.target).abs() < .0001 * _settlingScale &&
        next.velocity.abs() < .01 * _settlingScale;
    setState(() {
      _motion = settled
          ? DockMotionState(value: next.target, target: next.target)
          : next;
    });
    if (settled) {
      _ticker.stop();
      widget.onEnd?.call();
    }
  }

  @override
  void didUpdateWidget(covariant DockMotionBuilder oldWidget) {
    super.didUpdateWidget(oldWidget);
    final bounded = _motion.value
        .clamp(widget.lowerBound, widget.upperBound)
        .toDouble();
    if (bounded != _motion.value) {
      // A live preference or surface resize may reduce the reserved space.
      // Apply the new boundary before this frame is painted.
      _motion = DockMotionState(value: bounded, target: _motion.target);
    }
    if (widget.instant || widget.response == Duration.zero) {
      _ticker.stop();
      _motion = DockMotionState(value: widget.target, target: widget.target);
      return;
    }
    if (widget.target != _motion.target) {
      _settlingScale = math.max(1, (widget.target - _motion.value).abs());
      _motion = _motion.retarget(widget.target);
      if (!_ticker.isActive) {
        _elapsed = Duration.zero;
        _ticker.start();
      }
    }
  }

  @override
  void dispose() {
    _ticker.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) =>
      widget.builder(context, _motion.value, widget.child);
}
