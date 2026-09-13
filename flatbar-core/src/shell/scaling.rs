//! Scaling and HiDPI utilities according to Spec A.

/// Convert logical pixel dimension to buffer pixel dimension given a scale factor.
/// Buffer size = ceil(logical * scale).
pub fn logical_to_buffer(logical: u32, scale: f64) -> u32 {
    ((logical as f64) * scale).ceil() as u32
}

/// Convert buffer pixel coordinate to logical pixel coordinate given a scale factor.
pub fn buffer_to_logical(buffer: u32, scale: f64) -> f64 {
    (buffer as f64) / scale
}

/// Maximum menu popup width: ¼ of the output width. Menu popups are
/// content-fit but never exceed this cap.
pub fn quarter_output_width(output_width: u32) -> u32 {
    (output_width / 4).max(80)
}

/// Convert logical rectangle (x, y, width, height) to buffer rectangle.
pub fn logical_rect_to_buffer(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale: f64,
) -> (i32, i32, u32, u32) {
    let bx = ((x as f64) * scale).floor() as i32;
    let by = ((y as f64) * scale).floor() as i32;
    let bw = ((width as f64) * scale).ceil() as u32;
    let bh = ((height as f64) * scale).ceil() as u32;
    (bx, by, bw, bh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_logical_to_buffer() {
        assert_eq!(logical_to_buffer(30, 1.0), 30);
        assert_eq!(logical_to_buffer(30, 1.25), 38);
        assert_eq!(logical_to_buffer(30, 1.5), 45);
        assert_eq!(logical_to_buffer(30, 2.0), 60);
    }

    #[test]
    fn test_logical_rect_to_buffer() {
        let (x, y, w, h) = logical_rect_to_buffer(10, 0, 100, 30, 1.25);
        assert_eq!(x, 12);
        assert_eq!(y, 0);
        assert_eq!(w, 125);
        assert_eq!(h, 38);
    }
}
