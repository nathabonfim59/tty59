//! App-global cache for mermaid diagram renders.
//!
//! One [`MermaidCache`] lives in the gpui globals for the whole process. It
//! owns three structures:
//!
//! - `entries`: the render cache proper, keyed by the exact diagram source,
//!   the full [`MermaidTheme`] and the requested scale (as `f32` bits). Keys
//!   keep the source and theme themselves rather than a digest, so a hash
//!   collision can never hand one diagram another's image. Entries are
//!   [`EntryState::Pending`] while a background render runs, `Ready` with the
//!   `Arc<RenderImage>` and its logical size, or `Failed` with a one-line
//!   message.
//! - `last_good`: per `block_id`, the newest image that block successfully
//!   showed, with the theme and scale it was rendered at. While a block
//!   re-renders (typing, resize) it is shown again — but only when the
//!   pending render's theme and scale match its own, so a theme change never
//!   leaves the old palette on screen.
//! - `live_images`: per uploaded image, the windows that have drawn it. The
//!   sprite atlas is per window, so releasing an image means
//!   [`Window::drop_image`] on every recorded live window (a closed window is
//!   simply skipped). Records are pruned of closed windows whenever they are
//!   touched, and removed once no entry and no last-good reference to the
//!   image remains.
//!
//! # Capacity
//!
//! `entries` and `last_good` are each bounded by [`CAP_ENTRIES`] /
//! [`CAP_LAST_GOOD`], evicting least-recently-used first. Entry eviction is
//! never blocked by last-good references: evicting an entry only drops the
//! cache's `Arc`; the image (and its atlas tiles) survive while a last-good
//! still references it, and are released when the last reference goes away.
//! A fully saturated cache — every entry also retained by a last-good —
//! still admits new renders without exceeding the caps.
//!
//! # Threading and borrows
//!
//! Render work runs off the UI thread; completions carry the generation
//! they were scheduled with and are discarded if their entry has been
//! evicted or superseded. Methods that would need both the cache and `App`
//! are split in two: pure bookkeeping here, and the `Window`/`App` half —
//! spawning the task, dropping atlas tiles — in [`super`], which owns a
//! `&mut Window` during rendering.
//!
//! # Reads versus touches
//!
//! [`MermaidCache::peek`] and [`MermaidCache::last_good_for_block`] are
//! read-only on purpose: the preview builds every block of an
//! unvirtualized document every frame, and an offscreen block reading its
//! state must not look recently used — otherwise it would age the visible
//! diagrams' entries and last-goods out. Recency (and every state change)
//! is touched in [`super::VisibleBlock`]'s prepaint, only for blocks whose
//! bounds intersect the visible clip.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{AnyWindowHandle, Global, ImageId, RenderImage};
use tty59_mermaid::{MermaidError, MermaidTheme, Raster};

/// Rendered entries kept at once. Documents with more distinct diagrams age
/// the oldest out; re-requesting an evicted diagram simply re-renders it.
pub(super) const CAP_ENTRIES: usize = 64;
/// Blocks with a retained last-good image. Bounds the typing-flicker
/// guarantee the same way [`CAP_ENTRIES`] bounds the render cache.
pub(super) const CAP_LAST_GOOD: usize = 64;

/// The background render seam. Production runs [`tty59_mermaid::render`];
/// tests inject deterministic closures (see [`MermaidCache::new`]).
pub(super) type RenderFn =
    Arc<dyn Fn(&str, &MermaidTheme, f32) -> Result<Raster, MermaidError> + Send + Sync>;

/// The exact identity of one rendered diagram: its source, its theme and
/// the requested scale. Keeping the values (not digests of them) makes key
/// equality imply diagram equality.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct CacheKey {
    source: Arc<str>,
    theme: MermaidTheme,
    scale_bits: u32,
}

impl CacheKey {
    pub(super) fn new(source: &str, theme: MermaidTheme, scale: f32) -> Self {
        Self {
            source: Arc::from(source),
            theme,
            scale_bits: scale.to_bits(),
        }
    }

