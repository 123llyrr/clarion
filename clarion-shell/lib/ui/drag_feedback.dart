// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/widgets.dart';

/// Defers a drag feedback subtree until the overlay actually mounts it.
///
/// Tiles rebuild on hover, paging and search; only a started drag needs the
/// duplicated icon tree, so an idle tile constructs one widget instead of a
/// second complete icon subtree on every rebuild.
class DragFeedbackBuilder extends StatelessWidget {
  const DragFeedbackBuilder({super.key, required this.builder});
  final WidgetBuilder builder;

  @override
  Widget build(BuildContext context) => builder(context);
}
