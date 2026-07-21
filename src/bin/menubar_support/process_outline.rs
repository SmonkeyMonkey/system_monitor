// NSOutlineView data source/delegate for the Memory and CPU tabs' process
// lists: top-level rows are apps (grouped by base process name, e.g. all
// "Google Chrome Helper (...)" processes collapse under "Google Chrome"),
// expandable to show each underlying OS process and its own memory/CPU
// usage. Each app row shows the app's real icon (read from its .app bundle
// via NSWorkspace) next to its name, view-based-table style. One
// `ProcessOutlineDataSource` instance is created per tab, each with its
// own `Metric` (Memory or Cpu) controlling how the value column formats.
//
// NSOutlineView identifies rows by object identity (pointer equality) of
// the "item" passed back from the data source, not by index — so unlike
// NSTableView, this can't just hand back copies of the data on demand.
// Each group/child gets one persistent NSObject-subclass instance
// (GroupItem / ProcessItem) that's recreated only when `set_groups`
// rebuilds the row set (i.e. once per refresh tick), and outlineView
// itself decides whether that's "the same" node across calls based on
// whether it's the same pointer.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSControlTextEditingDelegate, NSImage, NSImageView, NSOutlineView, NSOutlineViewDataSource,
    NSOutlineViewDelegate, NSTableColumn, NSTextField, NSView, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSInteger, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};
use std::cell::RefCell;

// Which metric this outline view renders in its second column — controls
// only how the raw f64 value is formatted, since memory (bytes) and CPU
// (a percentage) need different units.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    Memory,
    Cpu,
}

impl Metric {
    fn format(self, value: f64) -> String {
        match self {
            Metric::Memory => format!("{:.0} MB", value / 1_048_576.0),
            Metric::Cpu => format!("{value:.1}%"),
        }
    }
}

pub struct Child {
    pub name: String,
    pub value: f64,
    pub pid: i32,
}

pub struct Group {
    pub app_name: String,
    pub total_value: f64,
    pub children: Vec<Child>,
    pub app_bundle_path: Option<String>,
}

pub struct GroupItemIvars {
    app_name: RefCell<String>,
    total_value: std::cell::Cell<f64>,
    app_bundle_path: RefCell<Option<String>>,
    children: RefCell<Vec<Retained<ProcessItem>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "SystemMonitorGroupItem"]
    #[ivars = GroupItemIvars]
    pub struct GroupItem;

    unsafe impl NSObjectProtocol for GroupItem {}
);

impl GroupItem {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(GroupItemIvars {
            app_name: RefCell::new(String::new()),
            total_value: std::cell::Cell::new(0.0),
            app_bundle_path: RefCell::new(None),
            children: RefCell::new(Vec::new()),
        });
        unsafe { msg_send![super(this), init] }
    }
}

pub struct ProcessItemIvars {
    name: RefCell<String>,
    value: std::cell::Cell<f64>,
    pid: std::cell::Cell<i32>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "SystemMonitorProcessItem"]
    #[ivars = ProcessItemIvars]
    pub struct ProcessItem;

    unsafe impl NSObjectProtocol for ProcessItem {}
);

impl ProcessItem {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(ProcessItemIvars {
            name: RefCell::new(String::new()),
            value: std::cell::Cell::new(0.0),
            pid: std::cell::Cell::new(0),
        });
        unsafe { msg_send![super(this), init] }
    }
}

pub struct ProcessOutlineDataSourceIvars {
    mtm: MainThreadMarker,
    metric: Metric,
    groups: RefCell<Vec<Retained<GroupItem>>>,
    // Cached per-bundle-path icons so we don't re-hit NSWorkspace (which
    // hits disk/LaunchServices) on every 2s refresh tick.
    icon_cache: RefCell<std::collections::HashMap<String, Retained<NSImage>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SystemMonitorProcessOutlineDataSource"]
    #[ivars = ProcessOutlineDataSourceIvars]
    pub struct ProcessOutlineDataSource;

    unsafe impl NSObjectProtocol for ProcessOutlineDataSource {}

