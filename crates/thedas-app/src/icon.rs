//! The OpenThedas emblem as the program's icon: the window's icon (Windows and X11 title bars
//! and task bars), the Dock icon on macOS (set at run time, since the game is not started from
//! an app bundle), and the picture in the launcher's header. The executable's own icon on
//! Windows comes from `assets/icon/openthedas.ico` through `build.rs`; `openthedas.icns` is
//! for a macOS app bundle.

/// The emblem, 256 pixels a side.
pub const PNG_256: &[u8] = include_bytes!("../assets/icon/emblem-256.png");

/// The emblem decoded to RGBA8: width, height, pixels.
pub fn rgba() -> Option<(u32, u32, Vec<u8>)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(PNG_256));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width, info.height);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        _ => return None,
    };
    (rgba.len() == (w * h * 4) as usize).then_some((w, h, rgba))
}

/// The emblem as a winit window icon.
pub fn window_icon() -> Option<winit::window::Icon> {
    let (w, h, rgba) = rgba()?;
    winit::window::Icon::from_rgba(rgba, w, h).ok()
}

/// Shows the emblem in the Dock. Call on the main thread once the event loop runs.
#[cfg(target_os = "macos")]
pub fn set_dock_icon() {
    use objc2::{AllocAnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let data = NSData::with_bytes(PNG_256);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: on the main thread (checked above), with a valid image.
    unsafe { app.setApplicationIconImage(Some(&image)) };
}

#[cfg(not(target_os = "macos"))]
pub fn set_dock_icon() {}

#[cfg(test)]
mod tests {
    #[test]
    fn emblem_decodes() {
        let (w, h, px) = super::rgba().expect("emblem decodes");
        assert_eq!((w, h), (256, 256));
        assert_eq!(px.len(), 256 * 256 * 4);
        assert!(super::window_icon().is_some());
    }
}
