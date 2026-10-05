use super::super::super::resources::EffectResourcePool;
use crate::egl_renderer::effects::{EffectTextureFilter, EffectTextureFormat, EffectTextureKey};
use oblivion_one::effects::EffectWorkingSpace;

#[test]
fn nested_graph_leases_keep_outer_texture_checked_out_until_outer_release() {
    let mut pool = EffectResourcePool::new();
    let key = EffectTextureKey::new(
        32,
        32,
        EffectTextureFormat::Rgba8,
        EffectTextureFilter::Linear,
        EffectWorkingSpace::LinearSrgb,
    );

    let outer = pool.checkout(key).expect("outer graph lease");
    let nested = pool.checkout(key).expect("nested graph lease");
    assert_ne!(outer.id, nested.id, "live graphs cannot alias one texture");
    assert!(pool.is_checked_out(outer.id));
    assert!(pool.is_checked_out(nested.id));
    assert_eq!(pool.metrics().checked_out_texture_count, 2);

    pool.return_texture(nested.clone())
        .expect("nested graph releases its own lease");
    assert!(pool.is_checked_out(outer.id));
    assert!(!pool.is_checked_out(nested.id));

    let nested_reuse = pool
        .checkout(key)
        .expect("nested graph may reuse its returned lease");
    assert_eq!(nested_reuse.id, nested.id);
    assert_ne!(nested_reuse.id, outer.id);
    assert!(pool.is_checked_out(outer.id));
    assert!(pool.is_checked_out(nested_reuse.id));

    pool.return_texture(nested_reuse)
        .expect("nested graph releases its reused lease");
    assert!(pool.is_checked_out(outer.id));
    pool.return_texture(outer)
        .expect("outer graph releases its own lease");
    assert_eq!(pool.metrics().checked_out_texture_count, 0);
}
