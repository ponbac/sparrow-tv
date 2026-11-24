use std::borrow::Cow;

use gpui::App;

// Static width and weight instances of the Tauri Guide's pinned Saira font.
pub(super) fn register(app: &App) {
    let fonts: &[&[u8]] = &[
        include_bytes!("../assets/fonts/sparrow-guide-72-700.ttf"),
        include_bytes!("../assets/fonts/sparrow-guide-76-700.ttf"),
        include_bytes!("../assets/fonts/sparrow-guide-80-600.ttf"),
        include_bytes!("../assets/fonts/sparrow-guide-80-650.ttf"),
        include_bytes!("../assets/fonts/sparrow-guide-80-700.ttf"),
        include_bytes!("../assets/fonts/sparrow-guide-82-650.ttf"),
        include_bytes!("../assets/fonts/sparrow-guide-84-650.ttf"),
        include_bytes!("../assets/fonts/sparrow-guide-84-700.ttf"),
        include_bytes!("../assets/fonts/sparrow-guide-92-400.ttf"),
        include_bytes!("../assets/fonts/sparrow-guide-92-500.ttf"),
        include_bytes!("../assets/fonts/sparrow-guide-92-600.ttf"),
        include_bytes!("../assets/fonts/sparrow-guide-92-700.ttf"),
        include_bytes!("../assets/fonts/sparrow-guide-100-600.ttf"),
        include_bytes!("../assets/fonts/sparrow-guide-110-250.ttf"),
        include_bytes!("../assets/fonts/sparrow-guide-110-400.ttf"),
    ];
    if app
        .text_system()
        .add_fonts(fonts.iter().copied().map(Cow::Borrowed).collect())
        .is_err()
    {
        log::warn!("Sparrow Guide fonts are unavailable");
    }
}
