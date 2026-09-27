mod processes;

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use appkit::prelude::*;
use appkit::viewkit::event::{EventContext, EventResult, ViewEvent};
use appkit::viewkit::platform::PointerButton;
use appkit::viewkit::view::{Constraints, MeasureContext, PaintContext};
use processes::{ProcessInfo, ProcessState};

const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const ROW_HEIGHT: f32 = 43.0;
const HEADER_HEIGHT: f32 = 38.0;

struct SystemMonitorApp {
    category: State<usize>,
    search: State<String>,
    refresh_generation: State<usize>,
}

impl App for SystemMonitorApp {
    type Body = ApplicationMenuBar<Box<dyn View + 'static>>;

    fn new() -> Self {
        Self {
            category: State::new(0),
            search: State::new(String::new()),
            refresh_generation: State::new(0),
        }
    }

    fn window(&self) -> WindowOptions {
        WindowOptions::new("System Monitor")
            .size(1120.0, 669.0)
            .resizable(true)
    }

    fn body(&self, _context: &ViewContext) -> Self::Body {
        let menu_refresh = self.refresh_generation.clone();
        let view_refresh = self.refresh_generation.clone();
        let content: Box<dyn View + 'static> = Box::new(
            VStack::new()
                .alignment(StackAlignment::Stretch)
                .gap(StackGap::None)
                .child(
                    Padding::symmetric(20.0, 15.0)
                        .content(
                            HStack::new()
                                .alignment(StackAlignment::Center)
                                .child(
                                    SegmentedControl::new(self.category.binding())
                                        .item(0, "CPU")
                                        .item(1, "Memory")
                                        .item(2, "Energy")
                                        .item(3, "Disk")
                                        .item(4, "Network")
                                        .frame(386.0, 32.0),
                                )
                                .child(Spacer::new())
                                .child(
                                    TextField::new(self.search.binding())
                                        .placeholder("Search")
                                        .leading_symbol(SymbolName::Search)
                                        .size(TextFieldSize::Small)
                                        .frame(206.0, 32.0),
                                ),
                        )
                        .height(62.0),
                )
                .child(Divider::new())
                .child(
                    MonitorView::new(
                        self.category.clone(),
                        self.search.clone(),
                        self.refresh_generation.clone(),
                    )
                    .layout()
                    .flex_grow(1.0),
                ),
        );

        ApplicationMenuBar::new(content)
            .menu(
                ApplicationMenu::new("System Monitor")
                    .item(
                        ApplicationMenuItem::new("Refresh Now", move || {
                            menu_refresh.set(menu_refresh.get().wrapping_add(1));
                        })
                        .shortcut(MenuShortcut::command('r', "Ctrl+R")),
                    )
                    .separator()
                    .item(
                        ApplicationMenuItem::new("Quit System Monitor", request_quit)
                            .shortcut(MenuShortcut::command('q', "Ctrl+Q")),
                    ),
            )
            .menu(
                ApplicationMenu::new("View").item(
                    ApplicationMenuItem::new("Refresh", move || {
                        view_refresh.set(view_refresh.get().wrapping_add(1));
                    })
                    .shortcut(MenuShortcut::command('r', "Ctrl+R")),
                ),
            )
            .menu(
                ApplicationMenu::new("Window").item(
                    ApplicationMenuItem::new("Close Window", request_close_key_window)
                        .shortcut(MenuShortcut::command('w', "Ctrl+W")),
                ),
            )
    }
}

struct MonitorView {
    category: State<usize>,
    search: State<String>,
    refresh_generation: State<usize>,
    observed_generation: Cell<usize>,
    processes: RefCell<Vec<ProcessInfo>>,
    error: RefCell<Option<String>>,
    last_refresh: Cell<Option<Instant>>,
    selected_pid: Cell<Option<u64>>,
    scroll_rows: Cell<usize>,
}

impl MonitorView {
    fn new(category: State<usize>, search: State<String>, refresh: State<usize>) -> Self {
        Self {
            category,
            search,
            observed_generation: Cell::new(refresh.get()),
            refresh_generation: refresh,
            processes: RefCell::new(Vec::new()),
            error: RefCell::new(None),
            last_refresh: Cell::new(None),
            selected_pid: Cell::new(None),
            scroll_rows: Cell::new(0),
        }
    }

    fn refresh(&self, bounds: Rect, context: &mut PaintContext<'_>) {
        let now = Instant::now();
        let generation = self.refresh_generation.get();
        let due = self
            .last_refresh
            .get()
            .and_then(|last| last.checked_add(REFRESH_INTERVAL));
        if generation != self.observed_generation.get() || due.is_none_or(|due| due <= now) {
            match processes::snapshot() {
                Ok(snapshot) => {
                    *self.processes.borrow_mut() = snapshot;
                    *self.error.borrow_mut() = None;
                }
                Err(error) => *self.error.borrow_mut() = Some(error.to_string()),
            }
            self.observed_generation.set(generation);
            self.last_refresh.set(Some(now));
        }
        context.request_redraw_in_at(
            Rect::new(bounds.origin.x, bounds.origin.y, 1.0, 1.0),
            now + REFRESH_INTERVAL,
        );
    }

