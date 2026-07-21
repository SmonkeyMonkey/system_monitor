// A custom NSView subclass that draws a circular progress ring with a
// percentage label in the middle (Activity-Monitor-widget style), used by
// the menubar app's System Monitor window. AppKit has no built-in circular
// progress control, so this draws the ring by hand with NSBezierPath in
// drawRect:.

use objc2::rc::Retained;
use objc2::{DefinedClass, define_class, msg_send};
use objc2_app_kit::{NSBezierPath, NSColor, NSLineCapStyle, NSView};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect};
use std::cell::Cell;

pub struct RingViewIvars {
    fraction: Cell<f32>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[name = "SystemMonitorRingView"]
    #[ivars = RingViewIvars]
    pub struct RingView;

    impl RingView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            self.draw();
        }
    }
);

impl RingView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(RingViewIvars {
            fraction: Cell::new(0.0),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this
    }

    // `fraction` is clamped to [0, 1] and represents how full the ring is
    // (e.g. CPU usage / 100). The center label is a separate NSTextField
    // overlaid by the caller, since drawing text directly in an NSView
    // subclass needs NSAttributedString plumbing this app doesn't need
    // otherwise.
    pub fn set_value(&self, fraction: f32) {
        self.ivars().fraction.set(fraction.clamp(0.0, 1.0));
        self.setNeedsDisplay(true);
    }

    fn draw(&self) {
        let bounds = self.bounds();
        let diameter = bounds.size.width.min(bounds.size.height) - 8.0;
        let center = NSPoint::new(bounds.size.width / 2.0, bounds.size.height / 2.0);
        let radius = diameter / 2.0;

        let track = NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
            NSPoint::new(center.x - radius, center.y - radius),
            objc2_foundation::NSSize::new(diameter, diameter),
        ));
        NSColor::separatorColor().setStroke();
        track.setLineWidth(6.0);
        track.stroke();

        let fraction = self.ivars().fraction.get() as f64;
        if fraction > 0.0 {
            let arc = NSBezierPath::bezierPath();
            arc.setLineWidth(6.0);
            arc.setLineCapStyle(NSLineCapStyle::Round);
            let start_angle = 90.0;
            let end_angle = 90.0 - 360.0 * fraction;
            arc.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle_clockwise(
                center,
                radius,
                start_angle,
                end_angle,
                true,
            );
            ring_color(fraction).setStroke();
            arc.stroke();
        }
    }
}

fn ring_color(fraction: f64) -> Retained<NSColor> {
    if fraction > 0.85 {
        NSColor::systemRedColor()
    } else if fraction > 0.6 {
        NSColor::systemOrangeColor()
    } else {
        NSColor::systemGreenColor()
    }
}
