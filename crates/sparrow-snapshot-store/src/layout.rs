use std::path::{Path, PathBuf};

pub(crate) const ACTIVE_POINTER_FILE: &str = "active.json";
pub(crate) const MAX_MANIFEST_BYTES: u64 = 24 * 1024;
pub(crate) const MAX_POINTER_BYTES: u64 = 256;
pub(crate) const MAX_VALIDATOR_BYTES: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum DiskKind {
    M3u,
    Epg,
}

impl DiskKind {
    pub(crate) const fn directory_name(self) -> &'static str {
        match self {
            Self::M3u => "m3u",
            Self::Epg => "epg",
        }
    }

    pub(crate) const fn manifest_name(self) -> &'static str {
        match self {
            Self::M3u => "m3u",
            Self::Epg => "epg",
        }
    }

    pub(crate) fn parse_manifest_name(value: &str) -> Option<Self> {
        match value {
            "m3u" => Some(Self::M3u),
            "epg" => Some(Self::Epg),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Slot {
    A,
    B,
}

impl Slot {
    pub(crate) const ALL: [Self; 2] = [Self::A, Self::B];

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
        }
    }

    pub(crate) const fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "a" => Some(Self::A),
            "b" => Some(Self::B),
            _ => None,
        }
    }
}

pub(crate) struct Layout {
    root: PathBuf,
}

impl Layout {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn source_dir(&self, kind: DiskKind) -> PathBuf {
        self.root.join(kind.directory_name())
    }

    pub(crate) fn payload(&self, kind: DiskKind, slot: Slot) -> PathBuf {
        self.source_dir(kind)
            .join(format!("slot-{}.payload", slot.name()))
    }

    pub(crate) fn manifest(&self, kind: DiskKind, slot: Slot) -> PathBuf {
        self.source_dir(kind)
            .join(format!("slot-{}.manifest.json", slot.name()))
    }

    pub(crate) fn pointer(&self, kind: DiskKind) -> PathBuf {
        self.source_dir(kind).join(ACTIVE_POINTER_FILE)
    }

    pub(crate) fn stage(&self, kind: DiskKind, token: u64) -> PathBuf {
        self.source_dir(kind)
            .join(format!(".stage-{token}.payload"))
    }

    pub(crate) fn manifest_temp(&self, kind: DiskKind, token: u64) -> PathBuf {
        self.source_dir(kind).join(format!(".manifest-{token}.tmp"))
    }

    pub(crate) fn pointer_temp(&self, kind: DiskKind, token: u64) -> PathBuf {
        self.source_dir(kind).join(format!(".pointer-{token}.tmp"))
    }

    pub(crate) fn adopt_temp(&self, kind: DiskKind, token: u64) -> PathBuf {
        self.source_dir(kind).join(format!(".adopt-{token}.tmp"))
    }

    pub(crate) fn revalidate_temp(&self, kind: DiskKind, token: u64) -> PathBuf {
        self.source_dir(kind)
            .join(format!(".revalidate-{token}.tmp"))
    }
}

pub(crate) fn is_transient_name(name: &str) -> bool {
    (name.starts_with(".stage-") && name.ends_with(".payload"))
        || (name.starts_with(".manifest-") && name.ends_with(".tmp"))
        || (name.starts_with(".pointer-") && name.ends_with(".tmp"))
        || (name.starts_with(".adopt-") && name.ends_with(".tmp"))
        || (name.starts_with(".revalidate-") && name.ends_with(".tmp"))
}
