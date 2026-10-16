// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';

/// Clears an entire native view before painting its retained child layers.
///
/// Keep this painter in the root display list. Caching a transparent clear in
/// its own repaint boundary turns it into an alpha image, which cannot erase
/// pixels left in the native compositor's framebuffer. This belongs only on a
/// surface that owns its background, never around an inline preview overlay.
class ShellSurfaceClear extends StatelessWidget {
  const ShellSurfaceClear({super.key, required this.child, this.repaint});

  final Widget child;
  final Listenable? repaint;

  @override
  Widget build(BuildContext context) => Stack(
    fit: StackFit.expand,
    alignment: Alignment.topLeft,
    children: [
      CustomPaint(painter: ShellSurfaceClearPainter(repaint: repaint)),
      child,
    ],
  );
}

/// An explicit clear keeps transparent native frames in the display list.
class ShellSurfaceClearPainter extends CustomPainter {
  const ShellSurfaceClearPainter({super.repaint});

  @override
  void paint(Canvas canvas, Size size) => canvas.drawRect(
    Offset.zero & size,
    Paint()
      ..color = Colors.white
      ..isAntiAlias = false
      ..blendMode = BlendMode.clear,
  );

  @override
  bool shouldRepaint(covariant ShellSurfaceClearPainter oldDelegate) => false;
}
