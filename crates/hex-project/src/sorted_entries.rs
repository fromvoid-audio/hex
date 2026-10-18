//! Capacity-visible control-side sorted ownership. No allocation provenance or
//! child-storage/admission claim follows from using this generic container.
use crate::Invalid;
use std::mem;

/// Unique immutable ordered keys, with one actual private Vec backing.
/// All construction, cloning, growth and potentially final drops belong on
/// control. Value borrows cannot mutate the key or invalidate ordering.
#[derive(Debug, PartialEq, Eq)]
pub struct SortedEntries<K, V> {
    entries: Vec<(K, V)>,
}
impl<K, V> Default for SortedEntries<K, V> {
    fn default() -> Self {
        Self::new()
    }
}
impl<K, V> SortedEntries<K, V> {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn capacity(&self) -> usize {
        self.entries.capacity()
    }
    /// Actual requested Vec backing, not a len estimate or allocator RSS.
    /// The inline Vec header belongs to the enclosing owner, charged once.
    pub fn backing_bytes(&self) -> Result<usize, Invalid> {
        self.capacity()
            .checked_mul(mem::size_of::<(K, V)>())
            .ok_or(Invalid("sorted entry backing size overflow"))
    }
    pub fn as_slice(&self) -> &[(K, V)] {
        &self.entries
    }
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (&K, &V)> + ExactSizeIterator {
        self.entries.iter().map(|(key, value)| (key, value))
    }
    pub fn keys(&self) -> impl DoubleEndedIterator<Item = &K> + ExactSizeIterator {
        self.entries.iter().map(|(key, _)| key)
    }
    pub fn values(&self) -> impl DoubleEndedIterator<Item = &V> + ExactSizeIterator {
        self.entries.iter().map(|(_, value)| value)
    }
    pub fn values_mut(&mut self) -> impl DoubleEndedIterator<Item = &mut V> + ExactSizeIterator {
        self.entries.iter_mut().map(|(_, value)| value)
    }
    pub fn clear(&mut self) {
        self.entries.clear();
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&K, &mut V) -> bool) {
        self.entries.retain_mut(|(key, value)| keep(key, value));
    }
    /// This may retain a grown backing on a reported post-reserve refusal;
    /// caller custody must still inventory it. Do not reserve accepted roots
    /// in place while publishing: reserve the candidate before atomic commit.
    pub fn try_reserve_on_control(
        &mut self,
        additional: usize,
        max_entries: usize,
    ) -> Result<(), Invalid> {
        let required = self
            .len()
            .checked_add(additional)
            .filter(|required| *required <= max_entries)
            .ok_or(Invalid("sorted entry count limit exceeded"))?;
        required
            .checked_mul(mem::size_of::<(K, V)>())
            .ok_or(Invalid("sorted entry backing size overflow"))?;
        self.entries
            .try_reserve_exact(additional)
            .map_err(|_| Invalid("could not reserve sorted entry backing"))?;
        // Report actual capacity, never assume the request is the allocation.
        self.backing_bytes()?;
        Ok(())
    }
}
impl<K: Ord, V> SortedEntries<K, V> {
    fn index(&self, key: &K) -> Result<usize, usize> {
        self.entries
            .binary_search_by(|(existing, _)| existing.cmp(key))
    }
    pub fn get(&self, key: &K) -> Option<&V> {
        self.index(key).ok().map(|index| &self.entries[index].1)
    }
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.index(key).ok().map(|index| &mut self.entries[index].1)
    }
    pub fn contains_key(&self, key: &K) -> bool {
        self.index(key).is_ok()
    }
    /// Existing-key replacement allocates no container backing. Refusal leaves
    /// values/ordering unchanged; input values retire on this control call.
    pub fn try_insert_on_control(
        &mut self,
        key: K,
        value: V,
        max_entries: usize,
    ) -> Result<Option<V>, Invalid> {
        match self.index(&key) {
            Ok(index) => Ok(Some(mem::replace(&mut self.entries[index].1, value))),
            Err(index) => {
                self.try_reserve_on_control(1, max_entries)?;
                self.entries.insert(index, (key, value));
                Ok(None)
            }
        }
    }
    pub fn remove(&mut self, key: &K) -> Option<V> {
        self.index(key)
            .ok()
            .map(|index| self.entries.remove(index).1)
    }
    /// Checked bulk construction: sort once without scratch allocation and
    /// refuse duplicates. Never build an intermediate std map or silently
    /// collapse duplicate canonical ownership keys.
    pub fn try_from_unsorted_on_control(
        mut entries: Vec<(K, V)>,
        max_entries: usize,
    ) -> Result<Self, Invalid> {
        if entries.len() > max_entries {
            return Err(Invalid("sorted entry count limit exceeded"));
        }
        entries
            .capacity()
            .checked_mul(mem::size_of::<(K, V)>())
            .ok_or(Invalid("sorted entry backing size overflow"))?;
        entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
        if entries.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(Invalid("duplicate sorted entry key"));
        }
        Ok(Self { entries })
    }
    /// Fallible backing reservation precedes caller-controlled child cloning.
    /// The caller quotes all old/new/backing/child overlap before calling.
    pub fn try_clone_with_on_control(
        &self,
        max_entries: usize,
        mut clone: impl FnMut(&K, &V) -> Result<(K, V), Invalid>,
    ) -> Result<Self, Invalid> {
        let mut result = Self::new();
        result.try_reserve_on_control(self.len(), max_entries)?;
        for (key, value) in &self.entries {
            result.entries.push(clone(key, value)?);
        }
        // A caller-supplied key clone must not change ordering or identity.
        if result
            .entries
            .iter()
            .zip(&self.entries)
            .any(|(new, old)| new.0 != old.0)
        {
            return Err(Invalid("sorted entry clone changed key"));
        }
        Ok(result)
    }
}
impl<K: Clone, V: Clone> Clone for SortedEntries<K, V> {
    /// Preserve existing canonical Project/control snapshot Clone behavior.
    /// This is control-only and grants no bounded peak/admission claim; new
    /// admitted preparation paths use the explicit fallible clone above.
    fn clone(&self) -> Self {
        Self {
            entries: self.entries.clone(),
        }
    }
}
impl<K, V> IntoIterator for SortedEntries<K, V> {
    type Item = (K, V);
    type IntoIter = std::vec::IntoIter<(K, V)>;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}
fn borrowed_pair<K, V>((key, value): &(K, V)) -> (&K, &V) {
    (key, value)
}
impl<'a, K, V> IntoIterator for &'a SortedEntries<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = std::iter::Map<std::slice::Iter<'a, (K, V)>, fn(&'a (K, V)) -> (&'a K, &'a V)>;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter().map(borrowed_pair::<K, V>)
    }
}

#[cfg(test)]
mod tests;
