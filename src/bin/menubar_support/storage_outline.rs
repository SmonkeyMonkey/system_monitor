// NSOutlineView data source for the Storage tab's "largest files and
// folders" results. The whole subtree is scanned up front (see
// storage.rs's scan_largest_entries — it walks every directory once and
// records each one's full children list, not just its total size), so
// building the EntryItem tree here is a pure in-memory conversion with no
// disk I/O — expanding any row, however deep, never re-scans anything.
// An earlier version scanned lazily on expand instead, which looked fine
// for shallow trees but froze the UI for several seconds expanding large
// directories (e.g. a user's home folder) since that scan ran
// synchronously on the main thread inside this data source.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, define_class, msg_send};
use objc2_app_kit::{NSOutlineView, NSOutlineViewDataSource, NSTableColumn};
use objc2_foundation::{MainThreadMarker, NSInteger, NSObject, NSObjectProtocol, NSString};
use std::cell::RefCell;
use std::path::PathBuf;

pub struct EntryItemIvars {
    path: RefCell<PathBuf>,
    size_bytes: std::cell::Cell<u64>,
    children: RefCell<Vec<Retained<EntryItem>>>,
    is_dir: std::cell::Cell<bool>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "SystemMonitorStorageEntryItem"]
    #[ivars = EntryItemIvars]
    pub struct EntryItem;

    unsafe impl NSObjectProtocol for EntryItem {}
);

impl EntryItem {
    fn from_entry(mtm: MainThreadMarker, entry: system_monitor::storage::Entry) -> Retained<Self> {
        let is_dir = entry.is_dir();
        let children: Vec<Retained<EntryItem>> = entry
            .children
            .unwrap_or_default()
            .into_iter()
            .map(|c| EntryItem::from_entry(mtm, c))
            .collect();

        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(EntryItemIvars {
            path: RefCell::new(entry.path),
            size_bytes: std::cell::Cell::new(entry.size_bytes),
            children: RefCell::new(children),
            is_dir: std::cell::Cell::new(is_dir),
        });
        unsafe { msg_send![super(this), init] }
    }
}

pub struct StorageOutlineDataSourceIvars {
    mtm: MainThreadMarker,
    roots: RefCell<Vec<Retained<EntryItem>>>,
    last_value: RefCell<Option<Retained<NSString>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "SystemMonitorStorageOutlineDataSource"]
    #[ivars = StorageOutlineDataSourceIvars]
    pub struct StorageOutlineDataSource;

    unsafe impl NSObjectProtocol for StorageOutlineDataSource {}

    unsafe impl NSOutlineViewDataSource for StorageOutlineDataSource {
        #[unsafe(method(outlineView:numberOfChildrenOfItem:))]
        fn number_of_children(
            &self,
            _outline_view: &NSOutlineView,
            item: Option<&AnyObject>,
        ) -> NSInteger {
            match item {
                None => self.ivars().roots.borrow().len() as NSInteger,
                Some(item) => as_entry(item)
                    .map(|e| e.ivars().children.borrow().len())
                    .unwrap_or(0) as NSInteger,
            }
        }

        #[unsafe(method(outlineView:isItemExpandable:))]
        fn is_expandable(&self, _outline_view: &NSOutlineView, item: &AnyObject) -> bool {
            as_entry(item).is_some_and(|e| e.ivars().is_dir.get())
        }

        #[unsafe(method(outlineView:child:ofItem:))]
        fn child_of_item(
            &self,
            _outline_view: &NSOutlineView,
            index: NSInteger,
            item: Option<&AnyObject>,
        ) -> *const AnyObject {
            // Retained for as long as it's reachable from `self.roots`
            // (root items directly, nested ones via their parent's
            // `children`), so a borrowed pointer is safe here — see the
            // equivalent note in process_outline.rs for why `Retained`
            // can't be returned directly from this selector.
            match item {
                None => {
                    let roots = self.ivars().roots.borrow();
                    Retained::as_ptr(&roots[index as usize]).cast()
                }
                Some(item) => {
                    let entry = as_entry(item).expect("child requested of a non-entry item");
                    let children = entry.ivars().children.borrow();
                    Retained::as_ptr(&children[index as usize]).cast()
                }
            }
        }

        #[unsafe(method(outlineView:objectValueForTableColumn:byItem:))]
        fn object_value(
            &self,
            _outline_view: &NSOutlineView,
            column: Option<&NSTableColumn>,
            item: Option<&AnyObject>,
        ) -> Option<&NSString> {
            let entry = as_entry(item?)?;
            let is_size_column = column?.identifier().to_string() == "size";

            let text = if is_size_column {
                format_size(entry.ivars().size_bytes.get())
            } else {
                entry
                    .ivars()
                    .path
                    .borrow()
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| entry.ivars().path.borrow().to_string_lossy().to_string())
            };

            *self.ivars().last_value.borrow_mut() = Some(NSString::from_str(&text));
            // SAFETY: same reasoning as process_outline.rs — AppKit reads
            // this synchronously before any call that could invalidate it.
            let ptr: *const NSString = self.ivars().last_value.borrow().as_ref()?.as_ref();
            Some(unsafe { &*ptr })
        }
    }
);

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

fn as_entry(item: &AnyObject) -> Option<&EntryItem> {
    item.downcast_ref::<EntryItem>()
}

// For the right-click "Show in Finder" context menu action: the caller
// gets an outline row via `itemAtRow`, and needs the underlying path.
pub fn path_for_item(item: &AnyObject) -> Option<std::path::PathBuf> {
    Some(as_entry(item)?.ivars().path.borrow().clone())
}

impl StorageOutlineDataSource {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(StorageOutlineDataSourceIvars {
            mtm,
            roots: RefCell::new(Vec::new()),
            last_value: RefCell::new(None),
        });
        unsafe { msg_send![super(this), init] }
    }

    pub fn set_roots(&self, entries: Vec<system_monitor::storage::Entry>) {
        let mtm = self.ivars().mtm;
        let items = entries
            .into_iter()
            .map(|e| EntryItem::from_entry(mtm, e))
            .collect();
        *self.ivars().roots.borrow_mut() = items;
    }

    pub fn as_protocol(&self) -> &ProtocolObject<dyn NSOutlineViewDataSource> {
        ProtocolObject::from_ref(self)
    }
}