    fn filtered(&self) -> Vec<ProcessInfo> {
        let query = self.search.get().trim().to_lowercase();
        self.processes
            .borrow()
            .iter()
            .filter(|item| {
                query.is_empty()
                    || item.name.to_lowercase().contains(&query)
                    || item.pid.to_string().contains(&query)
            })
            .cloned()
            .collect()
    }

    fn geometry(bounds: Rect) -> (Rect, Rect) {
        let content = Rect::new(
            bounds.origin.x + 20.0,
            bounds.origin.y + 18.0,
            (bounds.size.width - 40.0).max(0.0),
            (bounds.size.height - 18.0).max(0.0),
        );
        let table = Rect::new(
            content.origin.x,
            content.origin.y + 118.0,
            content.size.width,
            (content.size.height - 154.0).max(0.0),
        );
        (content, table)
    }

    fn uptime() -> String {
        let Some(seconds) = processes::uptime_seconds() else {
            return "—".to_owned();
        };
        let days = seconds / 86_400;
        let hours = (seconds % 86_400) / 3_600;
        let minutes = (seconds % 3_600) / 60;
        if days > 0 {
            format!("{days}d {hours}h")
        } else {
            format!("{hours}h {minutes:02}m")
        }
    }

    fn card(
        bounds: Rect,
        label: &str,
        value: String,
        detail: &str,
        context: &mut PaintContext<'_>,
    ) {
        let theme = Theme::current();
        Rectangle::new()
            .color(RectangleColor::Custom(theme.shell.content_background))
            .radius(CornerRadius::Large)
            .border(BorderStyle::custom(theme.shell.field_border, 1.0))
            .paint(bounds, context);
        Text::styled(label, TextRole::Caption)
            .color(theme.shell.secondary_text)
            .paint(
                Rect::new(
                    bounds.origin.x + 16.0,
                    bounds.origin.y + 10.0,
                    bounds.size.width - 32.0,
                    22.0,
                ),
                context,
            );
        Text::styled(value, TextRole::TitleLarge)
            .weight(500)
            .color(theme.shell.primary_text)
            .paint(
                Rect::new(
                    bounds.origin.x + 16.0,
                    bounds.origin.y + 35.0,
                    bounds.size.width - 32.0,
                    32.0,
                ),
                context,
            );
        Text::metadata(detail).paint(
            Rect::new(
                bounds.origin.x + 16.0,
                bounds.origin.y + 68.0,
                bounds.size.width - 32.0,
                20.0,
            ),
            context,
        );
    }

