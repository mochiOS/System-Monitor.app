mod processes;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use appcore::prelude::*;
use appcore::viewkit::draw_command::ImageSampling;
use appcore::viewkit::event::{EventContext, EventResult, ViewEvent};
use appcore::viewkit::platform::PointerButton;
use appcore::viewkit::view::{Constraints, MeasureContext, PaintContext};
use processes::{ProcessInfo, ProcessState};

const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const KERNEL_TICKS_PER_SECOND: f32 = 500.0;
const ROW_HEIGHT: f32 = 43.0;
const HEADER_HEIGHT: f32 = 38.0;
const FALLBACK_APPLICATION_ICON: &str = "/system/applications/Binder.app/appicon.svg";

struct SystemMonitorApp {
    category: State<usize>,
    search: State<String>,
}

impl App for SystemMonitorApp {
    type Body = Box<dyn View + 'static>;

    fn new() -> Self {
        Self {
            category: State::new(0),
            search: State::new(String::new()),
        }
    }

    fn window(&self) -> WindowOptions {
        WindowOptions::new("System Monitor")
            .size(1120.0, 669.0)
            .resizable(true)
    }

    fn body(&self, _context: &ViewContext) -> Self::Body {
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
                    MonitorView::new(self.category.clone(), self.search.clone())
                        .layout()
                        .flex_grow(1.0),
                ),
        );

        content
    }
}

#[derive(Clone)]
struct ApplicationPresentation {
    name: String,
    icon: Option<PathBuf>,
}

struct MonitorView {
    category: State<usize>,
    search: State<String>,
    applications: RefCell<HashMap<String, ApplicationPresentation>>,
    icon_cache: RefCell<HashMap<PathBuf, ImageData>>,
    processes: RefCell<Vec<ProcessInfo>>,
    error: RefCell<Option<String>>,
    last_refresh: Cell<Option<Instant>>,
    selected_pid: Cell<Option<u64>>,
    scroll_rows: Cell<usize>,
}

impl MonitorView {
    fn new(category: State<usize>, search: State<String>) -> Self {
        Self {
            category,
            search,
            applications: RefCell::new(load_applications()),
            icon_cache: RefCell::new(HashMap::new()),
            processes: RefCell::new(Vec::new()),
            error: RefCell::new(None),
            last_refresh: Cell::new(None),
            selected_pid: Cell::new(None),
            scroll_rows: Cell::new(0),
        }
    }

    fn refresh(&self, bounds: Rect, context: &mut PaintContext<'_>) {
        let now = Instant::now();
        let due = self
            .last_refresh
            .get()
            .and_then(|last| last.checked_add(REFRESH_INTERVAL));
        if due.is_none_or(|due| due <= now) {
            if self.applications.borrow().is_empty() {
                *self.applications.borrow_mut() = load_applications();
            }
            match processes::snapshot() {
                Ok(mut snapshot) => {
                    let elapsed = self
                        .last_refresh
                        .get()
                        .map(|last| now.saturating_duration_since(last).as_secs_f32())
                        .unwrap_or(0.0);
                    if elapsed > 0.0 {
                        let previous = self.processes.borrow();
                        for process in &mut snapshot {
                            if let Some(old) = previous.iter().find(|old| old.pid == process.pid) {
                                let ticks = process.cpu_ticks.saturating_sub(old.cpu_ticks);
                                process.cpu_percent =
                                    ticks as f32 * 100.0 / (KERNEL_TICKS_PER_SECOND * elapsed);
                            }
                        }
                    }
                    *self.processes.borrow_mut() = snapshot;
                    *self.error.borrow_mut() = None;
                }
                Err(error) => *self.error.borrow_mut() = Some(error.to_string()),
            }
            self.last_refresh.set(Some(now));
        }
        context.request_redraw_in_at(
            Rect::new(bounds.origin.x, bounds.origin.y, 1.0, 1.0),
            now + REFRESH_INTERVAL,
        );
    }

