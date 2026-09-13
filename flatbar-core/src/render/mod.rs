//! Rendering infrastructure: cosmic-text shaping, glyph caching, damage tracking, and software rasterization.

pub mod buffer;
pub mod damage;
pub mod glyphs;
pub mod icons;
pub mod layout;
pub mod snapshot;
pub mod text;
pub mod text_cache;

pub use damage::DamageTracker;
pub use glyphs::render_layout;
pub use icons::resolve_icon;
pub use layout::{compute_layout, LayoutResult, Rect, SpanPlacement, WidgetPlacement};
pub use snapshot::render_snapshot;
pub use text::{TextRenderParams, TextRenderer};
pub use text_cache::{CachedGlyph, ShapedRun, ShapingCache, TextCacheKey};