    /// The key's theme, for last-good matching and storage.
    pub(super) fn theme(&self) -> &MermaidTheme {
        &self.theme
    }

    /// The key's scale, reassembled from its bits.
    pub(super) fn scale(&self) -> f32 {
        f32::from_bits(self.scale_bits)
    }
}

/// A diagram's logical size in points — device pixels divided by the scale
/// the raster actually came back at, which is what layout must use.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LogicalSize {
    pub(crate) width: f32,
    pub(crate) height: f32,
}

/// What `render_block` should draw for a key, as decided by the cache.
#[derive(Clone)]
pub(super) enum Decision {
    /// Cache hit: show this image at this logical size.
    Ready(Arc<RenderImage>, LogicalSize),
    /// A render was already scheduled or is running.
    Pending,
    /// Rendering failed; show the fallback with this one-line message.
    Failed(String),
    /// Nothing cached and nothing scheduled yet. The caller decides when a
    /// block becomes visible enough to actually request a render
    /// ([`MermaidCache::request`]).
    Idle,
}

/// A render task scheduled by [`MermaidCache::request`], for [`super`] to
/// spawn on the executors — everything the completion needs to find its way
/// back to the cache.
pub(super) struct ScheduledRender {
    pub(super) key: CacheKey,
    pub(super) source: String,
    pub(super) theme: MermaidTheme,
    pub(super) scale: f32,
    pub(super) generation: u64,
    pub(super) renderer: RenderFn,
}

/// An image whose atlas tiles should be dropped on the listed windows.
/// Produced when the last reference to a drawn image goes away.
pub(super) struct Release {
    pub(super) image: Arc<RenderImage>,
    pub(super) windows: Vec<AnyWindowHandle>,
}

/// One cached render. `last_used` is a monotonic stamp from
/// [`MermaidCache::clock`], updated on every hit, driving LRU eviction.
struct Entry {
    state: EntryState,
    last_used: u64,
}

enum EntryState {
    Pending {
        generation: u64,
    },
    Ready {
        image: Arc<RenderImage>,
        logical: LogicalSize,
    },
    Failed {
        message: String,
    },
}

/// The newest image one block successfully showed, kept so edits to its
/// source keep showing a diagram instead of flickering to a placeholder.
struct LastGood {
    theme: MermaidTheme,
    image: Arc<RenderImage>,
    logical: LogicalSize,
    last_used: u64,
}

/// The windows that have drawn one retained image, for atlas release.
struct LiveImage {
    windows: Vec<AnyWindowHandle>,
}

impl LiveImage {
    fn record(&mut self, window: AnyWindowHandle, live: &[AnyWindowHandle]) {
        // Prune closed windows as we go so the record stays bounded across
        // repeated open/close of preview windows.
        self.windows.retain(|handle| live.contains(handle));
        if !self.windows.contains(&window) {
            self.windows.push(window);
        }
    }
}

/// The process-wide mermaid render state. Installed lazily as a gpui global
/// on first use; tests install their own instance with an injected
/// [`RenderFn`] before the first render.
pub(super) struct MermaidCache {
    entries: HashMap<CacheKey, Entry>,
    last_good: HashMap<u64, LastGood>,
    live_images: HashMap<ImageId, LiveImage>,
    clock: u64,
    next_generation: u64,
    renderer: RenderFn,
}

impl Global for MermaidCache {}

impl Default for MermaidCache {
    fn default() -> Self {
        Self::new(Arc::new(|source, theme, scale| {
            tty59_mermaid::render(source, theme, scale)
        }))
    }
}

impl MermaidCache {
    pub(super) fn new(renderer: RenderFn) -> Self {
        Self {
            entries: HashMap::new(),
            last_good: HashMap::new(),
            live_images: HashMap::new(),
            clock: 0,
            next_generation: 0,
            renderer,
        }
    }

