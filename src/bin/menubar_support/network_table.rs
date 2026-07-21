// View-based NSTableView data source/delegate for the Network Info
// window: one row per active network interface, showing download/upload
// throughput since the last refresh, a stability indicator (based on
// packet errors), and IP addresses. Read-only, no checkboxes or context
// menu — simpler than caches_table.rs, which this mirrors.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSControlTextEditingDelegate, NSTableColumn, NSTableView, NSTableViewDataSource,
    NSTableViewDelegate,
};
use objc2_foundation::{MainThreadMarker, NSInteger, NSObject, NSObjectProtocol};
use std::cell::RefCell;

pub struct Row {
    pub display_name: String,
    pub download_bytes_per_tick: u64,
    pub upload_bytes_per_tick: u64,
    pub packet_errors: u64,
    pub ip_addresses: Vec<String>,
    // See caches_table.rs's `Row::views` for why these need to be kept
    // alive here rather than just returned from `view_for_row`.
    views: RefCell<std::collections::HashMap<String, Retained<objc2_app_kit::NSView>>>,
}

pub struct NetworkTableDataSourceIvars {
    mtm: MainThreadMarker,
    rows: RefCell<Vec<Row>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SystemMonitorNetworkTableDataSource"]
    #[ivars = NetworkTableDataSourceIvars]
    pub struct NetworkTableDataSource;

    unsafe impl NSObjectProtocol for NetworkTableDataSource {}

    unsafe impl NSTableViewDataSource for NetworkTableDataSource {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows(&self, _table_view: &NSTableView) -> NSInteger {
            self.ivars().rows.borrow().len() as NSInteger
        }
    }

    unsafe impl NSControlTextEditingDelegate for NetworkTableDataSource {}

    unsafe impl NSTableViewDelegate for NetworkTableDataSource {
        #[unsafe(method(tableView:viewForTableColumn:row:))]
        fn view_for_row(
            &self,
            _table_view: &NSTableView,
            column: Option<&NSTableColumn>,
            row: NSInteger,
        ) -> *const objc2_app_kit::NSView {
            let mtm = self.ivars().mtm;
            let rows = self.ivars().rows.borrow();
            let Some(data) = rows.get(row as usize) else {
                return std::ptr::null();
            };
            let identifier = column
                .map(|c| c.identifier().to_string())
                .unwrap_or_default();

            let mut views = data.views.borrow_mut();
            let view = views.entry(identifier.clone()).or_insert_with(|| {
                let text = match identifier.as_str() {
                    "interface" => data.display_name.clone(),
                    "download" => format_bytes(data.download_bytes_per_tick),
                    "upload" => format_bytes(data.upload_bytes_per_tick),
                    "status" => system_monitor::network::classify_stability(data.packet_errors)
                        .label()
                        .to_string(),
                    "ip" => data.ip_addresses.join(", "),
                    _ => String::new(),
                };
                build_text_view(mtm, &text)
            });
            // SAFETY: `view` is kept alive in `data.views` for as long as
            // this row exists, mirroring caches_table.rs.
            Retained::as_ptr(view)
        }
    }
);

fn build_text_view(mtm: MainThreadMarker, text: &str) -> Retained<objc2_app_kit::NSView> {
    use objc2_app_kit::NSTextField;
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

    let field = NSTextField::alloc(mtm);
    let field = NSTextField::initWithFrame(
        field,
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(200.0, 18.0)),
    );
    field.setEditable(false);
    field.setBezeled(false);
    field.setDrawsBackground(false);
    // Non-selectable to avoid AppKit's built-in text-field context menu
    // fighting with anything else in this window — same reasoning as
    // caches_table.rs's build_text_view.
    field.setSelectable(false);
    field.setStringValue(&NSString::from_str(text));
    unsafe { Retained::cast_unchecked(field) }
}

fn format_bytes(bytes: u64) -> String {
    const MB: f64 = 1_048_576.0;
    const KB: f64 = 1_024.0;
    let bytes = bytes as f64;
    if bytes >= MB {
        format!("{:.2} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes / KB)
    } else {
        format!("{bytes:.0} B")
    }
}

impl NetworkTableDataSource {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(NetworkTableDataSourceIvars {
            mtm,
            rows: RefCell::new(Vec::new()),
        });
        unsafe { msg_send![super(this), init] }
    }

    pub fn set_rows(&self, interfaces: Vec<system_monitor::network::NetworkInterfaceInfo>) {
        let rows = interfaces
            .into_iter()
            .map(|i| Row {
                display_name: i.display_name,
                download_bytes_per_tick: i.download_bytes_per_tick,
                upload_bytes_per_tick: i.upload_bytes_per_tick,
                packet_errors: i.packet_errors,
                ip_addresses: i.ip_addresses,
                views: RefCell::new(std::collections::HashMap::new()),
            })
            .collect();
        *self.ivars().rows.borrow_mut() = rows;
    }

    pub fn as_data_source(&self) -> &ProtocolObject<dyn NSTableViewDataSource> {
        ProtocolObject::from_ref(self)
    }

    pub fn as_delegate(&self) -> &ProtocolObject<dyn NSTableViewDelegate> {
        ProtocolObject::from_ref(self)
    }
}
