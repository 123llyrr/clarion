### Overview

There are several gestures in clarion.

Also see the [gestures configuration](./Configuration:-Gestures.md) wiki page.

### Mouse

#### Interactive Move

You can move windows by holding <kbd>Mod</kbd> and the left mouse button.

You can customize the look of the window insertion preview in the [`insert-hint` layout config](./Configuration:-Layout.md#insert-hint).

Right click while moving to toggle between floating and tiling layout to put the window into.

#### Interactive Resize

You can resize windows by holding <kbd>Mod</kbd> and the right mouse button.

#### Reset Window Height

If you double-click on a top or bottom tiled window resize edge, the window height will reset to automatic.

This works with both window-initiated resizes (when using client-side decorations), and clarion-initiated <kbd>Mod</kbd> + right click resizes.

#### Toggle Full Width

If you double-click on a left or right tiled window resize edge, the column will expand to the full workspace width.

This works with both window-initiated resizes (when using client-side decorations), and clarion-initiated <kbd>Mod</kbd> + right click resizes.

#### Horizontal View Movement

Move the view horizontally by holding <kbd>Mod</kbd> and the middle mouse button (or the wheel) and dragging the mouse horizontally.

#### Workspace Switch

Switch workspaces by holding <kbd>Mod</kbd> and the middle mouse button (or the wheel) and dragging the mouse vertically.

### Touchpad

#### Workspace Switch

Switch workspaces with three-finger vertical swipes.

#### Horizontal View Movement

Move the view horizontally with three-finger horizontal swipes.

#### Open and Close the Overview

Open and close the overview with a four-finger vertical swipe.

### Touchscreen

#### Interactive Move

You can move windows by holding <kbd>Mod</kbd> and dragging the window with your finger.

Dragging a tiled window horizontally will scroll the view instead.

You can customize the look of the window insertion preview in the [`insert-hint` layout config](./Configuration:-Layout.md#insert-hint).

Tap with a second finger while moving to toggle between floating and tiling layout to put the window into.

### Tablet

#### Interactive Move

You can move windows by holding <kbd>Mod</kbd> and dragging the window with a tablet pen.

Dragging a tiled window horizontally will scroll the view instead.

You can customize the look of the window insertion preview in the [`insert-hint` layout config](./Configuration:-Layout.md#insert-hint).

### All Pointing Devices

#### Drag-and-Drop Edge View Scroll

Scroll the tiling view when moving the mouse cursor against a monitor edge during drag-and-drop (DnD).
Also works on a touchscreen.

#### Drag-and-Drop Edge Workspace Switch

Scroll the workspaces up/down when moving the mouse cursor against a monitor edge during drag-and-drop (DnD) while in the overview.
Also works on a touchscreen.

#### Drag-and-Drop Hold to Activate

While drag-and-dropping, hold your mouse over a window to activate it.
This will bring a floating window to the top for example.

In the overview, you can also hold the mouse over a workspace to switch to it.

#### Hot Corner to Toggle the Overview

Put your mouse at the very top-left corner of a monitor to toggle the overview.
Also works during drag-and-dropping something.
