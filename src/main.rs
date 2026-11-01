mod processes;
mod ui;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use appcore::prelude::*;
use appcore::viewkit::draw_command::ImageSampling;
use appcore::viewkit::event::{EventContext, EventResult, ViewEvent};
use appcore::viewkit::view::{Constraints, MeasureContext, PaintContext};
use processes::{ProcessInfo, ProcessState};

const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const KERNEL_TICKS_PER_SECOND: f32 = 500.0;
const FALLBACK_APPLICATION_ICON: &str = "/system/applications/Binder.app/appicon.svg";

struct SystemMonitorApp {
    category: State<usize>,
    search: State<String>,
    model: Rc<MonitorModel>,
    toolbar: Rc<ui::MonitorToolbar>,
}

impl App for SystemMonitorApp {
    type Body = Box<dyn View + 'static>;

    fn new() -> Self {
        let category = State::new(0);
        let search = State::new(String::new());
        let toolbar = Rc::new(ui::MonitorToolbar::new(
            category.binding(),
            search.binding(),
        ));
        Self {
            category,
            search,
            model: Rc::new(MonitorModel::new()),
            toolbar,
        }
    }

    fn window(&self) -> WindowOptions {
        WindowOptions::new("System Monitor")
            .size(1120.0, 669.0)
            .resizable(true)
    }

    fn body(&self, _context: &ViewContext) -> Self::Body {
        Box::new(
            VStack::new()
                .alignment(StackAlignment::Stretch)
                .gap(StackGap::None)
                .child(Toolbar::new(self.toolbar.clone()))
                .child(
                    MonitorView::new(
                        self.category.clone(),
                        self.search.clone(),
                        self.model.clone(),
                    )
                    .layout()
                    .flex_grow(1.0),
                ),
        )
    }
}

#[derive(Clone)]
struct ApplicationPresentation {
    name: String,
    icon: Option<PathBuf>,
}

#[derive(Clone)]
struct MonitorView {
    category: State<usize>,
    search: State<String>,
    model: Rc<MonitorModel>,
}

impl Deref for MonitorView {
    type Target = MonitorModel;
    fn deref(&self) -> &MonitorModel {
        &self.model
    }
}

struct MonitorModel {
    applications: RefCell<HashMap<String, ApplicationPresentation>>,
    icon_cache: RefCell<HashMap<PathBuf, ImageData>>,
    processes: RefCell<Vec<ProcessInfo>>,
    error: RefCell<Option<String>>,
    last_refresh: Cell<Option<Instant>>,
    selected_pid: Cell<Option<u64>>,
    scroll: ScrollState,
    row_interactions: RefCell<HashMap<u64, ButtonInteractionState>>,
    filter_key: RefCell<(usize, String)>,
}

impl MonitorModel {
    fn new() -> Self {
        Self {
            applications: RefCell::new(load_applications()),
            icon_cache: RefCell::new(HashMap::new()),
            processes: RefCell::new(Vec::new()),
            error: RefCell::new(None),
            last_refresh: Cell::new(None),
            selected_pid: Cell::new(None),
            scroll: ScrollState::new(),
            row_interactions: RefCell::new(HashMap::new()),
            filter_key: RefCell::new((0, String::new())),
        }
    }
}

impl MonitorView {
    fn new(category: State<usize>, search: State<String>, model: Rc<MonitorModel>) -> Self {
        Self {
            category,
            search,
            model,
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
                    self.row_interactions
                        .borrow_mut()
                        .retain(|pid, _| snapshot.iter().any(|p| p.pid == *pid));
                    *self.processes.borrow_mut() = snapshot;
                    *self.error.borrow_mut() = None;
                }
                Err(error) => *self.error.borrow_mut() = Some(error.to_string()),
            }
            self.last_refresh.set(Some(now));
        }
        context.request_redraw_in_at(bounds, now + REFRESH_INTERVAL);
    }

    fn filtered(&self) -> Vec<ProcessInfo> {
        let query = self.search.get().trim().to_lowercase();
        let key = (self.category.get(), query.clone());
        if *self.filter_key.borrow() != key {
            *self.filter_key.borrow_mut() = key;
            self.scroll.reset();
        }
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

    fn metrics(&self) -> Vec<(&'static str, String, String)> {
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
        cards
    }
    fn notice(&self) -> String {
        self.error.borrow().as_ref().map(|error| format!("Could not refresh: {error}. Showing the last available snapshot."))
            .unwrap_or_else(|| match self.category.get() {
                2 => "Energy impact is estimated from CPU activity; power consumption is not measured.".into(),
                3 => "Per-process disk activity is not available yet.".into(),
                4 => "Per-process network activity is not available yet.".into(),
                _ => "Processes · Highest usage first".into(),
            })
    }
}