    unsafe impl NSOutlineViewDataSource for ProcessOutlineDataSource {
        #[unsafe(method(outlineView:numberOfChildrenOfItem:))]
        fn number_of_children(
            &self,
            _outline_view: &NSOutlineView,
            item: Option<&AnyObject>,
        ) -> NSInteger {
            match item {
                None => self.ivars().groups.borrow().len() as NSInteger,
                Some(item) => {
                    // Single-process "groups" are rendered as plain leaf
                    // rows (no disclosure triangle, see is_expandable), so
                    // they report zero children even though the data has
                    // one — otherwise AppKit still treats the row as
                    // expandable internally.
                    if let Some(group) = as_group(item) {
                        let count = group.ivars().children.borrow().len();
                        if count > 1 { count as NSInteger } else { 0 }
                    } else {
                        0
                    }
                }
            }
        }

        #[unsafe(method(outlineView:isItemExpandable:))]
        fn is_expandable(&self, _outline_view: &NSOutlineView, item: &AnyObject) -> bool {
            as_group(item).is_some_and(|g| g.ivars().children.borrow().len() > 1)
        }

        #[unsafe(method(outlineView:child:ofItem:))]
        fn child_of_item(
            &self,
            _outline_view: &NSOutlineView,
            index: NSInteger,
            item: Option<&AnyObject>,
        ) -> *const AnyObject {
            // GroupItem/ProcessItem instances are retained for as long as
            // they're in `self.ivars().groups`, which outlives any single
            // call into this data source (replaced wholesale by
            // `set_groups` on the next refresh tick) — so a borrowed
            // pointer here is safe without objc retain/autorelease
            // plumbing, which `Retained` return values can't express for
            // a non-"new"/"copy"/"init" selector in this objc2 version.
            match item {
                None => {
                    let groups = self.ivars().groups.borrow();
                    let group = &groups[index as usize];
                    Retained::as_ptr(group).cast()
                }
                Some(item) => {
                    let group = as_group(item).expect("child requested of a non-group item");
                    let children = group.ivars().children.borrow();
                    Retained::as_ptr(&children[index as usize]).cast()
                }
            }
        }
    }

    unsafe impl NSControlTextEditingDelegate for ProcessOutlineDataSource {}

    unsafe impl NSOutlineViewDelegate for ProcessOutlineDataSource {
        #[unsafe(method(outlineView:viewForTableColumn:item:))]
        fn view_for_item(
            &self,
            outline_view: &NSOutlineView,
            column: Option<&NSTableColumn>,
            item: &AnyObject,
        ) -> *const NSView {
            let mtm = self.ivars().mtm;
            let metric = self.ivars().metric;
            let is_value_column = column
                .map(|c| c.identifier().to_string() == "value")
                .unwrap_or(false);

            let (text, icon) = if let Some(group) = as_group(item) {
                if is_value_column {
                    (metric.format(group.ivars().total_value.get()), None)
                } else {
                    let child_count = group.ivars().children.borrow().len();
                    let name = if child_count > 1 {
                        format!("{} ({child_count})", group.ivars().app_name.borrow())
                    } else {
                        group.ivars().app_name.borrow().clone()
                    };
                    let icon = group
                        .ivars()
                        .app_bundle_path
                        .borrow()
                        .as_ref()
                        .map(|path| self.icon_for_bundle(path));
                    (name, icon)
                }
            } else if let Some(process) = as_process(item) {
                if is_value_column {
                    (metric.format(process.ivars().value.get()), None)
                } else {
                    (process.ivars().name.borrow().clone(), None)
                }
            } else {
                return std::ptr::null();
            };

            let row_view =
                build_cell_view(mtm, outline_view, &text, icon.as_deref(), !is_value_column);
            // SAFETY: the returned view is retained by being added as a
            // child of `outline_view` (via the NSTableView row machinery
            // that owns whatever this method returns), so it outlives
            // this call despite us only handing back a raw pointer — see
            // the same rationale as `child_of_item` above.
            Retained::as_ptr(&row_view)
        }
    }
);

fn build_cell_view(
    mtm: MainThreadMarker,
    outline_view: &NSOutlineView,
    text: &str,
    icon: Option<&NSImage>,
    show_icon: bool,
) -> Retained<NSView> {
    let row_height = outline_view.rowHeight();
    let container = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(200.0, row_height)),
    );

    let text_x = if show_icon && icon.is_some() {
        let icon_size = (row_height - 4.0).max(12.0);
        let image_view = NSImageView::initWithFrame(
            NSImageView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, (row_height - icon_size) / 2.0),
                NSSize::new(icon_size, icon_size),
            ),
        );
        image_view.setImage(icon);
        container.addSubview(&image_view);
        icon_size + 6.0
    } else {
        0.0
    };

    let text_field = NSTextField::alloc(mtm);
    let text_field = NSTextField::initWithFrame(
        text_field,
        NSRect::new(
            NSPoint::new(text_x, 0.0),
            NSSize::new(200.0 - text_x, row_height),
        ),
    );
    text_field.setEditable(false);
    text_field.setBezeled(false);
    text_field.setDrawsBackground(false);
    text_field.setSelectable(false);
    text_field.setStringValue(&NSString::from_str(text));
    container.addSubview(&text_field);

    container
}