    /// Look the key up without touching anything: no LRU stamp, no
    /// scheduling, no insertion. Element building uses this for every
    /// block — visible or not — so merely being part of an unvirtualized
    /// document can never make a diagram's entry look recently used. A
    /// miss returns [`Decision::Idle`]; the caller decides at visible
    /// prepaint whether to actually request a render ([`MermaidCache::request`]).
    pub(super) fn peek(&self, key: &CacheKey) -> Decision {
        match self.entries.get(key).map(|entry| &entry.state) {
            Some(EntryState::Pending { .. }) => Decision::Pending,
            Some(EntryState::Ready { image, logical }) => Decision::Ready(image.clone(), *logical),
            Some(EntryState::Failed { message }) => Decision::Failed(message.clone()),
            None => Decision::Idle,
        }
    }

    /// Refresh the entry's LRU stamp, if it still exists. Called when a
    /// block is actually displayed (visible prepaint), not when it is
    /// merely built.
    pub(super) fn touch(&mut self, key: &CacheKey) {
        self.clock = self.clock.saturating_add(1);
        if let Some(entry) = self.entries.get_mut(key) {
            entry.last_used = self.clock;
        }
    }

    /// Look the key up; on a miss insert `Pending` and hand back the render
    /// task for [`super`] to spawn. Hits refresh the entry's LRU stamp. The
    /// caller owns spawning so this stays borrow-clean against `App`.
    pub(super) fn request(&mut self, key: &CacheKey) -> (Decision, Option<ScheduledRender>) {
        self.clock = self.clock.saturating_add(1);
        if let Some(entry) = self.entries.get_mut(key) {
            entry.last_used = self.clock;
            let decision = match &entry.state {
                EntryState::Pending { .. } => Decision::Pending,
                EntryState::Ready { image, logical } => Decision::Ready(image.clone(), *logical),
                EntryState::Failed { message } => Decision::Failed(message.clone()),
            };
            return (decision, None);
        }

        let generation = self.next_generation;
        self.next_generation = self.next_generation.saturating_add(1);
        self.entries.insert(
            key.clone(),
            Entry {
                state: EntryState::Pending { generation },
                last_used: self.clock,
            },
        );
        let task = ScheduledRender {
            key: key.clone(),
            source: key.source.to_string(),
            theme: key.theme.clone(),
            scale: key.scale(),
            generation,
            renderer: self.renderer.clone(),
        };
        (Decision::Pending, Some(task))
    }

    /// Adopt a completed render, unless the entry it belongs to has since
    /// been evicted or superseded by a newer generation for the same key.
    /// Discarded results were never uploaded to any atlas, so they are
    /// simply dropped.
    pub(super) fn complete(
        &mut self,
        key: &CacheKey,
        generation: u64,
        raster: Result<Raster, MermaidError>,
    ) {
        let is_current_pending = match self.entries.get(key) {
            Some(Entry {
                state:
                    EntryState::Pending {
                        generation: current,
                    },
                ..
            }) => *current == generation,
            _ => false,
        };
        if !is_current_pending {
            return;
        }

        let state = match raster {
            Ok(raster) => match super::raster_to_image(raster) {
                Some((image, logical)) => EntryState::Ready { image, logical },
                // A malformed raster has no message worth showing beyond
                // the localized error label the renderer draws by itself.
                None => EntryState::Failed {
                    message: String::new(),
                },
            },
            Err(err) => EntryState::Failed {
                message: super::failure_message(&err.to_string()),
            },
        };
        if let Some(entry) = self.entries.get_mut(key) {
            entry.state = state;
            entry.last_used = self.clock;
        }
    }

    /// Record that `window` drew `image`, so a later eviction can release
    /// the image from that window's sprite atlas. `live` is the caller's
    /// snapshot of open windows, used to prune handles that have closed.
    pub(super) fn record_window(
        &mut self,
        image: &Arc<RenderImage>,
        window: AnyWindowHandle,
        live: &[AnyWindowHandle],
    ) {
        self.live_images
            .entry(image.id)
            .or_insert_with(LiveImage::empty)
            .record(window, live);
    }

