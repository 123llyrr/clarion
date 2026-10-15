// SPDX-License-Identifier: GPL-3.0-or-later
/// Local civil dates: constructing each date avoids DST-duration drift.
List<DateTime> macCalendarMonthDays(DateTime month, int firstDayOfWeek) {
  final first = DateTime(month.year, month.month);
  final leading = (first.weekday % 7 - firstDayOfWeek + 7) % 7;
  final length = DateTime(month.year, month.month + 1, 0).day;
  final count = ((leading + length + 6) ~/ 7) * 7;
  return List.unmodifiable([
    for (var index = 0; index < count; index++)
      DateTime(month.year, month.month, 1 - leading + index),
  ]);
}

DateTime macCalendarShiftMonth(DateTime date, int delta) {
  final target = DateTime(date.year, date.month + delta);
  final last = DateTime(target.year, target.month + 1, 0).day;
  return DateTime(target.year, target.month, date.day > last ? last : date.day);
}
