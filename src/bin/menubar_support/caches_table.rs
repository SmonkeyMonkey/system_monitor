// View-based NSTableView data source/delegate for the Storage window's
// "Caches" tab: one row per detected build/dependency cache directory
// (target, node_modules, .venv, ...), each with a checkbox, its owning
// project folder, full path, and size. Selected rows get deleted (moved
// to Trash) via the "Delete Selected" button — never the project folder
// itself, only the cache directory found inside it.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSButton, NSControlStateValueOff, NSControlStateValueOn, NSControlTextEditingDelegate,
    NSTableColumn, NSTableView, NSTableViewDataSource, NSTableViewDelegate, NSTextField, NSView,
};
use objc2_foundation::{
    MainThreadMarker, NSInteger, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};
use std::cell::RefCell;
use std::path::PathBuf;

pub struct Row {
    pub path: PathBuf,
    pub project_path: PathBuf,
    pub size_bytes: u64,
    pub checked: std::cell::Cell<bool>,
    // Cell views handed out for this row, keyed by column identifier.
    // NSTableView expects `tableView:viewForTableColumn:row:` to return a
    // view it can retain and reuse; since this isn't an "init"/"new"/
    // "copy"-family selector, objc2 can't express returning an owned
    // `Retained` from it directly here (the same limitation worked around
    // in process_outline.rs/storage_outline.rs by keeping the item itself
    // alive in a tree). Views have no such backing tree, so they're kept
    // alive here instead — AppKit retains the pointer we hand back, but
    // the crash this replaced happened because *we* were the only owner
    // and dropped it the instant the function returned, leaving AppKit
    // holding a dangling pointer.
    views: RefCell<std::collections::HashMap<String, Retained<NSView>>>,
}

pub struct CachesTableDataSourceIvars {
    mtm: MainThreadMarker,
    rows: RefCell<Vec<Row>>,
    checkbox_target: RefCell<Option<Retained<objc2::runtime::AnyObject>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SystemMonitorCachesTableDataSource"]
    #[ivars = CachesTableDataSourceIvars]
    pub struct CachesTableDataSource;

    unsafe impl NSObjectProtocol for CachesTableDataSource {}

    unsafe impl NSTableViewDataSource for CachesTableDataSource {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows(&self, _table_view: &NSTableView) -> NSInteger {
            self.ivars().rows.borrow().len() as NSInteger
        }
    }

    unsafe impl NSControlTextEditingDelegate for CachesTableDataSource {}

    unsafe impl NSTableViewDelegate for CachesTableDataSource {
        #[unsafe(method(tableView:viewForTableColumn:row:))]
        fn view_for_row(
            &self,
            _table_view: &NSTableView,
            column: Option<&NSTableColumn>,
            row: NSInteger,
        ) -> *const NSView {
            let mtm = self.ivars().mtm;
            let rows = self.ivars().rows.borrow();
            let Some(data) = rows.get(row as usize) else {
                return std::ptr::null();
            };
            let identifier = column
                .map(|c| c.identifier().to_string())
                .unwrap_or_default();

            let checkbox_target = self.ivars().checkbox_target.borrow().clone();
            let mut views = data.views.borrow_mut();
            let view =
                views
                    .entry(identifier.clone())
                    .or_insert_with(|| match identifier.as_str() {
                        "checkbox" => build_checkbox_view(
                            mtm,
                            checkbox_target.as_deref(),
                            row,
                            data.checked.get(),
                        ),
                        "project" => build_text_view(mtm, &project_name(&data.project_path)),
                        "path" => build_text_view(mtm, &data.path.to_string_lossy()),
                        "size" => build_text_view(mtm, &format_size(data.size_bytes)),
                        _ => NSView::initWithFrame(
                            NSView::alloc(mtm),
                            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)),
                        ),
                    });
            // SAFETY: `view` is kept alive in `data.views` for as long as
            // this row exists, so this pointer stays valid after the
            // function returns — see the comment on `Row::views` above.
            Retained::as_ptr(view)
        }
    }
);