    /// This block's last-good image, when it was rendered for the same
    /// theme. The scale is deliberately not matched: the editor's zoom
    /// changes the scale every diagram renders at, and flashing the
    /// placeholder on all of them for a zoom the eye is following would be
    /// worse than a briefly soft image — the caller draws the last-good at
    /// the new size until the new raster lands. A theme change still returns
    /// `None`: the stale palette must not stay on screen.
    /// Read-only: like [`MermaidCache::peek`], building an offscreen
    /// element must not refresh recency; a displayed last-good is touched
    /// separately ([`MermaidCache::touch_last_good`]).
    pub(super) fn last_good_for_block(
        &self,
        block_id: u64,
        theme: &MermaidTheme,
    ) -> Option<(Arc<RenderImage>, LogicalSize)> {
        let good = self.last_good.get(&block_id)?;
        if &good.theme != theme {
            return None;
        }
        Some((good.image.clone(), good.logical))
    }

    /// Refresh the block's last-good LRU stamp, if it still exists. Called
    /// when the last-good image is actually displayed (visible prepaint).
    pub(super) fn touch_last_good(&mut self, block_id: u64) {
        self.clock = self.clock.saturating_add(1);
        if let Some(good) = self.last_good.get_mut(&block_id) {
            good.last_used = self.clock;
        }
    }

    /// The height a placeholder box for this block should keep, so an
    /// unrenderable moment doesn't jump the layout: the block's own
    /// last-good height when there is one, `None` for the default.
    pub(super) fn last_good_height(&self, block_id: u64) -> Option<f32> {
        self.last_good
            .get(&block_id)
            .map(|good| good.logical.height)
    }

    /// Remember `image` as the newest successful render for `block_id`.
    ///
    /// Replacing an older last-good drops that reference: if nothing else
    /// retains the old image (its cache entry may already have been evicted),
    /// its atlas tiles must go too, so the replaced image comes back as a
    /// [`Release`] for the caller to execute.
    pub(super) fn store_last_good(
        &mut self,
        block_id: u64,
        theme: MermaidTheme,
        image: Arc<RenderImage>,
        logical: LogicalSize,
    ) -> Option<Release> {
        self.clock = self.clock.saturating_add(1);
        let replaced = self.last_good.insert(
            block_id,
            LastGood {
                theme,
                image,
                logical,
                last_used: self.clock,
            },
        );
        let old = replaced.map(|good| good.image)?;
        self.release_if_unreferenced(&old)
    }

    /// Evict past the caps and decide about atlas releases, pruning every
    /// record of closed windows along the way. Returns the images whose
    /// last reference went away, with the live windows that drew them; the
    /// caller performs the actual `drop_image` calls where a `&mut Window`
    /// exists.
    pub(super) fn collect_releases(&mut self, live: &[AnyWindowHandle]) -> Vec<Release> {
        let mut evicted = self.evict_over_capacity();
        for record in self.live_images.values_mut() {
            record.windows.retain(|handle| live.contains(handle));
        }

        let mut releases = Vec::new();
        evicted.drain(..).for_each(|image| {
            if let Some(release) = self.release_if_unreferenced(&image) {
                releases.push(release);
            }
        });
        releases
    }

    /// Bring both maps back under their caps, oldest first, returning the
    /// images that stopped being referenced. Never blocked by
    /// cross-references between the two maps: an entry evicted here may keep
    /// its image alive through a last-good, and vice versa.
    fn evict_over_capacity(&mut self) -> Vec<Arc<RenderImage>> {
        let mut released = Vec::new();

        while self.entries.len() > CAP_ENTRIES {
            let victim = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone());
            let Some(victim) = victim else { break };
            if let Some(Entry {
                state: EntryState::Ready { image, .. },
                ..
            }) = self.entries.remove(&victim)
            {
                released.push(image);
            }
        }

        while self.last_good.len() > CAP_LAST_GOOD {
            let victim = self
                .last_good
                .iter()
                .min_by_key(|(_, good)| good.last_used)
                .map(|(block, _)| *block);
            let Some(victim) = victim else { break };
            if let Some(good) = self.last_good.remove(&victim) {
                released.push(good.image);
            }
        }

