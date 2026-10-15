# realand-visual-tests

> [!NOTE]
>
> This is a development-only app, you shouldn't package it.

This app contains a number of hard-coded test scenarios for visual inspection.
It uses Realand's real layout and rendering code, but with mock windows instead of Wayland clients.
The idea is to go through the test scenarios and check that everything *looks* right.

## Running

You will need recent GTK and libadwaita.
Then, run `cargo run -p realand-visual-tests` from the repository root.

The Spread scenarios exercise the real compositor transition:

- **Realand - Spread Transition and Reversal** opens windows from three workspaces,
  cancels during entry, reopens during exit, searches for `app4`, and confirms it.
- **Realand - Spread Cancel after Navigation** moves the selection through the grid
  and cancels back to the original window.
- **Realand - Window Spread Search** shows filtering with a larger window set.

Use the app's clock controls to inspect intermediate frames and resize the preview
to check compact and portrait layouts. These are mock-window scenes; the automated
layout and hardware rendering regressions run separately through Cargo tests.
