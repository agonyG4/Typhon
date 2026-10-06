use super::*;

use oblivion_one::compositor::{DecorationRect, DecorationRenderPrimitive};

#[test]
fn decoration_requirements_include_solid_resources() {
    let primitives = [DecorationRenderPrimitive::SolidRect {
        rect: DecorationRect {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
        },
        color: [51, 51, 51, 255],
    }];

    let required = test_decoration_resource_requirements([primitives.as_slice()]);

    assert!(
        required
            .required
            .contains(&DecorationResourceKey::Solid(0xff33_3333))
    );
    assert!(required.required_assets.is_empty());
}