        released
    }

    /// If neither the entries nor the last-good map references `image`
    /// anymore, take its live-window record and hand it back as a
    /// [`Release`]. While any reference remains, the image stays live
    /// exactly as if it had not been evicted — capacity is enforced on the
    /// maps, not on the images.
    fn release_if_unreferenced(&mut self, image: &Arc<RenderImage>) -> Option<Release> {
        let still_retained = self.entries.values().any(|entry| {
            matches!(&entry.state, EntryState::Ready { image: kept, .. } if Arc::ptr_eq(kept, image))
        }) || self
            .last_good
            .values()
            .any(|good| Arc::ptr_eq(&good.image, image));
        if still_retained {
            return None;
        }

        // Never drawn: nothing was ever uploaded to any atlas.
        self.live_images.remove(&image.id).map(|record| Release {
            image: image.clone(),
            windows: record.windows,
        })
    }

    /// Number of cached entries.
    #[cfg(test)]
    pub(super) fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Number of retained last-good images.
    #[cfg(test)]
    pub(super) fn last_good_count(&self) -> usize {
        self.last_good.len()
    }

    /// The state of one key: "pending" / "ready" / "failed", or `None` when
    /// the key is not cached.
    #[cfg(test)]
    pub(super) fn state_of(&self, key: &CacheKey) -> Option<&'static str> {
        self.entries.get(key).map(|entry| match &entry.state {
            EntryState::Pending { .. } => "pending",
            EntryState::Ready { .. } => "ready",
            EntryState::Failed { .. } => "failed",
        })
    }

    /// The ready image for one key, cloned.
    #[cfg(test)]
    pub(super) fn ready_image(&self, key: &CacheKey) -> Option<Arc<RenderImage>> {
        match self.entries.get(key).map(|entry| &entry.state) {
            Some(EntryState::Ready { image, .. }) => Some(image.clone()),
            _ => None,
        }
    }

    /// Whether `image` still has a live-image record.
    #[cfg(test)]
    pub(super) fn has_live_record(&self, image: &Arc<RenderImage>) -> bool {
        self.live_images.contains_key(&image.id)
    }

    /// The windows recorded for one image.
    #[cfg(test)]
    pub(super) fn recorded_windows(&self, image: &Arc<RenderImage>) -> Vec<AnyWindowHandle> {
        self.live_images
            .get(&image.id)
            .map(|record| record.windows.clone())
            .unwrap_or_default()
    }
}