    fn paint_table(&self, table: Rect, context: &mut PaintContext<'_>) {
        let theme = Theme::current();
        Rectangle::new()
            .color(RectangleColor::Custom(theme.shell.content_background))
            .radius(CornerRadius::Large)
            .border(BorderStyle::custom(theme.shell.field_border, 1.0))
            .paint(table, context);
        let header = Rect::new(
            table.origin.x + 1.0,
            table.origin.y + 1.0,
            table.size.width - 2.0,
            HEADER_HEIGHT,
        );
        Rectangle::new()
            .color(RectangleColor::Custom(theme.shell.item_enabled))
            .radius(CornerRadius::Large)
            .paint(header, context);

        let widths = [0.37, 0.12, 0.14, 0.11, 0.10, 0.16];
        let headings = ["Process Name", "% CPU", "Memory", "Threads", "PID", "State"];
        let mut x = table.origin.x + 20.0;
        for (index, heading) in headings.into_iter().enumerate() {
            let width = table.size.width * widths[index];
            Text::styled(heading, TextRole::Caption)
                .color(theme.shell.secondary_text)
                .paint(
                    Rect::new(x, header.origin.y + 9.0, width - 20.0, 20.0),
                    context,
                );
            x += width;
        }

        let filtered = self.filtered();
        if filtered.is_empty() {
            let message = self
                .error
                .borrow()
                .clone()
                .unwrap_or_else(|| "No matching processes".to_owned());
            Text::styled(message, TextRole::Body)
                .alignment(TextAlignment::Center)
                .color(theme.shell.secondary_text)
                .paint(
                    Rect::new(
                        table.origin.x + 20.0,
                        table.origin.y + 88.0,
                        table.size.width - 40.0,
                        28.0,
                    ),
                    context,
                );
            return;
        }

        let visible = ((table.size.height - HEADER_HEIGHT) / ROW_HEIGHT)
            .floor()
            .max(1.0) as usize;
        let start = self.scroll_rows.get().min(filtered.len().saturating_sub(1));
        for (row_index, process) in filtered.iter().skip(start).take(visible).enumerate() {
            let y = table.origin.y + HEADER_HEIGHT + row_index as f32 * ROW_HEIGHT;
            let row = Rect::new(table.origin.x + 1.0, y, table.size.width - 2.0, ROW_HEIGHT);
            if self.selected_pid.get() == Some(process.pid) {
                Rectangle::new()
                    .color(RectangleColor::Custom(theme.shell.selection_soft))
                    .paint(row, context);
            }
            if row_index > 0 {
                Rectangle::new()
                    .color(RectangleColor::Custom(theme.shell.field_border))
                    .paint(
                        Rect::new(table.origin.x + 18.0, y, table.size.width - 36.0, 1.0),
                        context,
                    );
            }

            let values = [
                process.name.clone(),
                "—".to_owned(),
                "—".to_owned(),
                "—".to_owned(),
                process.pid.to_string(),
                process.state.label().to_owned(),
            ];
            let mut x = table.origin.x + 20.0;
            for (index, value) in values.into_iter().enumerate() {
                let width = table.size.width * widths[index];
                let state_inset = if index == 5 { 16.0 } else { 0.0 };
                Text::styled(value, TextRole::Label)
                    .color(
                        if index == 0 && self.selected_pid.get() == Some(process.pid) {
                            theme.shell.action
                        } else {
                            theme.shell.primary_text
                        },
                    )
                    .paint(
                        Rect::new(x + state_inset, y + 10.0, width - 20.0, 22.0),
                        context,
                    );
                x += width;
            }
            let state_x = table.origin.x + table.size.width * widths[..5].iter().sum::<f32>();
            Ellipse::new()
                .color(EllipseColor::Custom(match process.state {
                    ProcessState::Running => Color::from_rgb_hex(0x2eae7b),
                    ProcessState::Sleeping => Color::from_rgb_hex(0xaeb4bd),
                    ProcessState::Zombie => theme.shell.alert,
                    ProcessState::Unknown(_) => theme.shell.tertiary_text,
                }))
                .paint(Rect::new(state_x + 2.0, y + 17.0, 7.0, 7.0), context);
        }

        Text::metadata(format!("{} processes", filtered.len())).paint(
            Rect::new(
                table.origin.x + 18.0,
                table.origin.y + table.size.height + 7.0,
                180.0,
                22.0,
            ),
            context,
        );
    }
}

impl View for MonitorView {
    fn measure(&self, constraints: Constraints, _context: &mut MeasureContext<'_>) -> Size {
        constraints.constrain(Size::new(1080.0, 606.0))
    }

    fn paint(&self, bounds: Rect, context: &mut PaintContext<'_>) {
        self.refresh(bounds, context);
        Rectangle::new()
            .color(RectangleColor::Custom(
                Theme::current().shell.window_background,
            ))
            .paint(bounds, context);
        let (content, table) = Self::geometry(bounds);
        let count = self.processes.borrow().len();
        let gap = 20.0;
        let width = (content.size.width - gap * 3.0) / 4.0;
        let cards = [
            ("CPU Load", "—".to_owned(), "Accounting unavailable"),
            ("Memory", "—".to_owned(), "Accounting unavailable"),
            ("Processes", count.to_string(), "Live kernel processes"),
            ("Uptime", Self::uptime(), "Since system start"),
        ];
        for (index, (label, value, detail)) in cards.into_iter().enumerate() {
            Self::card(
                Rect::new(
                    content.origin.x + index as f32 * (width + gap),
                    content.origin.y,
                    width,
                    100.0,
                ),
                label,
                value,
                detail,
                context,
            );
        }
        self.paint_table(table, context);
        let _ = self.category.get();
    }

    fn handle_event(
        &self,
        bounds: Rect,
        event: &ViewEvent,
        context: &mut EventContext<'_>,
    ) -> EventResult {
        let (_, table) = Self::geometry(bounds);
        let filtered = self.filtered();
        match event {
            ViewEvent::PointerPressed {
                position,
                button: PointerButton::Primary,
            } if table.contains(*position) && position.y >= table.origin.y + HEADER_HEIGHT => {
                let row = ((position.y - table.origin.y - HEADER_HEIGHT) / ROW_HEIGHT) as usize;
                if let Some(process) = filtered.get(self.scroll_rows.get() + row) {
                    self.selected_pid.set(Some(process.pid));
                    context.request_redraw_in(table);
                }
                EventResult::Consumed
            }
            ViewEvent::Scroll {
                position, delta_y, ..
            } if table.contains(*position) => {
                let maximum = filtered.len().saturating_sub(1);
                let current = self.scroll_rows.get();
                let next = if *delta_y < 0.0 {
                    current.saturating_add(3).min(maximum)
                } else {
                    current.saturating_sub(3)
                };
                if next != current {
                    self.scroll_rows.set(next);
                    context.request_redraw_in(table);
                }
                EventResult::Consumed
            }
            _ => EventResult::Ignored,
        }
    }
}

fn main() -> Result<(), appkit::ViewKitError> {
    appkit::run::<SystemMonitorApp>()
}
