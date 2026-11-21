use std::{
    cmp::Ordering,
    fmt::{self, Debug, Formatter},
    hash::{Hash, Hasher},
    sync::{Arc, OnceLock},
};

/// A compact owned slice into a bounded `Arc<String>` text arena.
///
/// Value semantics (`Eq`, `Ord`, `Hash`, `Debug`) operate strictly on the
/// selected slice so identical text from different page arenas compares and
/// hashes identically, and `Debug` never prints surrounding arena content.
#[derive(Clone)]
pub(crate) struct SharedText {
    arena: Arc<String>,
    start: u32,
    end: u32,
}

impl SharedText {
    pub(crate) fn empty() -> Self {
        static EMPTY: OnceLock<Arc<String>> = OnceLock::new();
        Self {
            arena: Arc::clone(EMPTY.get_or_init(|| Arc::new(String::new()))),
            start: 0,
            end: 0,
        }
    }

    pub(crate) fn from_owned(value: String) -> Self {
        if value.is_empty() {
            return Self::empty();
        }
        let end = u32::try_from(value.len()).expect("domain text fits in u32");
        Self {
            arena: Arc::new(value),
            start: 0,
            end,
        }
    }

    pub(crate) fn from_arena_slice(arena: Arc<String>, start: u32, end: u32) -> Self {
        debug_assert!(start <= end);
        debug_assert!(arena.get(start as usize..end as usize).is_some());
        Self { arena, start, end }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.arena[self.start as usize..self.end as usize]
    }
}

impl AsRef<str> for SharedText {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl PartialEq for SharedText {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for SharedText {}

impl PartialOrd for SharedText {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SharedText {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl Hash for SharedText {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl Debug for SharedText {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        Debug::fmt(self.as_str(), formatter)
    }
}

/// A bounded page-local text arena that packs all strings of one projected
/// `Page<T>` into a single `Arc<String>` allocation.
pub(crate) struct PageTextArena {
    buffer: String,
}

#[derive(Clone, Copy)]
pub(crate) struct TextSlot {
    start: u32,
    end: u32,
}

impl TextSlot {
    pub(crate) const fn empty() -> Self {
        Self { start: 0, end: 0 }
    }
}

impl PageTextArena {
    pub(crate) fn with_capacity(bytes: usize) -> Self {
        Self {
            buffer: String::with_capacity(bytes),
        }
    }

    pub(crate) fn push_str(&mut self, value: &str) -> TextSlot {
        if value.is_empty() {
            return TextSlot::empty();
        }
        let start = u32::try_from(self.buffer.len()).expect("bounded page arena fits in u32");
        self.buffer.push_str(value);
        let end = u32::try_from(self.buffer.len()).expect("bounded page arena fits in u32");
        TextSlot { start, end }
    }

    pub(crate) fn push_channel_id_digest(&mut self, digest: &[u8; 32]) -> TextSlot {
        let hex = blake3::Hash::from_bytes(*digest).to_hex();
        let prefix = super::CHANNEL_ID_PREFIX.as_bytes();
        let hex_bytes = hex.as_str().as_bytes();
        let mut stack = [0_u8; 68];
        debug_assert_eq!(prefix.len() + hex_bytes.len(), stack.len());
        stack[..prefix.len()].copy_from_slice(prefix);
        stack[prefix.len()..].copy_from_slice(hex_bytes);
        let encoded = std::str::from_utf8(&stack).expect("channel id prefix and hex are UTF-8");
        self.push_str(encoded)
    }

    pub(crate) fn finish(self) -> FrozenPageTextArena {
        FrozenPageTextArena {
            primary: (!self.buffer.is_empty()).then(|| Arc::new(self.buffer)),
        }
    }
}

pub(crate) struct FrozenPageTextArena {
    primary: Option<Arc<String>>,
}

impl FrozenPageTextArena {
    pub(crate) fn resolve(&self, slot: TextSlot) -> SharedText {
        if slot.start == slot.end {
            return SharedText::empty();
        }
        let arena = Arc::clone(
            self.primary
                .as_ref()
                .expect("non-empty slot has backing arena"),
        );
        SharedText::from_arena_slice(arena, slot.start, slot.end)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::hash_map::DefaultHasher,
        hash::{Hash, Hasher},
        mem::size_of,
        sync::Arc,
    };

    use super::{PageTextArena, SharedText};

    #[test]
    fn shared_text_fits_within_sixteen_bytes_and_uses_slice_value_semantics() {
        assert!(size_of::<SharedText>() <= 16);
        assert_eq!(size_of::<SharedText>(), size_of::<Arc<String>>() + 8);

        let mut arena_a = PageTextArena::with_capacity(32);
        let _prefix = arena_a.push_str("neighbor-before-");
        let slot_a = arena_a.push_str("target-value");
        let frozen_a = arena_a.finish();

        let mut arena_b = PageTextArena::with_capacity(32);
        let slot_b = arena_b.push_str("target-value");
        let _suffix = arena_b.push_str("-neighbor-after");
        let frozen_b = arena_b.finish();

        let text_a = frozen_a.resolve(slot_a);
        let text_b = frozen_b.resolve(slot_b);
        let text_owned = SharedText::from_owned("target-value".to_owned());

        assert_eq!(text_a, text_b);
        assert_eq!(text_a, text_owned);
        assert_eq!(text_a.cmp(&text_b), std::cmp::Ordering::Equal);

        let hash = |text: &SharedText| {
            let mut hasher = DefaultHasher::new();
            text.hash(&mut hasher);
            hasher.finish()
        };
        assert_eq!(hash(&text_a), hash(&text_b));
        assert_eq!(hash(&text_a), hash(&text_owned));

        let debug = format!("{text_a:?}");
        assert_eq!(debug, "\"target-value\"");
        assert!(!debug.contains("neighbor-before"));
    }
}