impl LiveImage {
    fn empty() -> Self {
        Self {
            windows: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::App;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn solid_raster(width: u32, height: u32, scale: f32) -> Raster {
        Raster {
            width,
            height,
            scale,
            bgra: vec![0; width as usize * height as usize * 4],
        }
    }

    fn key_for(source: &str) -> CacheKey {
        CacheKey::new(source, MermaidTheme::light(), 1.0)
    }

    fn ok_renderer() -> RenderFn {
        Arc::new(|_: &str, _: &MermaidTheme, _: f32| Ok(solid_raster(2, 2, 1.0)))
    }

    /// The full request pipeline as `render_block` runs it: decide, then
    /// spawn the returned task on the executors.
    fn request(cx: &mut App, key: &CacheKey) -> Decision {
        let (decision, task) = cx.global_mut::<MermaidCache>().request(key);
        if let Some(task) = task {
            super::super::spawn_render(task, cx);
        }
        decision
    }

    /// A miss schedules exactly one render; further requests for the same
    /// key ride the same entry. The call count is only settled after the
    /// executors park.
    #[gpui::test]
    fn a_miss_schedules_exactly_one_render(cx: &mut gpui::TestAppContext) {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        cx.update(|cx| {
            cx.set_global(MermaidCache::new(Arc::new(move |_, _, _| {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(solid_raster(2, 2, 1.0))
            })));
        });
        let key = key_for("graph TD\nA-->B");
        cx.update(|cx| {
            assert!(matches!(request(cx, &key), Decision::Pending));
            assert!(matches!(request(cx, &key), Decision::Pending));
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let cache = cx.global::<MermaidCache>();
            assert_eq!(cache.state_of(&key), Some("ready"));
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        });
    }

    /// Theme, scale and source each change the key, so each variation is
    /// rendered and cached independently.
    #[test]
    fn theme_scale_and_source_all_change_the_key() {
        let base = CacheKey::new("graph TD\nA-->B", MermaidTheme::light(), 1.0);
        assert_ne!(
            base,
            CacheKey::new("graph TD\nA-->C", MermaidTheme::light(), 1.0)
        );
        assert_ne!(
            base,
            CacheKey::new("graph TD\nA-->B", MermaidTheme::dark(), 1.0)
        );
        assert_ne!(
            base,
            CacheKey::new("graph TD\nA-->B", MermaidTheme::light(), 2.0)
        );
    }

    /// Accents are part of the key's theme by value, so a palette change
    /// alone — the colors toggle flipping between empty and six accents —
    /// is a new diagram identity: its own entry, and no match for another
    /// palette's last-good.
    #[test]
    fn mermaid_accents_alone_change_the_cache_key() {
        let neutral = MermaidTheme::light();
        assert!(neutral.accents.is_empty());
        let mut colored = neutral.clone();
        colored.accents = vec![tty59_mermaid::Accent {
            fill: tty59_mermaid::Rgba::rgb(0xd9, 0xda, 0xdc),
            border: tty59_mermaid::Rgba::rgb(0x05, 0x08, 0x10),
            text: tty59_mermaid::Rgba::rgb(0xb0, 0x1a, 0x2c),
        }];
        assert_ne!(
            CacheKey::new("graph TD\nA-->B", neutral.clone(), 1.0),
            CacheKey::new("graph TD\nA-->B", colored.clone(), 1.0),
            "an accent-only change must rekey the cache"
        );
        assert_eq!(
            CacheKey::new("graph TD\nA-->B", colored.clone(), 1.0),
            CacheKey::new("graph TD\nA-->B", colored, 1.0),
            "equal accents keep equal keys"
        );
    }

    /// A completion whose entry aged out stays out; a completion from an
    /// older generation must not overwrite a newer pending render of the
    /// same key.
    #[gpui::test]
    fn stale_completions_cannot_resurrect_or_overwrite(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(MermaidCache::new(ok_renderer())));
        let key = key_for("graph TD\nA-->B");

        // First render completes…
        cx.update(|cx| {
            request(cx, &key);
        });
        cx.run_until_parked();

        // …and ages out for real: the cache is filled past its cap with
        // newer keys, so the eviction path removes our entry.
        cx.update(|cx| {
            for i in 0..CAP_ENTRIES {
                request(
                    cx,
                    &CacheKey::new(&format!("graph newer {i}"), MermaidTheme::light(), 1.0),
                );
            }
            let cache = cx.global_mut::<MermaidCache>();
            cache.collect_releases(&[]);
            assert_eq!(
                cache.state_of(&key),
                None,
                "the oldest entry must age out once the cache is over capacity"
            );

            // A late completion for the evicted entry must not resurrect it.
            cache.complete(&key, 0, Ok(solid_raster(2, 2, 1.0)));
            assert_eq!(cache.state_of(&key), None);
        });

        // Re-request: a fresh generation is pending; the old generation's
        // result arriving again must not satisfy or corrupt it.
        cx.update(|cx| {
            request(cx, &key);
        });
        cx.update(|cx| {
            let cache = cx.global_mut::<MermaidCache>();
            cache.complete(&key, 0, Ok(solid_raster(2, 2, 1.0)));
            assert_eq!(
                cache.state_of(&key),
                Some("pending"),
                "an old generation must not complete a newer pending render"
            );
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let cache = cx.global::<MermaidCache>();
            assert_eq!(
                cache.state_of(&key),
                Some("ready"),
                "the current generation's own completion still lands"
            );
        });
    }

    /// Saturation: [`CAP_ENTRIES`] ready entries all also referenced by
    /// last-goods still evict the oldest entry for a new key, and the
    /// evicted image is *not* released while its last-good holds it. The
    /// release only happens once the last-good goes too.
    #[gpui::test]
    fn saturation_enforces_caps_and_defers_release(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(MermaidCache::new(ok_renderer())));

        let keys: Vec<CacheKey> = (0..CAP_ENTRIES)
            .map(|i| CacheKey::new(&format!("graph {i}"), MermaidTheme::light(), 1.0))
            .collect();
        cx.update(|cx| {
            for key in &keys {
                request(cx, key);
            }
        });
        cx.run_until_parked();

        // Give every block a last-good referencing the cached image.
        let oldest_image = cx.update(|cx| {
            let mut oldest = None;
            let cache = cx.global_mut::<MermaidCache>();
            for (i, key) in keys.iter().enumerate() {
                let Decision::Ready(image, logical) = cache.request(key).0 else {
                    panic!("key {i} should be ready after parking");
                };
                assert!(
                    cache
                        .store_last_good(i as u64, key.theme().clone(), image.clone(), logical)
                        .is_none(),
                    "nothing is replaced the first time"
                );
                if i == 0 {
                    oldest = Some(image);
                }
            }
            assert_eq!(cache.entry_count(), CAP_ENTRIES);
            assert_eq!(cache.last_good_count(), CAP_LAST_GOOD);
            oldest.expect("block 0 stored")
        });

        // One more distinct diagram ages the oldest entry out even though
        // its image is still held by a last-good; the run through the
        // render-bound path produces no atlas release for it.
        cx.update(|cx| {
            request(
                cx,
                &CacheKey::new("graph fresh", MermaidTheme::light(), 1.0),
            );
        });
        cx.run_until_parked();
        let oldest_image = cx.update(|cx| {
            let cache = cx.global_mut::<MermaidCache>();
            // Give the still-held image a drawing record so its eventual
            // release is observable.
            cache.record_window(&oldest_image, fake_handle(9), &[fake_handle(9)]);
            let releases = cache.collect_releases(&[fake_handle(9)]);
            assert!(
                releases.is_empty(),
                "images still held by a last-good are not released"
            );
            assert_eq!(cache.entry_count(), CAP_ENTRIES);
            assert_eq!(cache.state_of(&keys[0]), None, "the oldest entry aged out");
            assert_eq!(
                cache.ready_image(&keys[0]),
                None,
                "the entry no longer pins the image"
            );
            oldest_image
        });

        // Aging the oldest last-good out (CAP_LAST_GOOD newer blocks) leaves
        // its image referenced by nothing — it must come back as a release
        // carrying the window that drew it.
        cx.update(|cx| {
            for i in 0..CAP_LAST_GOOD {
                let key =
                    CacheKey::new(&format!("graph newer good {i}"), MermaidTheme::light(), 1.0);
                request(cx, &key);
            }
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let cache = cx.global_mut::<MermaidCache>();
            for i in 0..CAP_LAST_GOOD {
                let key =
                    CacheKey::new(&format!("graph newer good {i}"), MermaidTheme::light(), 1.0);
                let Decision::Ready(image, logical) = cache.request(&key).0 else {
                    panic!("filler {i} should be ready");
                };
                cache.store_last_good(100 + i as u64, key.theme().clone(), image, logical);
            }
            assert_eq!(cache.last_good_count(), CAP_LAST_GOOD * 2);
            let releases = cache.collect_releases(&[fake_handle(9)]);
            assert_eq!(
                cache.last_good_count(),
                CAP_LAST_GOOD,
                "the overflow aged the oldest last-goods out"
            );
            assert!(
                releases
                    .iter()
                    .any(|release| Arc::ptr_eq(&release.image, &oldest_image)),
                "the aged-out last-good's image must be released"
            );
            assert!(!cache.has_live_record(&oldest_image));
        });
    }