    fn filtered(&self) -> Vec<ProcessInfo> {
        let query = self.search.get().trim().to_lowercase();
        let mut processes = self
            .processes
            .borrow()
            .iter()
            .filter(|item| {
                let application_name = self
                    .applications
                    .borrow()
                    .get(&item.name)
                    .map(|application| application.name.to_lowercase())
                    .unwrap_or_else(|| fallback_process_name(&item.name).to_lowercase());
                query.is_empty()
                    || item.name.to_lowercase().contains(&query)
                    || application_name.contains(&query)
                    || item.pid.to_string().contains(&query)
            })
            .cloned()
            .collect::<Vec<_>>();
        processes.sort_by(|left, right| {
            let category_order = match self.category.get() {
                0 | 2 => right.cpu_percent.total_cmp(&left.cpu_percent),
                1 => right.memory_bytes.cmp(&left.memory_bytes),
                _ => std::cmp::Ordering::Equal,
            };
            let left_name = self
                .applications
                .borrow()
                .get(&left.name)
                .map(|application| application.name.clone())
                .unwrap_or_else(|| fallback_process_name(&left.name));
            let right_name = self
                .applications
                .borrow()
                .get(&right.name)
                .map(|application| application.name.clone())
                .unwrap_or_else(|| fallback_process_name(&right.name));
            category_order
                .then_with(|| left_name.to_lowercase().cmp(&right_name.to_lowercase()))
                .then(left.pid.cmp(&right.pid))
        });
        processes
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

    fn application_icon(&self, path: &Path) -> Option<ImageData> {
        if let Some(icon) = self.icon_cache.borrow().get(path) {
            return Some(icon.clone());
        }
        let icon = load_icon(path)?;
        self.icon_cache
            .borrow_mut()
            .insert(path.to_path_buf(), icon.clone());
        Some(icon)
    }

    fn paint_table(&self, table: Rect, context: &mut PaintContext<'_>) {
        let theme = Theme::current();
        let header = Rect::new(
            table.origin.x,
            table.origin.y,
            table.size.width,
            HEADER_HEIGHT,
        );
        Rectangle::new()
            .color(RectangleColor::Custom(theme.shell.item_enabled))
            .radius(CornerRadius::Large)
            .paint(header, context);

        let widths = [0.37, 0.12, 0.14, 0.11, 0.10, 0.16];
        let headings = match self.category.get() {
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
            let row = Rect::new(table.origin.x, y + 2.0, table.size.width, ROW_HEIGHT - 4.0);
            if self.selected_pid.get() == Some(process.pid) {
                Rectangle::new()
                    .color(RectangleColor::Custom(theme.shell.selection_soft))
                    .radius(CornerRadius::Medium)
                    .paint(row, context);
            }

            let presentation = self.applications.borrow().get(&process.name).cloned();
            let fallback_name = fallback_process_name(&process.name);
            let display_name = presentation
                .as_ref()
                .map(|application| application.name.as_str())
                .unwrap_or(&fallback_name);
            let icon_bounds = Rect::new(table.origin.x + 20.0, y + 8.0, 27.0, 27.0);
            let icon = presentation
                .as_ref()
                .and_then(|application| application.icon.as_deref())
                .and_then(|path| self.application_icon(path))
                .or_else(|| self.application_icon(Path::new(FALLBACK_APPLICATION_ICON)));
            if let Some(icon) = icon {
                Image::new(icon)
                    .content_mode(ImageContentMode::Fit)
                    .sampling(ImageSampling::Bicubic)
                    .radius(CornerRadius::Small)
                    .paint(icon_bounds, context);
            }
            Text::styled(display_name, TextRole::Label)
                .color(if self.selected_pid.get() == Some(process.pid) {
                    theme.shell.action
                } else {
                    theme.shell.primary_text
                })
                .paint(
                    Rect::new(
                        table.origin.x + 57.0,
                        y + 10.0,
                        table.size.width * widths[0] - 77.0,
                        22.0,
                    ),
                    context,
                );

            let cpu = format!("{:.1}%", process.cpu_percent);
            let memory = format_bytes(process.memory_bytes);
            let (primary, secondary) = match self.category.get() {
                1 => (memory, cpu),
                2 => (energy_impact(process.cpu_percent).to_owned(), cpu),
                3 | 4 => ("—".to_owned(), "—".to_owned()),
                _ => (cpu, memory),
            };
            let values = [
                primary,
                secondary,
                process.thread_count.to_string(),
                process.pid.to_string(),
                process.state.label().to_owned(),
            ];
            let mut x = table.origin.x + 20.0 + table.size.width * widths[0];
            for (offset, value) in values.into_iter().enumerate() {
                let index = offset + 1;
                let width = table.size.width * widths[index];
                let state_inset = if index == 5 { 16.0 } else { 0.0 };
                Text::styled(value, TextRole::Label)
                    .color(theme.shell.primary_text)
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
        let processes = self.processes.borrow();
        let count = processes.len();
        let total_cpu = processes
            .iter()
            .map(|process| process.cpu_percent)
            .sum::<f32>();
        let total_memory = processes.iter().fold(0u64, |total, process| {
            total.saturating_add(process.memory_bytes)
        });
        let total_threads = processes.iter().fold(0u64, |total, process| {
            total.saturating_add(process.thread_count)
        });
        let running = processes
            .iter()
            .filter(|process| process.state == ProcessState::Running)
            .count();
        let largest = processes.iter().max_by_key(|process| process.memory_bytes);
        let gap = 20.0;
        let width = (content.size.width - gap * 3.0) / 4.0;
        let cards = match self.category.get() {
            1 => vec![
                (
                    "Mapped Memory",
                    format_bytes(total_memory),
                    "Across live processes".to_owned(),
                ),
                (
                    "Largest Process",
                    largest
                        .map(|process| format_bytes(process.memory_bytes))
                        .unwrap_or_else(|| "—".to_owned()),
                    largest
                        .map(|process| {
                            self.applications
                                .borrow()
                                .get(&process.name)
                                .map(|application| application.name.clone())
                                .unwrap_or_else(|| fallback_process_name(&process.name))
                        })
                        .unwrap_or_else(|| "No processes".to_owned()),
                ),
                (
                    "Processes",
                    count.to_string(),
                    "Live kernel processes".to_owned(),
                ),
                ("Uptime", Self::uptime(), "Since system start".to_owned()),
            ],
            2 => vec![
                (
                    "Relative Load",
                    format!("{total_cpu:.1}%"),
                    "CPU-derived impact".to_owned(),
                ),
                (
                    "Active",
                    running.to_string(),
                    "Running processes".to_owned(),
                ),
                (
                    "Threads",
                    total_threads.to_string(),
                    "Across all processes".to_owned(),
                ),
                ("Uptime", Self::uptime(), "Since system start".to_owned()),
            ],
            3 => vec![
                (
                    "Data Read",
                    "—".to_owned(),
                    "Accounting not available yet".to_owned(),
                ),
                (
                    "Data Written",
                    "—".to_owned(),
                    "Accounting not available yet".to_owned(),
                ),
                (
                    "Processes",
                    count.to_string(),
                    "Live kernel processes".to_owned(),
                ),
                ("Uptime", Self::uptime(), "Since system start".to_owned()),
            ],
            4 => vec![
                (
                    "Received",
                    "—".to_owned(),
                    "Accounting not available yet".to_owned(),
                ),
                (
                    "Sent",
                    "—".to_owned(),
                    "Accounting not available yet".to_owned(),
                ),
                (
                    "Processes",
                    count.to_string(),
                    "Live kernel processes".to_owned(),
                ),
                ("Uptime", Self::uptime(), "Since system start".to_owned()),
            ],
            _ => vec![
                (
                    "CPU Load",
                    format!("{total_cpu:.1}%"),
                    "Across all processors".to_owned(),
                ),
                (
                    "Running",
                    running.to_string(),
                    "Active processes".to_owned(),
                ),
                (
                    "Threads",
                    total_threads.to_string(),
                    "Across all processes".to_owned(),
                ),
                ("Uptime", Self::uptime(), "Since system start".to_owned()),
            ],
        };
        drop(processes);
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
                &detail,
                context,
            );
        }
        self.paint_table(table, context);
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

fn main() -> Result<(), appcore::ViewKitError> {
    appcore::run::<SystemMonitorApp>()
}

fn load_applications() -> HashMap<String, ApplicationPresentation> {
    let mut applications = HashMap::new();
    for root in [Path::new("/applications")] {
        let Ok(entries) = read_directory_entries(root) else {
            continue;
        };
        for entry in entries {
            let app_root = entry.path();
            if app_root.extension().and_then(|value| value.to_str()) != Some("app") {
                continue;
            }
            let Ok(manifest) = read_to_string(app_root.join("manifest.toml")) else {
                continue;
            };
            let (Some(id), Some(name)) = (
                manifest_string(&manifest, "id"),
                manifest_string(&manifest, "name"),
            ) else {
                continue;
            };
            let bundle_name = entry.file_name().to_string_lossy().into_owned();
            let name = if bundle_name.ends_with(".app") {
                bundle_name.clone()
            } else {
                format!("{name}.app")
            };
            let icon = manifest_string(&manifest, "icon").map(|relative| app_root.join(relative));
            let package_bundle_alias = id
                .rsplit('.')
                .next()
                .map(|component| format!("{component}.app"));
            let presentation = ApplicationPresentation { name, icon };

            // A process can be reported by its signed package ID, by the installed
            // bundle directory, or by the executable's package-derived `.app`
            // name. Keep all three spellings tied to the same installed bundle so
            // the visible name and icon never depend on how the process was
            // launched.
            applications
                .entry(id)
                .or_insert_with(|| presentation.clone());
            applications
                .entry(bundle_name)
                .or_insert_with(|| presentation.clone());
            if let Some(alias) = package_bundle_alias {
                applications.entry(alias).or_insert(presentation);
            }
        }
    }
    applications
}

fn read_directory_entries(root: &Path) -> io::Result<Vec<fs::DirEntry>> {
    let mut last_error = None;
    for _ in 0..8 {
        match fs::read_dir(root) {
            Ok(entries) => return collect_directory_entries(entries),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                last_error = Some(error);
                transient_pause();
            }
            Err(error) => return Err(error),
        }
    }
    match fs::read_dir(root) {
        Ok(entries) => collect_directory_entries(entries),
        Err(error) => Err(last_error.unwrap_or(error)),
    }
}

fn collect_directory_entries(entries: fs::ReadDir) -> io::Result<Vec<fs::DirEntry>> {
    let mut collected = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => collected.push(entry),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                transient_pause();
            }
            Err(error) => return Err(error),
        }
    }
    Ok(collected)
}

fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    let path = path.as_ref();
    let mut last_error = None;
    for _ in 0..8 {
        match fs::read_to_string(path) {
            Ok(content) => return Ok(content),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                last_error = Some(error);
                transient_pause();
            }
            Err(error) => return Err(error),
        }
    }
    fs::read_to_string(path).or_else(|error| Err(last_error.unwrap_or(error)))
}

fn transient_pause() {
    for _ in 0..256 {
        core::hint::spin_loop();
    }
}

fn manifest_string(manifest: &str, key: &str) -> Option<String> {
    manifest.lines().find_map(|line| {
        let (candidate, value) = line.split_once('=')?;
        (candidate.trim() == key).then(|| value.trim().trim_matches('"').to_owned())
    })
}

fn load_icon(path: &Path) -> Option<ImageData> {
    if path.extension().and_then(|value| value.to_str()) == Some("svg") {
        let svg = SvgData::from_path(path).ok()?;
        ImageData::from_svg(&svg, 54, 54).ok()
    } else {
        ImageData::thumbnail_from_path(path, 54, 54).ok()
    }
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.1} GB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KB", bytes / KIB)
    } else {
        format!("{} B", bytes as u64)
    }
}

fn energy_impact(cpu_percent: f32) -> &'static str {
    if cpu_percent >= 50.0 {
        "High"
    } else if cpu_percent >= 10.0 {
        "Medium"
    } else {
        "Low"
    }
}

fn fallback_process_name(name: &str) -> String {
    if name.ends_with(".app") || name.ends_with(".service") || name.ends_with(".driver") {
        return name.to_owned();
    }
    if (name.starts_with("org.") || name.starts_with("com.") || name.starts_with("net."))
        && let Some(component) = name.rsplit('.').next()
    {
        return format!("{component}.app");
    }
    name.to_owned()
}
