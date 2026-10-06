#[test]
fn argb_pixels_pack_to_rgba_without_changing_channel_order() {
    let mut packed = Vec::new();

    super::super::image::pack_argb_pixels_rgba(&[0x1122_3344, 0xaa55_6677], &mut packed);

    assert_eq!(packed, vec![0x22, 0x33, 0x44, 0x11, 0x55, 0x66, 0x77, 0xaa]);
}
