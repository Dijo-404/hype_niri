use gtk::cairo;

pub fn rounded_region(x: i32, y: i32, width: i32, height: i32, radius: i32) -> cairo::Region {
    let region = cairo::Region::create();
    if width <= 1 || height <= 1 {
        return region;
    }
    let radius = radius.min(width / 2).min(height / 2).max(0);
    let _ = region.union_rectangle(&cairo::RectangleInt::new(
        x,
        y + radius,
        width,
        height - 2 * radius,
    ));
    for row in 0..radius {
        let dy = f64::from(radius - row) - 0.5;
        let mut inset = 0;
        while inset < radius {
            let dx = f64::from(radius - inset) - 0.5;
            if dx * dx + dy * dy <= f64::from(radius * radius) {
                break;
            }
            inset += 1;
        }
        for row in [y + row, y + height - 1 - row] {
            let _ = region.union_rectangle(&cairo::RectangleInt::new(
                x + inset,
                row,
                width - 2 * inset,
                1,
            ));
        }
    }
    region
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blur_rounds_corners_and_keeps_centre() {
        let region = rounded_region(10, 20, 400, 200, 22);
        assert!(!region.contains_point(10, 20));
        assert!(region.contains_point(32, 21));
        assert!(region.contains_point(11, 50));
        assert!(!region.contains_point(9, 100));
        assert!(rounded_region(0, 0, 0, 0, 22).is_empty());
        assert!(!rounded_region(0, 0, 76, 8, 22).contains_point(0, 0));
    }
}
