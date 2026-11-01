//! Native control composition; process sampling and metric semantics stay in the model.
use super::*;

pub(super) struct MonitorToolbar {
    category: Rc<SegmentedControl>,
    search: Rc<TextField>,
}
impl MonitorToolbar {
    pub(super) fn new(category: Binding<usize>, search: Binding<String>) -> Self {
        Self {
            category: Rc::new(
                SegmentedControl::new(category)
                    .item(0, "CPU")
                    .item(1, "Memory")
                    .item(2, "Energy")
                    .item(3, "Disk")
                    .item(4, "Network"),
            ),
            search: Rc::new(
                TextField::new(search)
                    .placeholder("Name or PID")
                    .leading_symbol(SymbolName::Search)
                    .size(TextFieldSize::Small),
            ),
        }
    }
    fn content(&self, width: f32, context: &mut MeasureContext<'_>) -> Box<dyn View> {
        let category_width = self
            .category
            .measure(
                Constraints::loose(Size::new(f32::INFINITY, f32::INFINITY)),
                context,
            )
            .width;
        let search_width = context.theme.layout.navigation_sidebar_width;
        if width < category_width + search_width + context.theme.spacing.large {
            Box::new(
                VStack::new()
                    .alignment(StackAlignment::Stretch)
                    .gap(StackGap::Small)
                    .child(self.category.clone())
                    .child(self.search.clone()),
            )
        } else {
            Box::new(
                HStack::new()
                    .alignment(StackAlignment::Center)
                    .gap(StackGap::Large)
                    .child(self.category.clone().width(category_width))
                    .child(Spacer::new())
                    .child(self.search.clone().width(search_width)),
            )
        }
    }
}
impl View for MonitorToolbar {
    fn measure(&self, c: Constraints, context: &mut MeasureContext<'_>) -> Size {
        self.content(c.maximum.width, context).measure(c, context)
    }
    fn paint(&self, bounds: Rect, context: &mut PaintContext<'_>) {
        self.content(
            bounds.size.width,
            &mut MeasureContext {
                theme: context.theme,
                typography: context.typography,
                text_measurer: context.text_measurer,
            },
        )
        .paint(bounds, context);
    }
    fn handle_event(
        &self,
        bounds: Rect,
        event: &ViewEvent,
        context: &mut EventContext<'_>,
    ) -> EventResult {
        self.content(bounds.size.width, &mut context.measure_context())
            .handle_event(bounds, event, context)
    }
}

