use std::path::PathBuf;

use tower_http::services::{ServeDir, ServeFile};

pub(crate) fn service(app_root: PathBuf) -> ServeDir<ServeFile> {
    let index = app_root.join("index.html");
    ServeDir::new(app_root)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(index))
}
