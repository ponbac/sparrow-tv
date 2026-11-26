//! Persistent downscaled BGRA frames. Call on a bounded blocking worker.
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, SystemTime},
};

use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use image::RgbaImage;
use sha2::{Digest, Sha256};

use crate::{
    DecodedChannelIcon,
    raster::{MAX_OUTPUT_EDGE, check_dimensions},
};

const MAX_BYTES: u64 = 64 << 20;
const MAX_AGE: Duration = Duration::from_secs(7 * 86400);

pub struct DiskCache {
    directory: PathBuf,
    writes: AtomicUsize,
}

pub struct CachedIcon {
    pub icon: DecodedChannelIcon,
    pub stale: bool,
}

impl DiskCache {
    pub fn new(cache_dir: &Path) -> Self {
        Self {
            directory: cache_dir.join("channel-icons-v1"),
            writes: AtomicUsize::new(0),
        }
    }

    fn path(&self, location: &str) -> PathBuf {
        self.directory
            .join(format!("{:x}", Sha256::digest(location.as_bytes())))
    }

    pub fn load(&self, location: &str) -> Option<CachedIcon> {
        let path = self.path(location);
        let result = (|| {
            let file = File::open(&path)?;
            let modified = file.metadata()?.modified()?;
            let icon = read_frame(&file)?;
            Ok(CachedIcon {
                icon,
                stale: modified.elapsed().unwrap_or_default() >= MAX_AGE,
            })
        })();
        match result {
            Ok(hit) => Some(hit),
            Err(e) => {
                if matches!(
                    io::Error::kind(&e),
                    io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof
                ) {
                    let _ = fs::remove_file(path);
                }
                None
            }
        }
    }

    /// Returns true when the caller should schedule a cache sweep.
    pub fn store(&self, location: &str, icon: &DecodedChannelIcon) -> io::Result<bool> {
        fs::create_dir_all(&self.directory)?;
        let mut tmp = tempfile::NamedTempFile::new_in(&self.directory)?;
        let mut gzip = GzEncoder::new(tmp.as_file_mut(), Compression::fast());
        let (sw, sh) = icon.source_dimensions();
        for dimension in [sw, sh, icon.bgra.width(), icon.bgra.height()] {
            gzip.write_all(&dimension.to_le_bytes())?;
        }
        gzip.write_all(icon.bgra.as_raw())?;
        gzip.finish()?;
        tmp.persist(self.path(location))?;
        Ok(self
            .writes
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(32))
    }

    pub fn trim(&self) -> io::Result<()> {
        self.trim_to(MAX_BYTES)
    }

    fn trim_to(&self, budget: u64) -> io::Result<()> {
        let mut entries = Vec::new();
        let mut size = 0;
        for entry in fs::read_dir(&self.directory)?.flatten() {
            // A concurrent store may still be writing its temporary file.
            if entry.file_name().as_encoded_bytes().starts_with(b".") {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                entries.push((
                    meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    entry.path(),
                    meta.len(),
                ));
                size += meta.len();
            }
        }
        entries.sort_unstable();
        for (_, path, len) in entries {
            if size <= budget {
                break;
            }
            if fs::remove_file(path).is_ok() {
                size -= len;
            }
        }
        Ok(())
    }
}

fn read_frame(file: &File) -> io::Result<DecodedChannelIcon> {
    // Include the original geometry used for Guide layout. Reading to EOF
    // verifies the gzip checksum, with a hard bound on the expanded frame.
    let invalid = || io::Error::from(io::ErrorKind::InvalidData);
    const FRAME_BYTES: u64 = (MAX_OUTPUT_EDGE as u64).pow(2) * 4 + 4 * size_of::<u32>() as u64;
    // Allow 1% for DEFLATE block overhead plus the gzip header and trailer.
    const MAX_GZIP_BYTES: u64 = FRAME_BYTES + FRAME_BYTES.div_ceil(100) + 18;
    if file.metadata()?.len() > MAX_GZIP_BYTES {
        return Err(invalid());
    }
    let mut gzip = GzDecoder::new(file);
    let mut dimension = || {
        let mut bytes = [0; 4];
        gzip.read_exact(&mut bytes)?;
        Ok::<_, io::Error>(u32::from_le_bytes(bytes))
    };
    let source_dimensions = (dimension()?, dimension()?);
    check_dimensions(source_dimensions.0, source_dimensions.1).map_err(|_| invalid())?;
    let (w, h) = (dimension()?, dimension()?);
    if w.min(h) == 0 || w.max(h) > MAX_OUTPUT_EDGE {
        return Err(invalid());
    }
    let mut bgra = RgbaImage::new(w, h);
    gzip.read_exact(&mut bgra)?;
    if gzip.read(&mut [0])? != 0 {
        return Err(invalid());
    }
    Ok(DecodedChannelIcon {
        source_dimensions,
        bgra,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icon() -> DecodedChannelIcon {
        DecodedChannelIcon {
            source_dimensions: (512, 2),
            bgra: RgbaImage::from_pixel(256, 1, image::Rgba([255, 80, 9, 127])),
        }
    }

    #[test]
    fn persisted_frames_keep_pixels_geometry_and_refresh_age_across_reads() {
        let root = tempfile::tempdir().unwrap();
        DiskCache::new(root.path()).store("a", &icon()).unwrap();
        let disk = DiskCache::new(root.path());
        let hit = disk.load("a").unwrap();
        assert!(!hit.stale);
        assert_eq!(hit.icon.into_bgra(), icon().into_bgra());
        File::open(disk.path("a"))
            .unwrap()
            .set_modified(SystemTime::now() - MAX_AGE)
            .unwrap();
        for _ in 0..2 {
            let hit = disk.load("a").unwrap();
            assert!(hit.stale);
            assert_eq!(hit.icon.source_dimensions(), (512, 2));
        }
    }

    #[test]
    fn corrupt_frame_is_removed_and_can_be_replaced() {
        let root = tempfile::tempdir().unwrap();
        let disk = DiskCache::new(root.path());
        disk.store("a", &icon()).unwrap();
        let path = disk.path("a");
        let mut bytes = fs::read(&path).unwrap();
        bytes.pop();
        fs::write(&path, bytes).unwrap();
        assert!(disk.load("a").is_none());
        assert!(!path.exists());
        disk.store("a", &icon()).unwrap();
        assert!(disk.load("a").is_some());
    }

    #[test]
    fn trimming_preserves_the_last_fetched_frame() {
        let root = tempfile::tempdir().unwrap();
        let disk = DiskCache::new(root.path());
        for location in ["b", "c"] {
            disk.store(location, &icon()).unwrap();
            File::open(disk.path(location))
                .unwrap()
                .set_modified(SystemTime::UNIX_EPOCH)
                .unwrap();
        }
        disk.store("a", &icon()).unwrap();
        assert!(disk.load("b").is_some());
        disk.trim_to(fs::metadata(disk.path("a")).unwrap().len())
            .unwrap();
        assert!(disk.load("a").is_some());
        assert!(disk.load("b").is_none());
        assert!(disk.load("c").is_none());
    }
}