pub(super) fn content(view: &MonitorView, width: f32, theme: &Theme) -> VStack {
    let filtered = Rc::new(view.filtered());
    let columns = Columns::new(width, view.category.get(), theme);
    let wide = width >= theme.layout.navigation_sidebar_width * 4.0;
    let metric_width = width / if wide { 4.0 } else { 2.0 };
    let metric = |(label, value, detail): (&str, String, String)| {
        Padding::all(theme.spacing.medium).content(
            VStack::new()
                .alignment(StackAlignment::Stretch)
                .gap(StackGap::ExtraSmall)
                .child(
                    Text::caption(label)
                        .single_line(true)
                        .tone(TextTone::Secondary),
                )
                .child(Text::styled(value, TextRole::TitleMedium).single_line(true))
                .child(Text::metadata(detail).single_line(true)),
        )
    };
    let mut metrics = view.metrics().into_iter();
    let pair = |a, b| {
        HStack::new()
            .gap(StackGap::None)
            .alignment(StackAlignment::Stretch)
            .child(metric(a).width(metric_width))
            .child(metric(b).width(metric_width))
    };
    let first = pair(metrics.next().unwrap(), metrics.next().unwrap());
    let second = pair(metrics.next().unwrap(), metrics.next().unwrap());
    let overview: Box<dyn View> = if wide {
        Box::new(
            HStack::new()
                .gap(StackGap::None)
                .alignment(StackAlignment::Stretch)
                .child(first.width(metric_width * 2.0))
                .child(second.width(metric_width * 2.0)),
        )
    } else {
        Box::new(
            VStack::new()
                .gap(StackGap::None)
                .alignment(StackAlignment::Stretch)
                .child(first)
                .child(second),
        )
    };
    let count = Text::caption(format!(
        "{} of {} processes · Updated every second",
        filtered.len(),
        view.processes.borrow().len()
    ))
    .single_line(true)
    .tone(TextTone::Secondary);
    let mut footer = VStack::new()
        .gap(StackGap::ExtraSmall)
        .alignment(StackAlignment::Stretch)
        .child(count);
    let selected = view.selected_pid.get().and_then(|pid| {
        view.processes
            .borrow()
            .iter()
            .find(|p| p.pid == pid)
            .cloned()
    });
    if let Some(process) = selected {
        footer = footer.child(
            Text::metadata(format!(
                "PID {} · {} · {:.1}% CPU · {} memory · {} threads",
                process.pid,
                process.state.label(),
                process.cpu_percent,
                format_bytes(process.memory_bytes),
                process.thread_count
            ))
            .single_line(true),
        );
    }
    VStack::new()
        .alignment(StackAlignment::Stretch)
        .gap(StackGap::None)
        .child(overview.layout().flex_shrink(0.0))
        .child(
            Padding::symmetric(theme.spacing.medium, theme.spacing.small)
                .content(Text::metadata(view.notice()).single_line(true))
                .layout()
                .flex_shrink(0.0),
        )
        .child(
            Surface::sidebar()
                .content(ColumnHeader { columns })
                .layout()
                .flex_shrink(0.0),
        )
        .child(Divider::new())
        .child(
            ProcessScroll {
                view: view.clone(),
                filtered,
            }
            .layout()
            .flex_grow(1.0),
        )
        .child(Toolbar::bottom(footer))
}