fn build_checkbox_view(
    mtm: MainThreadMarker,
    target: Option<&objc2::runtime::AnyObject>,
    row: NSInteger,
    checked: bool,
) -> Retained<NSView> {
    let button = NSButton::initWithFrame(
        NSButton::alloc(mtm),
        NSRect::new(NSPoint::new(2.0, 0.0), NSSize::new(20.0, 18.0)),
    );
    button.setButtonType(objc2_app_kit::NSButtonType::Switch);
    button.setTitle(objc2_foundation::ns_string!(""));
    button.setState(if checked {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    button.setTag(row);
    // The button's target is the app-wide AppTarget (set via
    // set_checkbox_target), not this data source — toggling a row also
    // needs to refresh the "N GB selected" summary label, which lives
    // outside this file (built alongside the table in
    // menubar.rs::build_caches_tab), so the action has to be handled
    // wherever that label is reachable.
    unsafe {
        button.setTarget(target);
        button.setAction(Some(sel!(toggleCacheRow:)));
    }
    // objc2's NSButton is already an NSView subclass; the row's frame is
    // set by the table column width, so returning the button directly
    // (without a wrapping container) is fine here.
    unsafe { Retained::cast_unchecked(button) }
}

fn build_text_view(mtm: MainThreadMarker, text: &str) -> Retained<NSView> {
    let field = NSTextField::alloc(mtm);
    let field = NSTextField::initWithFrame(
        field,
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(200.0, 18.0)),
    );
    field.setEditable(false);
    field.setBezeled(false);
    field.setDrawsBackground(false);
    // Selectable text fields get their own right-click menu (Translate,
    // Look Up, Search With Google, ...) from AppKit, which either
    // replaces or fights with the table's own "Show in Finder" context
    // menu depending on the click target. Non-selectable avoids that —
    // this is a read-only label, not an editable/copyable text field.
    field.setSelectable(false);
    field.setStringValue(&NSString::from_str(text));
    unsafe { Retained::cast_unchecked(field) }
}

fn project_name(project_path: &std::path::Path) -> String {
    project_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| project_path.to_string_lossy().to_string())
}

fn format_size(bytes: u64) -> String {
    const GB: f64 = 1_073_741_824.0;
    const MB: f64 = 1_048_576.0;
    let bytes = bytes as f64;
    if bytes >= GB {
        format!("{:.2} GB", bytes / GB)
    } else {
        format!("{:.1} MB", bytes / MB)
    }
}

impl CachesTableDataSource {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(CachesTableDataSourceIvars {
            mtm,
            rows: RefCell::new(Vec::new()),
            checkbox_target: RefCell::new(None),
        });
        unsafe { msg_send![super(this), init] }
    }

    // The checkbox buttons' target/action needs to reach code outside
    // this file (to refresh the "N GB selected" summary label), so the
    // caller hands in the app-wide AppTarget object here rather than this
    // data source registering itself as the target.
    pub fn set_checkbox_target(&self, target: Retained<objc2::runtime::AnyObject>) {
        *self.ivars().checkbox_target.borrow_mut() = Some(target);
    }

    pub fn set_rows(&self, entries: Vec<system_monitor::storage::CacheEntry>) {
        let rows = entries
            .into_iter()
            .map(|e| Row {
                path: e.path,
                project_path: e.project_path,
                size_bytes: e.size_bytes,
                checked: std::cell::Cell::new(false),
                views: RefCell::new(std::collections::HashMap::new()),
            })
            .collect();
        *self.ivars().rows.borrow_mut() = rows;
    }

    pub fn checked_paths(&self) -> Vec<PathBuf> {
        self.ivars()
            .rows
            .borrow()
            .iter()
            .filter(|r| r.checked.get())
            .map(|r| r.path.clone())
            .collect()
    }

    pub fn total_size_bytes(&self) -> u64 {
        self.ivars()
            .rows
            .borrow()
            .iter()
            .map(|r| r.size_bytes)
            .sum()
    }

    pub fn checked_total_size_bytes(&self) -> u64 {
        self.ivars()
            .rows
            .borrow()
            .iter()
            .filter(|r| r.checked.get())
            .map(|r| r.size_bytes)
            .sum()
    }

    // Called after a successful deletion so the deleted rows disappear
    // without needing a full re-scan.
    pub fn remove_checked(&self) {
        self.ivars().rows.borrow_mut().retain(|r| !r.checked.get());
    }

    // For the right-click "Show in Finder" context menu action.
    pub fn path_for_row(&self, row: NSInteger) -> Option<PathBuf> {
        if row < 0 {
            return None;
        }
        self.ivars()
            .rows
            .borrow()
            .get(row as usize)
            .map(|r| r.path.clone())
    }

    pub fn as_data_source(&self) -> &ProtocolObject<dyn NSTableViewDataSource> {
        ProtocolObject::from_ref(self)
    }

    pub fn as_delegate(&self) -> &ProtocolObject<dyn NSTableViewDelegate> {
        ProtocolObject::from_ref(self)
    }

    pub fn set_row_checked(&self, row: NSInteger, checked: bool) {
        if let Some(r) = self.ivars().rows.borrow().get(row as usize) {
            r.checked.set(checked);
        }
    }
}