impl View for MonitorView {
    fn measure(&self, constraints: Constraints, _: &mut MeasureContext<'_>) -> Size {
        constraints.constrain(Size::new(1080.0, 606.0))
    }
    fn paint(&self, bounds: Rect, context: &mut PaintContext<'_>) {
        self.refresh(bounds, context);
        Surface::pane().paint_content(bounds, context, |context| {
            ui::content(self, bounds.size.width, context.theme).paint(bounds, context);
        });
    }
    fn handle_event(
        &self,
        bounds: Rect,
        event: &ViewEvent,
        context: &mut EventContext<'_>,
    ) -> EventResult {
        Surface::pane()
            .content(ui::content(self, bounds.size.width, context.theme()))
            .handle_event(bounds, event, context)
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

#[cfg(test)]
mod ui_tests {
    use super::*;
    use appcore::viewkit::{
        draw_command::{DisplayList, DrawCommand},
        typography::TextMeasurer,
        view::RedrawSchedule,
    };

    pub(super) fn model() -> Rc<MonitorModel> {
        let model = Rc::new(MonitorModel::new());
        *model.processes.borrow_mut() = (1..=100)
            .map(|pid| ProcessInfo {
                pid,
                name: format!("worker-{pid}"),
                state: ProcessState::Sleeping,
                cpu_ticks: pid,
                cpu_percent: pid as f32,
                memory_bytes: pid * 1024,
                thread_count: 1,
            })
            .collect();
        model.last_refresh.set(Some(Instant::now()));
        model
    }

    #[test]
    fn rebuilding_after_search_preserves_snapshot_and_selection_but_resets_scroll() {
        let model = model();
        let category = State::new(0);
        let search = State::new(String::new());
        let view = MonitorView::new(category.clone(), search.clone(), model.clone());
        model.selected_pid.set(Some(99));
        model.scroll.set_offset(0.0, 500.0);
        assert_eq!(view.filtered().len(), 100);
        search.set("worker-99".into());
        let rebuilt = MonitorView::new(category.clone(), search, model.clone());
        let filtered = rebuilt.filtered();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].pid, 99);
        assert_eq!(model.selected_pid.get(), Some(99));
        assert_eq!(model.scroll.offset_y(), 0.0);
        assert_eq!(model.processes.borrow().len(), 100);
        category.set(1);
        assert_eq!(rebuilt.filtered()[0].pid, 99);
        assert_eq!(model.processes.borrow()[98].cpu_percent, 99.0);
    }

    #[test]
    fn paint_explains_unavailable_metrics_and_retains_refresh_errors_with_data() {
        let model = model();
        let category = State::new(4);
        let view = MonitorView::new(category.clone(), State::new(String::new()), model.clone());
        let theme = Theme::current();
        let mut measurer = TextMeasurer::new();
        let mut list = DisplayList::new();
        let mut schedule = RedrawSchedule::new();
        let bounds = Rect::new(0.0, 0.0, 1000.0, 600.0);
        view.paint(
            bounds,
            &mut PaintContext::new(&mut list, &theme, &theme.typography, &mut measurer)
                .with_redraw_schedule(&mut schedule),
        );
        assert!(list.commands().iter().any(|command| matches!(command,
            DrawCommand::DrawText { command } if command.text.contains("network activity is not available"))));
        assert!(schedule.deadline().is_some());
        *model.error.borrow_mut() = Some("Inspection denied".into());
        category.set(0);
        // Keep the fixture snapshot fixed even if the first font/layout pass
        // took longer than the real one-second sampling interval.
        model.last_refresh.set(Some(Instant::now()));
        list.clear();
        view.paint(
            bounds,
            &mut PaintContext::new(&mut list, &theme, &theme.typography, &mut measurer),
        );
        assert!(list.commands().iter().any(|command| matches!(command,
            DrawCommand::DrawText { command } if command.text.contains("Inspection denied"))));
        assert!(list.commands().iter().any(|command| matches!(command,
            DrawCommand::DrawText { command } if command.text == "worker-100")));
    }
}
