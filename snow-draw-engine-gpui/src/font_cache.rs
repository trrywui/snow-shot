//! Own used font bytes once. File-backed fontdb sources otherwise map/unmap the
//! same font for each outlined glyph and each bidi run, dominating long text.
use std::{
    collections::{HashMap, VecDeque},
    io::Read,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use usvg::fontdb;

#[derive(Default)]
pub(crate) struct FontBytes {
    files: HashMap<PathBuf, Arc<dyn AsRef<[u8]> + Send + Sync>>,
    bytes: usize,
}
pub(crate) type Cache = Arc<Mutex<FontBytes>>;

#[derive(Clone, PartialEq, Eq, Hash)]
struct FallbackKey {
    character: char,
    // The first face determines the preferred style; all faces are excluded.
    // Preserve order as well as membership to retain the exact resolver policy.
    used: Box<[fontdb::ID]>,
}

/// Cache resolution separately from shaped lines. Different labels commonly
/// need the same fallback; scanning and mapping every candidate font for each
/// label is much more expensive than shaping the selected font.
#[derive(Default)]
pub(crate) struct FallbackSelections {
    entries: HashMap<FallbackKey, Option<fontdb::ID>>,
    order: VecDeque<(FallbackKey, usize)>,
    bytes: usize,
}

impl FallbackSelections {
    const MAX_ENTRIES: usize = 4096;
    const MAX_BYTES: usize = 512 * 1024;

    pub fn get_or_resolve(
        &mut self,
        character: char,
        used: &[fontdb::ID],
        resolve: impl FnOnce() -> Option<fontdb::ID>,
    ) -> Option<fontdb::ID> {
        let bytes = 2 * (std::mem::size_of::<FallbackKey>() + std::mem::size_of_val(used))
            + std::mem::size_of::<Option<fontdb::ID>>()
            + 32;
        if bytes > Self::MAX_BYTES {
            return resolve();
        }
        let key = FallbackKey {
            character,
            used: used.into(),
        };
        if let Some(selected) = self.entries.get(&key) {
            return *selected;
        }
        let selected = resolve();
        while self.entries.len() >= Self::MAX_ENTRIES
            || self.bytes.saturating_add(bytes) > Self::MAX_BYTES
        {
            let Some((oldest, old_bytes)) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
            self.bytes -= old_bytes;
        }
        self.bytes += bytes;
        self.order.push_back((key.clone(), bytes));
        self.entries.insert(key, selected);
        selected
    }
}

pub(crate) fn owned_font(
    id: fontdb::ID,
    database: &mut Arc<fontdb::Database>,
    cache: &Cache,
) -> Option<fontdb::ID> {
    let mut face = database.face(id)?.clone();
    let fontdb::Source::File(path) = &face.source else {
        return Some(id);
    };
    let data = {
        let mut cache = cache.lock().ok()?;
        if let Some(data) = cache.files.get(path) {
            data.clone()
        } else {
            const BUDGET: usize = 64 * 1024 * 1024;
            let remaining = BUDGET.saturating_sub(cache.bytes);
            let file = std::fs::File::open(path).ok()?;
            if file.metadata().ok()?.len() > remaining as u64 {
                return Some(id);
            }
            let mut bytes = Vec::new();
            file.take(remaining as u64 + 1)
                .read_to_end(&mut bytes)
                .ok()?;
            if bytes.len() > remaining {
                return Some(id);
            }
            cache.bytes += bytes.len();
            let data: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(bytes);
            cache.files.insert(path.clone(), data.clone());
            data
        }
    };
    if let Some(existing) = database.faces().find(|candidate| candidate.index == face.index
        && matches!(&candidate.source, fontdb::Source::Binary(bytes) if Arc::ptr_eq(bytes, &data))) {
        return Some(existing.id);
    }
    face.id = fontdb::ID::dummy();
    face.source = fontdb::Source::Binary(data);
    Some(Arc::make_mut(database).push_face_info(face))
}

pub(crate) fn fallback_resolver() -> (usvg::FallbackSelectionFn<'static>, Cache) {
    let cache = Cache::default();
    let fallback = usvg::FontResolver::default_fallback_selector();
    let fallback_cache = cache.clone();
    (
        Box::new(move |character, excluded, database| {
            // The original file face and its owned-byte copy are the same font.
            let names: std::collections::HashSet<_> = excluded
                .iter()
                .filter_map(|id| database.face(*id))
                .map(|face| face.post_script_name.as_str())
                .collect();
            let mut excluded = excluded.to_vec();
            excluded.extend(
                database
                    .faces()
                    .filter(|face| names.contains(face.post_script_name.as_str()))
                    .map(|face| face.id),
            );
            let id = fallback(character, &excluded, database)?;
            owned_font(id, database, &fallback_cache)
        }),
        cache,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn faces() -> [fontdb::ID; 3] {
        let mut database = fontdb::Database::new();
        std::array::from_fn(|index| {
            database.push_face_info(fontdb::FaceInfo {
                id: fontdb::ID::dummy(),
                source: fontdb::Source::Binary(Arc::new(Vec::<u8>::new())),
                index: index as u32,
                families: Vec::new(),
                post_script_name: format!("fixture{index}"),
                style: fontdb::Style::Normal,
                weight: fontdb::Weight::NORMAL,
                stretch: fontdb::Stretch::Normal,
                monospaced: false,
            })
        })
    }

    #[test]
    fn fallback_cache_keeps_exact_font_exclusions_and_caches_missing_glyphs() {
        let [a, b, c] = faces();
        let mut cache = FallbackSelections::default();
        for (character, used, result) in [
            ('漢', vec![a], Some(b)),
            ('漢', vec![a, b], Some(c)),
            ('漢', vec![b, a], Some(a)),
            ('\u{10ffff}', vec![a], None),
        ] {
            assert_eq!(cache.get_or_resolve(character, &used, || result), result);
            assert_eq!(
                cache.get_or_resolve(character, &used, || panic!("repeated font scan")),
                result
            );
        }
        assert_eq!(cache.entries.len(), 4);
    }

    #[test]
    fn fallback_cache_bounds_entry_count_and_long_exclusion_chains() {
        let [a, b, _] = faces();
        let mut cache = FallbackSelections::default();
        for index in 0..5000 {
            cache.get_or_resolve(char::from_u32(0x1000 + index).unwrap(), &[a], || Some(b));
        }
        assert_eq!(cache.entries.len(), FallbackSelections::MAX_ENTRIES);
        assert!(cache.bytes <= FallbackSelections::MAX_BYTES);
        let long_chain = vec![a; 4096];
        for index in 0..32 {
            cache.get_or_resolve(char::from_u32(0x1000 + index).unwrap(), &long_chain, || {
                None
            });
        }
        assert!(cache.entries.len() < 32);
        assert_eq!(cache.order.len(), cache.entries.len());
        assert!(cache.bytes <= FallbackSelections::MAX_BYTES);
        let count = cache.entries.len();
        let oversized = vec![a; FallbackSelections::MAX_BYTES];
        assert_eq!(cache.get_or_resolve('漢', &oversized, || Some(b)), Some(b));
        assert_eq!(cache.entries.len(), count);
    }
}