// There is no native table component yet. These columns only arrange standard
// text/icon views; ListRow owns appearance, focus, hover and selection behavior.
#[derive(Clone)]
struct Columns {
    entries: Vec<(usize, f32)>,
    headings: [&'static str; 6],
}
impl Columns {
    fn new(width: f32, category: usize, theme: &Theme) -> Self {
        let headings = match category {
            1 => ["Process Name", "Memory", "% CPU", "Threads", "PID", "State"],
            2 => ["Process Name", "Impact", "% CPU", "Threads", "PID", "State"],
            3 => [
                "Process Name",
                "Data Read",
                "Data Written",
                "Threads",
                "PID",
                "State",
            ],
            4 => [
                "Process Name",
                "Received",
                "Sent",
                "Threads",
                "PID",
                "State",
            ],
            _ => ["Process Name", "% CPU", "Memory", "Threads", "PID", "State"],
        };
        let entries = if width < theme.layout.navigation_sidebar_width * 3.0 {
            // Keep identity and the category's primary metric readable first.
            vec![(0, 0.55), (1, 0.25), (4, 0.20)]
        } else {
            vec![
                (0, 0.37),
                (1, 0.12),
                (2, 0.14),
                (3, 0.11),
                (4, 0.10),
                (5, 0.16),
            ]
        };
        Self { entries, headings }
    }
    fn stack(&self, width: f32, theme: &Theme, cells: impl Fn(usize) -> Box<dyn View>) -> HStack {
        // Native Scroll overlays its thumb. Share this gutter with headers so
        // the trailing PID column never paints underneath it in compact mode.
        let width = (width - theme.scrollbar.thickness - theme.scrollbar.inset * 2.0).max(0.0);
        let mut row = HStack::new()
            .alignment(StackAlignment::Center)
            .gap(StackGap::None);
        for &(index, fraction) in &self.entries {
            row = row.child(cells(index).width(width.max(0.0) * fraction));
        }
        row
    }
}
struct ColumnHeader {
    columns: Columns,
}
impl View for ColumnHeader {
    fn measure(&self, c: Constraints, context: &mut MeasureContext<'_>) -> Size {
        c.constrain(Size::new(
            c.maximum.width,
            context.theme.controls.small_height + context.theme.spacing.small,
        ))
    }
    fn paint(&self, bounds: Rect, context: &mut PaintContext<'_>) {
        let inset = context.theme.list.row_padding;
        self.columns
            .stack(bounds.size.width, context.theme, |index| {
                Box::new(
                    Padding::symmetric(inset, 0.0).content(
                        Text::caption(self.columns.headings[index])
                            .single_line(true)
                            .tone(TextTone::Secondary)
                            .alignment(if (1..=4).contains(&index) {
                                TextAlignment::End
                            } else {
                                TextAlignment::Start
                            }),
                    ),
                )
            })
            .paint(bounds, context);
    }
}

struct ProcessScroll {
    view: MonitorView,
    filtered: Rc<Vec<ProcessInfo>>,
}
impl ProcessScroll {
    fn scroll(&self, height: f32) -> Scroll {
        Scroll::new(self.view.scroll.clone()).content(ProcessList {
            view: self.view.clone(),
            filtered: self.filtered.clone(),
            viewport_height: height,
        })
    }
}
impl View for ProcessScroll {
    fn measure(&self, c: Constraints, context: &mut MeasureContext<'_>) -> Size {
        self.scroll(c.maximum.height).measure(c, context)
    }
    fn paint(&self, bounds: Rect, context: &mut PaintContext<'_>) {
        self.scroll(bounds.size.height).paint(bounds, context);
    }
    fn handle_event(
        &self,
        bounds: Rect,
        event: &ViewEvent,
        context: &mut EventContext<'_>,
    ) -> EventResult {
        self.scroll(bounds.size.height)
            .handle_event(bounds, event, context)
    }
}
struct ProcessList {
    view: MonitorView,
    filtered: Rc<Vec<ProcessInfo>>,
    viewport_height: f32,
}
impl ProcessList {
    fn row_height(theme: &Theme) -> f32 {
        theme.controls.small_height + theme.spacing.small * 2.0
    }
    fn visible(&self, theme: &Theme) -> std::ops::Range<usize> {
        let h = Self::row_height(theme);
        let start = (self.view.scroll.offset_y() / h).floor() as usize;
        let end = start
            .saturating_add((self.viewport_height.max(0.0) / h).ceil() as usize + 2)
            .min(self.filtered.len());
        start.min(end)..end
    }
    fn row(&self, index: usize, width: f32, theme: &Theme) -> ListRow {
        let process = &self.filtered[index];
        let presentation = self.view.applications.borrow().get(&process.name).cloned();
        let name = presentation
            .as_ref()
            .map(|app| app.name.clone())
            .unwrap_or_else(|| fallback_process_name(&process.name));
        let icon = presentation
            .as_ref()
            .and_then(|app| app.icon.as_deref())
            .and_then(|path| self.view.application_icon(path))
            .or_else(|| {
                self.view
                    .application_icon(Path::new(FALLBACK_APPLICATION_ICON))
            });
        let cpu = format!("{:.1}%", process.cpu_percent);
        let memory = format_bytes(process.memory_bytes);
        let (primary, secondary) = match self.view.category.get() {
            1 => (memory, cpu),
            2 => (energy_impact(process.cpu_percent).into(), cpu),
            3 | 4 => ("—".into(), "—".into()),
            _ => (cpu, memory),
        };
        let label = format!(
            "{name}, PID {}, {primary}, {secondary}, {} threads, {}",
            process.pid,
            process.thread_count,
            process.state.label()
        );
        let values = [
            name,
            primary,
            secondary,
            process.thread_count.to_string(),
            process.pid.to_string(),
            process.state.label().into(),
        ];
        let columns = Columns::new(width, self.view.category.get(), theme);
        let inset = theme.list.row_padding;
        let icon_size = theme.layout.compact_icon_size;
        let content = columns.stack(width, theme, |index| {
            let text = Text::label(values[index].clone())
                .single_line(true)
                .alignment(if (1..=4).contains(&index) {
                    TextAlignment::End
                } else {
                    TextAlignment::Start
                });
            let cell: Box<dyn View> = if index == 0 {
                let leading: Box<dyn View> = if let Some(icon) = &icon {
                    Box::new(
                        Image::new(icon.clone())
                            .content_mode(ImageContentMode::Fit)
                            .sampling(ImageSampling::Bicubic)
                            .radius(CornerRadius::Small),
                    )
                } else {
                    Box::new(Icon::new(SymbolName::Grid).size(icon_size))
                };
                Box::new(
                    HStack::new()
                        .gap(StackGap::Small)
                        .alignment(StackAlignment::Center)
                        .child(leading.frame(icon_size, icon_size))
                        .child(text.layout().flex_grow(1.0)),
                )
            } else {
                Box::new(text)
            };
            Box::new(Padding::symmetric(inset, 0.0).content(cell))
        });
        let interaction = self
            .view
            .row_interactions
            .borrow_mut()
            .entry(process.pid)
            .or_default()
            .clone();
        let model = self.view.model.clone();
        let pid = process.pid;
        ListRow::with_interaction(interaction, label)
            .content(content)
            .adjacent_rows(index > 0, index + 1 < self.filtered.len())
            .selected(self.view.selected_pid.get() == Some(pid))
            .on_select(move || model.selected_pid.set(Some(pid)))
    }
    fn frame(bounds: Rect, index: usize, height: f32) -> Rect {
        Rect::new(
            bounds.origin.x,
            bounds.origin.y + index as f32 * height,
            bounds.size.width,
            height,
        )
    }
}
impl View for ProcessList {
    fn measure(&self, c: Constraints, context: &mut MeasureContext<'_>) -> Size {
        c.constrain(Size::new(
            c.maximum.width,
            self.filtered.len() as f32 * Self::row_height(context.theme),
        ))
    }
    fn paint(&self, bounds: Rect, context: &mut PaintContext<'_>) {
        use appcore::viewkit::accessibility::{AccessibilityNode, AccessibilityRole};
        context.record_accessibility(AccessibilityNode::new(AccessibilityRole::List, bounds));
        if self.filtered.is_empty() {
            Padding::all(context.theme.spacing.extra_large)
                .content(
                    Text::body(
                        self.view
                            .error
                            .borrow()
                            .clone()
                            .unwrap_or_else(|| "No matching processes".into()),
                    )
                    .tone(TextTone::Secondary),
                )
                .paint(bounds, context);
        }
        let height = Self::row_height(context.theme);
        let visible = self.visible(context.theme);
        for (pid, interaction) in self.view.row_interactions.borrow().iter() {
            if !self.filtered[visible.clone()]
                .iter()
                .any(|process| process.pid == *pid)
            {
                // Virtualization must not keep a press/focus armed offscreen.
                interaction.reset();
            }
        }
        for index in visible {
            self.row(index, bounds.size.width, context.theme)
                .paint(Self::frame(bounds, index, height), context);
        }
    }
    fn handle_event(
        &self,
        bounds: Rect,
        event: &ViewEvent,
        context: &mut EventContext<'_>,
    ) -> EventResult {
        let mut result = EventResult::Ignored;
        let height = Self::row_height(context.theme());
        for index in self.visible(context.theme()) {
            result = result.merge(
                self.row(index, bounds.size.width, context.theme())
                    .handle_event(Self::frame(bounds, index, height), event, context),
            );
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use appcore::viewkit::platform::PointerButton;
    use appcore::viewkit::{
        accessibility::AccessibilityRole,
        draw_command::{DisplayList, DrawCommand},
        typography::TextMeasurer,
    };

    fn view() -> MonitorView {
        MonitorView::new(
            State::new(0),
            State::new(String::new()),
            super::super::ui_tests::model(),
        )
    }

    #[test]
    fn standard_rows_select_on_release_and_inherit_theme_selection_colors() {
        for theme in [Theme::LIGHT, Theme::DARK] {
            let view = view();
            let rows = ProcessList {
                filtered: Rc::new(view.filtered()),
                view: view.clone(),
                viewport_height: 160.0,
            };
            let row = rows.row(0, 1000.0, &theme);
            let frame = Rect::new(0.0, 0.0, 1000.0, ProcessList::row_height(&theme));
            let mut measurer = TextMeasurer::new();
            let mut context = EventContext::new(&theme, &theme.typography, &mut measurer);
            let point = Point::new(100.0, 20.0);
            row.handle_event(
                frame,
                &ViewEvent::PointerPressed {
                    position: point,
                    button: PointerButton::Primary,
                },
                &mut context,
            );
            assert_eq!(view.selected_pid.get(), None);
            row.handle_event(
                frame,
                &ViewEvent::PointerReleased {
                    position: point,
                    button: PointerButton::Primary,
                },
                &mut context,
            );
            assert_eq!(view.selected_pid.get(), Some(100));
            let mut list = DisplayList::new();
            rows.row(0, 1000.0, &theme).paint(
                frame,
                &mut PaintContext::new(&mut list, &theme, &theme.typography, &mut measurer),
            );
            assert!(list.commands().iter().any(|command| matches!(command,
                DrawCommand::DrawText { command } if command.text == "worker-100" && command.color == theme.list.selected_foreground)));
        }
    }

    #[test]
    fn native_scroll_clamps_last_page_and_filtered_results_and_bounds_row_creation() {
        let view = view();
        let theme = Theme::LIGHT;
        let frame = Rect::new(0.0, 0.0, 1000.0, 160.0);
        let mut measurer = TextMeasurer::new();
        let mut list = DisplayList::new();
        let rows = ProcessList {
            view: view.clone(),
            filtered: Rc::new(view.filtered()),
            viewport_height: frame.size.height,
        };
        let first = rows.row(0, frame.size.width, &theme);
        let tracked = first.interaction().clone();
        first.handle_event(
            Rect::new(0.0, 0.0, frame.size.width, ProcessList::row_height(&theme)),
            &ViewEvent::PointerPressed {
                position: Point::new(100.0, 20.0),
                button: PointerButton::Primary,
            },
            &mut EventContext::new(&theme, &theme.typography, &mut measurer),
        );
        assert!(tracked.is_pressed());
        view.scroll.set_offset(0.0, 1_000_000.0);
        let scroll = ProcessScroll {
            view: view.clone(),
            filtered: Rc::new(view.filtered()),
        };
        scroll.paint(
            frame,
            &mut PaintContext::new(&mut list, &theme, &theme.typography, &mut measurer),
        );
        assert_eq!(
            view.scroll.offset_y(),
            100.0 * ProcessList::row_height(&theme) - frame.size.height
        );
        assert!(list.commands().iter().any(|command| matches!(command, DrawCommand::DrawText { command } if command.text == "worker-1")));
        assert!(view.row_interactions.borrow().len() <= 6);
        assert!(
            !tracked.is_pressed(),
            "scrolling must cancel an offscreen press"
        );
        view.search.set("worker-99".into());
        let scroll = ProcessScroll {
            view: view.clone(),
            filtered: Rc::new(view.filtered()),
        };
        list.clear();
        scroll.paint(
            frame,
            &mut PaintContext::new(&mut list, &theme, &theme.typography, &mut measurer),
        );
        assert_eq!(view.scroll.offset_y(), 0.0);
        assert!(list.commands().iter().any(|command| matches!(command, DrawCommand::DrawText { command } if command.text == "worker-99")));
    }

    #[test]
    fn native_toolbar_wraps_without_overlapping_or_losing_search() {
        let category = State::new(0);
        let search = State::new(String::new());
        let toolbar = Toolbar::new(MonitorToolbar::new(category.binding(), search.binding()));
        let theme = Theme::LIGHT;
        let mut measurer = TextMeasurer::new();
        for width in [360.0, 480.0, 720.0, 1120.0] {
            let size = toolbar.measure(
                Constraints::loose(Size::new(width, 300.0)),
                &mut MeasureContext {
                    theme: &theme,
                    typography: &theme.typography,
                    text_measurer: &mut measurer,
                },
            );
            let bounds = Rect::new(0.0, 0.0, width, size.height);
            let mut list = DisplayList::new();
            let mut nodes = Vec::new();
            toolbar.paint(
                bounds,
                &mut PaintContext::new(&mut list, &theme, &theme.typography, &mut measurer)
                    .with_accessibility_nodes(&mut nodes),
            );
            let group = nodes
                .iter()
                .find(|node| node.role == AccessibilityRole::RadioGroup)
                .unwrap();
            let search = nodes
                .iter()
                .find(|node| node.role == AccessibilityRole::TextField)
                .unwrap();
            assert!(group.bounds.intersection(search.bounds).is_none());
            for frame in [group.bounds, search.bounds] {
                assert!(frame.origin.x >= 0.0 && frame.origin.y >= 0.0);
                assert!(frame.origin.x + frame.size.width <= width + 0.01);
                assert!(frame.origin.y + frame.size.height <= size.height + 0.01);
            }
        }
    }

    #[test]
    fn retained_search_accepts_multiple_edits_across_root_rebuilds() {
        let app = SystemMonitorApp::new();
        app.toolbar.search.interaction().set_focused(true);
        let theme = Theme::LIGHT;
        let mut measurer = TextMeasurer::new();
        for text in ["work", "er"] {
            let toolbar = app.toolbar.clone();
            toolbar.handle_event(
                Rect::new(0.0, 0.0, 1000.0, 60.0),
                &ViewEvent::TextInput { text: text.into() },
                &mut EventContext::new(&theme, &theme.typography, &mut measurer),
            );
        }
        assert_eq!(app.search.get(), "worker");
        assert!(app.toolbar.search.interaction().is_focused());
    }

    #[test]
    fn overview_values_are_measured_above_the_header_at_both_window_widths() {
        for theme in [Theme::LIGHT, Theme::DARK] {
            for width in [480.0, 1120.0] {
                let view = view();
                view.selected_pid.set(Some(100));
                let mut list = DisplayList::new();
                let mut measurer = TextMeasurer::new();
                view.paint(
                    Rect::new(0.0, 0.0, width, 606.0),
                    &mut PaintContext::new(&mut list, &theme, &theme.typography, &mut measurer),
                );
                let text_bounds = |text| {
                    list.commands()
                        .iter()
                        .find_map(|cmd| match cmd {
                            DrawCommand::DrawText { command } if command.text == text => {
                                Some(command.bounds)
                            }
                            _ => None,
                        })
                        .expect("expected visible UI text")
                };
                let value = text_bounds("5050.0%");
                let header = text_bounds("Process Name");
                assert!(value.size.width > 0.0 && value.size.height > 0.0);
                assert!(
                    value.origin.y >= 0.0 && value.origin.y + value.size.height <= header.origin.y
                );
                assert!(list.commands().iter().any(|cmd| matches!(cmd,
                    DrawCommand::DrawText { command } if command.text.contains("PID 100 · Sleeping"))));
            }
        }
    }

    #[test]
    fn compact_pid_cells_leave_the_native_scrollbar_gutter_clear() {
        let view = view();
        let theme = Theme::LIGHT;
        let rows = ProcessList {
            view: view.clone(),
            filtered: Rc::new(view.filtered()),
            viewport_height: 160.0,
        };
        let mut list = DisplayList::new();
        let mut measurer = TextMeasurer::new();
        rows.paint(
            Rect::new(0.0, 0.0, 480.0, 4000.0),
            &mut PaintContext::new(&mut list, &theme, &theme.typography, &mut measurer),
        );
        let pid = list
            .commands()
            .iter()
            .find_map(|cmd| match cmd {
                DrawCommand::DrawText { command } if command.text == "100" => Some(command.bounds),
                _ => None,
            })
            .unwrap();
        assert!(
            pid.origin.x + pid.size.width
                <= 480.0 - theme.scrollbar.thickness - theme.scrollbar.inset
        );
    }
}