    /// Replacing a last-good releases the old image exactly when nothing
    /// else retains it: not while its cache entry holds the image, not
    /// while the last-good slot itself does — but immediately when a newer
    /// last-good replaces an image whose entry already aged out.
    #[gpui::test]
    fn replacing_a_last_good_releases_an_unretained_image(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(MermaidCache::new(ok_renderer())));
        let first = key_for("graph TD\nA-->B");
        let second = key_for("graph TD\nA-->C");
        cx.update(|cx| {
            request(cx, &first);
            request(cx, &second);
        });
        cx.run_until_parked();

        let first_image = cx.update(|cx| {
            let cache = cx.global_mut::<MermaidCache>();
            let Decision::Ready(first_image, first_logical) = cache.request(&first).0 else {
                panic!("first should be ready");
            };
            let handle = fake_handle(1);
            cache.record_window(&first_image, handle, &[handle]);

            // Block 7 shows `first`…
            assert!(
                cache
                    .store_last_good(7, first.theme().clone(), first_image.clone(), first_logical)
                    .is_none()
            );
            // …and re-storing it while its entry still holds the image
            // releases nothing.
            assert!(
                cache
                    .store_last_good(7, first.theme().clone(), first_image.clone(), first_logical)
                    .is_none(),
                "the entry still retains the replaced image"
            );
            // Keep `second` newer than `first` so the overflow below evicts
            // exactly `first`'s entry.
            let _ = cache.request(&second);
            first_image
        });