impl ProcessOutlineDataSource {
    fn icon_for_bundle(&self, bundle_path: &str) -> Retained<NSImage> {
        let mut cache = self.ivars().icon_cache.borrow_mut();
        if let Some(icon) = cache.get(bundle_path) {
            return icon.clone();
        }
        let icon = NSWorkspace::sharedWorkspace().iconForFile(&NSString::from_str(bundle_path));
        cache.insert(bundle_path.to_string(), icon.clone());
        icon
    }
}

fn as_group(item: &AnyObject) -> Option<&GroupItem> {
    item.downcast_ref::<GroupItem>()
}

fn as_process(item: &AnyObject) -> Option<&ProcessItem> {
    item.downcast_ref::<ProcessItem>()
}

// Only leaf process rows carry a PID (a multi-process app's group row
// represents several PIDs at once, so there's no single "rusage" to show
// for it) — returns `(display name, pid)` for double-click handling.
pub fn describe_item(item: &AnyObject) -> Option<(String, i32)> {
    let process = as_process(item)?;
    Some((
        process.ivars().name.borrow().clone(),
        process.ivars().pid.get(),
    ))
}

impl ProcessOutlineDataSource {
    pub fn new(mtm: MainThreadMarker, metric: Metric) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(ProcessOutlineDataSourceIvars {
            mtm,
            metric,
            groups: RefCell::new(Vec::new()),
            icon_cache: RefCell::new(std::collections::HashMap::new()),
        });
        unsafe { msg_send![super(this), init] }
    }

    // Reuses the previous tick's GroupItem/ProcessItem instances when the
    // app/process name matches, instead of always allocating fresh ones.
    // NSOutlineView tracks which rows are expanded by object identity
    // (pointer equality) of the item it was handed, so recreating every
    // item on every refresh silently collapses whatever the user just
    // expanded — that's the bug this works around.
    pub fn set_groups(&self, groups: Vec<Group>) {
        let mtm = self.ivars().mtm;
        let mut old_by_name: std::collections::HashMap<String, Retained<GroupItem>> = self
            .ivars()
            .groups
            .borrow()
            .iter()
            .map(|g| (g.ivars().app_name.borrow().clone(), g.clone()))
            .collect();

        let new_groups: Vec<Retained<GroupItem>> = groups
            .into_iter()
            .map(|g| {
                let group_item = old_by_name
                    .remove(&g.app_name)
                    .unwrap_or_else(|| GroupItem::new(mtm));
                *group_item.ivars().app_name.borrow_mut() = g.app_name;
                group_item.ivars().total_value.set(g.total_value);
                *group_item.ivars().app_bundle_path.borrow_mut() = g.app_bundle_path;

                let mut old_children_by_name: std::collections::HashMap<
                    String,
                    Retained<ProcessItem>,
                > = group_item
                    .ivars()
                    .children
                    .borrow()
                    .iter()
                    .map(|c| (c.ivars().name.borrow().clone(), c.clone()))
                    .collect();

                let children: Vec<Retained<ProcessItem>> = g
                    .children
                    .into_iter()
                    .map(|c| {
                        let process_item = old_children_by_name
                            .remove(&c.name)
                            .unwrap_or_else(|| ProcessItem::new(mtm));
                        *process_item.ivars().name.borrow_mut() = c.name;
                        process_item.ivars().value.set(c.value);
                        process_item.ivars().pid.set(c.pid);
                        process_item
                    })
                    .collect();

                *group_item.ivars().children.borrow_mut() = children;
                group_item
            })
            .collect();

        *self.ivars().groups.borrow_mut() = new_groups;
    }

    pub fn as_data_source(&self) -> &ProtocolObject<dyn NSOutlineViewDataSource> {
        ProtocolObject::from_ref(self)
    }

    pub fn as_delegate(&self) -> &ProtocolObject<dyn NSOutlineViewDelegate> {
        ProtocolObject::from_ref(self)
    }
}
