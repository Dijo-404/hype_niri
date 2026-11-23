use gtk::{glib, graphene, gsk, prelude::*, subclass::prelude::*};
use std::cell::{Cell, OnceCell, RefCell};

mod imp {
    use super::*;
    #[derive(Default)]
    pub struct Morph {
        pub child: OnceCell<gtk::Widget>,
        pub size: Cell<(f64, f64)>,
        pub from: Cell<(f64, f64)>,
        pub target: Cell<(f64, f64)>,
        pub start: Cell<i64>,
        pub header: Cell<i32>,
        pub tick: RefCell<Option<gtk::TickCallbackId>>,
    }
    #[glib::object_subclass]
    impl ObjectSubclass for Morph {
        const NAME: &'static str = "HypeNotchMorph";
        type Type = super::Morph;
        type ParentType = gtk::Widget;
    }
    impl ObjectImpl for Morph {
        fn dispose(&self) {
            if let Some(tick) = self.tick.take() {
                tick.remove();
            }
            if let Some(child) = self.child.get() {
                child.unparent();
            }
        }
    }
    impl WidgetImpl for Morph {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::ConstantSize
        }
        fn measure(&self, orientation: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            let (width, height) = self.size.get();
            // Even widths keep the centred shape from shimmering by half a pixel.
            let size = if orientation == gtk::Orientation::Horizontal {
                2 * (width / 2.0 + 0.5) as i32
            } else {
                height.round() as i32
            };
            (size, size, -1, -1)
        }
        fn size_allocate(&self, width: i32, _: i32, _: i32) {
            if let Some(child) = self.child.get() {
                let height = child.measure(gtk::Orientation::Vertical, 400).1;
                let header = self.header.get().max(9);
                // Below peek height the content rides the bottom edge, so the pill stays
                // empty and the header slides down as the shape grows.
                let shown = ((self.size.get().1 - 8.0) / f64::from(header - 8)).clamp(0.0, 1.0);
                let transform = gsk::Transform::new().translate(&graphene::Point::new(
                    (width - 400) as f32 / 2.0,
                    -((1.0 - shown) * f64::from(header)).round() as f32,
                ));
                child.allocate(400, height, -1, Some(transform));
            }
        }
        #[allow(deprecated)] // Named colors preserve the existing GTK stylesheet.
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let width = obj.width() as f32;
            let height = obj.height() as f32;
            if width <= 1.0 || height <= 1.0 {
                return;
            }
            let radius = 22.0_f32.min(width / 2.0).min(height / 2.0);
            let bounds = graphene::Rect::new(0.0, 0.0, width, height);
            let rounded = gsk::RoundedRect::from_rect(bounds, radius);
            let style = obj.style_context();
            let fill = style
                .lookup_color("notch_bg")
                .unwrap_or_else(|| gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 0.75));
            let edge = style
                .lookup_color("notch_border")
                .unwrap_or_else(|| gtk::gdk::RGBA::new(1.0, 1.0, 1.0, 0.18));
            snapshot.push_rounded_clip(&rounded);
            snapshot.append_color(&fill, &bounds);
            if let Some(child) = self.child.get() {
                obj.snapshot_child(child, snapshot);
            }
            snapshot.pop();
            snapshot.append_border(&rounded, &[2.0; 4], &[edge; 4]);
        }
    }
}

glib::wrapper! {
    pub struct Morph(ObjectSubclass<imp::Morph>) @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}
impl Morph {
    pub fn new(child: &impl IsA<gtk::Widget>) -> Self {
        let obj: Self = glib::Object::new();
        obj.set_widget_name("notch-morph");
        obj.set_halign(gtk::Align::Center);
        obj.imp().size.set((76.0, 8.0));
        obj.imp().target.set((76.0, 8.0));
        child.set_parent(&obj);
        obj.imp().child.set(child.as_ref().clone()).unwrap();
        obj
    }
    pub fn animating(&self) -> bool {
        self.imp().tick.borrow().is_some()
    }
    pub fn retarget(&self, width: f64, height: f64, header: i32) {
        let imp = self.imp();
        imp.header.set(header);
        if imp.target.get() == (width, height) {
            return;
        }
        imp.from.set(imp.size.get());
        imp.target.set((width, height));
        imp.start.set(glib::monotonic_time());
        if !self.settings().is_gtk_enable_animations() || !self.is_mapped() {
            if let Some(tick) = imp.tick.take() {
                tick.remove();
            }
            imp.size.set((width, height));
        } else if imp.tick.borrow().is_none() {
            let weak = self.downgrade();
            let tick = self.add_tick_callback(move |_, clock| {
                let Some(obj) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let imp = obj.imp();
                let t = (clock.frame_time() - imp.start.get()) as f64 / 150_000.0;
                let p = crate::model::ease(t);
                let (x, y) = imp.from.get();
                let (width, height) = imp.target.get();
                imp.size.set((x + (width - x) * p, y + (height - y) * p));
                obj.queue_resize();
                if t >= 1.0 {
                    imp.tick.take();
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            });
            *imp.tick.borrow_mut() = Some(tick);
        }
        self.queue_resize();
    }
}