        // Overflow the cache with distinct keys so `first`'s entry really
        // ages out (tasks are dropped unspawned: the entry state machine is
        // what matters here, not the executor). The image survives — its
        // last-good still holds it.
        cx.update(|cx| {
            let cache = cx.global_mut::<MermaidCache>();
            for i in 0..CAP_ENTRIES - 1 {
                let key = CacheKey::new(&format!("graph fill {i}"), MermaidTheme::light(), 1.0);
                let (_, task) = cache.request(&key);
                drop(task);
            }
            let releases = cache.collect_releases(&[fake_handle(1)]);
            let cache = cx.global::<MermaidCache>();
            assert_eq!(cache.state_of(&first), None, "the entry aged out");
            assert!(
                releases.is_empty(),
                "the last-good still retains the image, so nothing is released"
            );
        });

        // Now replace block 7's last-good: with the entry gone, the old
        // image is referenced by nothing and must come back as a release
        // naming the window that drew it.
        cx.update(|cx| {
            let cache = cx.global_mut::<MermaidCache>();
            let Decision::Ready(second_image, second_logical) = cache.request(&second).0 else {
                panic!("second should be ready");
            };
            let release = cache
                .store_last_good(7, second.theme().clone(), second_image, second_logical)
                .expect("the aged-out image is no longer retained anywhere");
            assert!(Arc::ptr_eq(&release.image, &first_image));
            assert_eq!(release.windows, vec![fake_handle(1)]);
            assert!(!cx.global::<MermaidCache>().has_live_record(&release.image));
        });
    }

    /// A stand-in root view so test-only window handles can be built
    /// (`WindowHandle::new` requires a `Render` root type).
    struct FakeRoot;

    impl gpui::Render for FakeRoot {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            gpui::div()
        }
    }

    fn fake_handle(id: u64) -> AnyWindowHandle {
        *gpui::WindowHandle::<FakeRoot>::new(gpui::WindowId::from(id))
    }

    /// Last-good is per block and per theme/scale: another block never
    /// borrows it, and a theme change refuses it.
    #[gpui::test]
    fn last_good_is_isolated_per_block_and_theme(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(MermaidCache::new(ok_renderer())));
        let key = key_for("graph TD\nA-->B");
        cx.update(|cx| {
            request(cx, &key);
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let cache = cx.global_mut::<MermaidCache>();
            let Decision::Ready(image, logical) = cache.request(&key).0 else {
                panic!("should be ready");
            };
            cache.store_last_good(7, key.theme().clone(), image, logical);

            // Same block, same theme, new source: eligible.
            assert!(cache.last_good_for_block(7, key.theme()).is_some());
            // Same block, different theme: refused — the stale palette must
            // not stay on screen.
            assert!(
                cache
                    .last_good_for_block(7, &MermaidTheme::dark())
                    .is_none()
            );
            // Different block: never borrowed.
            assert!(cache.last_good_for_block(8, key.theme()).is_none());
        });
    }
}
