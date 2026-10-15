# Animation Timing and Presentation

A compositor deals with one or more monitors on mostly fixed refresh cycles.
For example, a 170 Hz monitor can draw a frame every ~5.88 ms.

Most of the time, the compositor doesn't actually redraw the monitor: when nothing changes on screen (e.g. you're reading a document and aren't moving your cursor), it would be wasteful to wake up the GPU to composite the same image.
During an animation however, screen contents do change every frame.
Each output uses its own presentation clock and adaptive budget from `realand-core`.
The scheduler predicts a presentation target and reserves time for wake-up, scene work,
rendering and submission before that target.

Since the monitor refresh cycle is fixed in most cases (even with VRR, there's a maximum refresh rate), the compositor can predict when the next frame will show up on the monitor, and render ongoing animations for that exact moment in time.
Sampling animations for the target time reduces dependence on event-loop wake-up jitter.
It cannot guarantee that a frame will meet its deadline: GPU work, client readiness,
driver behavior and CPU scheduling can still delay presentation. Hardware feedback
corrects the prediction and adjusts the budget for subsequent frames.

The budget retains rollback history while testing smaller reserves, so a short cheap
part of a periodic workload does not immediately discard a previously working reserve.
Provisional reductions let it recover after sustained load decreases. These are Realand
scheduling policies and are covered by periodic-load and recovery regressions.

With native Vulkan, submitted GPU resources are retired by a completion worker. GPU
command-buffer timing, compositor CPU work and hardware presentation intervals are
separate diagnostic signals; a command-buffer sample alone does not measure the whole
input-to-display path.

There are hence several properties that a compositor wants from its timing system.

1. It should be possible to get the state of the animations at a specific time in the near future, for rendering a frame exactly timed to when the monitor will show it.
    - This time override ability should be usable in tests to advance the time in a fully controlled fashion.
1. Animations in response to user actions should begin at the moment when the action happens.
   For example, pressing a workspace switch key should start the animation at the instant when the user pressed the key (rather than, say, slightly in the future where we predicted the next monitor frame, which we had already rendered by now).
1. During the processing of a single action, querying the current time should return the exact same value.
   Even if the processing finishes a few microseconds after it started, querying the time in the end should return the same thing.
   This generally makes writing code much more sane; otherwise you'd need to for example avoid reading the position of some element twice in a row, since it could have moved by one pixel in-between, screwing with the logic.
   Also, fetching the current system time [can be quite expensive](https://mastodon.online/@YaLTeR/109934977035721850) in terms of overhead.
1. It should be reasonably easy to implement an animation slow-down preference, so all animations can be slowed down or sped up by the same factor.

The solution in realand is a `LazyClock`, a clock that remembers one timestamp.
Initially, the timestamp is empty, so when you ask `LazyClock` for the current time, it will fetch and return the system time, and also remember it.
Subsequently, it will keep returning the same timestamp that it had remembered.

You can also clear the timestamp, then `LazyClock` will fetch the system time anew when it's needed.
In realand, the timestamp is cleared at the end of every event loop iteration, right before going to sleep waiting for new events.
This way, anything that happens next (like a user key press) will fetch and use the most up-to-date timestamp as soon as one is needed, but then the processing code will keep getting the exact same timestamp, since `LazyClock` stores it.

For presentation reads, `Clock::project_to()` temporarily projects the clock to an
output's predicted target. Dropping its guard restores the event-time clock, so building
a future frame does not advance input handling or animation lifecycle state. Tests can
set explicit timestamps with `Clock::set_unadjusted()` instead of using system time.

Finally, there's an `AdjustableClock` wrapper on top that provides the ability to control the slow-down rate by modifying the timestamps returned by the clock.

An important detail is that with rate changes, timestamps from the `AdjustableClock` will drift away and become unrelated to the system time.
However, the presentation target uses the system's monotonic time domain, so projection
works on the underlying `LazyClock` and preserves the rate adjustment.
That is, overriding the timestamp and then querying the `AdjustableClock` will return a *different* timestamp that is correct and consistent with the adjustments made by `AdjustableClock`.
This is reflected in the API by naming the function `Clock::set_unadjusted()` (and there's also `Clock::now_unadjusted()` to get the raw timestamp).

The clock is shared among all animations in realand through passing around and storing a reference-counted pointer.
This way, overriding the time automatically applies to everything, whereas in tests we can use a separate clock per test so that they don't interfere with each other.
