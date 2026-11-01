# System Monitor

A native AppCore and ViewKit process monitor. The default window remains
1120 × 669 logical pixels, with CPU, Memory, Energy, Disk and Network categories.

Toolbar controls use SegmentedControl and TextField with retained interaction
state. Native stacks wrap search below the categories when necessary. The
overview uses standard typography and spacing; the process pane uses Surface,
ListRow and Scroll for selection, hover, focus, continuous corners and scrolling.
There is no shared table component yet: the app only arranges its own columns
and virtualizes rows, creating controls for the viewport plus two spare rows.
Offscreen presses/focus are cancelled, and expired process interaction state is
removed after a successful refresh.

Wide windows show all six columns. Compact windows prioritize the process name,
the category's primary metric and PID. Selecting a process shows its state,
CPU, mapped memory and thread count in the standard bottom bar. Row accessibility
labels retain the complete values at either width.

Process sampling remains independent of presentation in `processes.rs` and the
monitor model. Refreshes occur once per second. Search, category ordering and
selection survive root rebuilds; search/category changes reset scroll position.
Refresh errors retain the last successful snapshot and show the failure.

Energy impact is an estimate based on CPU activity. Per-process disk and network
accounting remains unavailable and is displayed as such, without simulated
values. This migration adds neither new collection mechanisms nor graphs.
