// A custom NSView subclass that draws a simple line chart over a rolling
// window of recent samples (Activity-Monitor-graph style), used by the
// Network window's Status/Speed tabs. Same drawing approach as
// ring_view.rs — hand-drawn with NSBezierPath in drawRect: since AppKit has
// no built-in sparkline control.

use objc2::rc::Retained;
use objc2::{DefinedClass, define_class, msg_send};
use objc2_app_kit::{NSBezierPath, NSColor};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect};
use std::cell::RefCell;
use std::collections::VecDeque;

// 60 samples at the Network window's 5s refresh cadence covers the last 5
// minutes, long enough to see a trend without the view needing to scroll
// or thin out old data.
const MAX_SAMPLES: usize = 60;

pub struct SparklineViewIvars {
    samples: RefCell<VecDeque<f32>>,
}

define_class!(
    #[unsafe(super(objc2_app_kit::NSView))]
    #[name = "SystemMonitorSparklineView"]
    #[ivars = SparklineViewIvars]
    pub struct SparklineView;

    impl SparklineView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            self.draw();
        }
    }
);

impl SparklineView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(SparklineViewIvars {
            samples: RefCell::new(VecDeque::with_capacity(MAX_SAMPLES)),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this
    }

    pub fn push_sample(&self, value: f32) {
        let mut samples = self.ivars().samples.borrow_mut();
        if samples.len() == MAX_SAMPLES {
            samples.pop_front();
        }
        samples.push_back(value);
        drop(samples);
        self.setNeedsDisplay(true);
    }

    fn draw(&self) {
        let bounds = self.bounds();
        NSColor::separatorColor().setStroke();
        let baseline = NSBezierPath::bezierPath();
        baseline.moveToPoint(NSPoint::new(0.0, 1.0));
        baseline.lineToPoint(NSPoint::new(bounds.size.width, 1.0));
        baseline.setLineWidth(1.0);
        baseline.stroke();

        let samples = self.ivars().samples.borrow();
        if samples.len() < 2 {
            return;
        }

        // Autoscale to the visible window's own max rather than a fixed
        // cap: byte rates swing between a few KB/s and tens of MB/s
        // depending on what's running, so no single fixed ceiling reads
        // well across that whole range.
        let scale_max = samples.iter().cloned().fold(0.0f32, f32::max).max(1.0);

        let top_inset = 4.0;
        let usable_height = (bounds.size.height - top_inset) as f32;
        let step = bounds.size.width / (MAX_SAMPLES - 1) as f64;
        let start_x = bounds.size.width - step * (samples.len() - 1) as f64;

        let line = NSBezierPath::bezierPath();
        for (i, &value) in samples.iter().enumerate() {
            let x = start_x + step * i as f64;
            let y = (value / scale_max * usable_height) as f64 + 1.0;
            let point = NSPoint::new(x, y);
            if i == 0 {
                line.moveToPoint(point);
            } else {
                line.lineToPoint(point);
            }
        }
        line.setLineWidth(1.5);
        NSColor::systemBlueColor().setStroke();
        line.stroke();
    }
}
