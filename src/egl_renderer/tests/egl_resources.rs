use super::*;

#[derive(Clone)]
pub(super) struct DropProbe(pub(super) std::rc::Rc<std::cell::Cell<usize>>);

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.set(self.0.get().saturating_add(1));
    }
}

pub(super) fn fake_egl_image() -> egl::Image {
    // SAFETY: the fake handle is only passed to the test cleanup probe;
    // it is never sent to EGL.
    unsafe { egl::Image::from_ptr(std::ptr::NonNull::<c_void>::dangling().as_ptr()) }
}

#[test]
fn failed_image_creation_has_no_cleanup_owner() {
    let image_cleanup_count = std::rc::Rc::new(std::cell::Cell::new(0));
    let image_result: Result<egl::Image, ()> = Err(());

    if let Ok(image) = image_result {
        let cleanup_count = std::rc::Rc::clone(&image_cleanup_count);
        let _guard = EglImageGuard::new(image, move |_| {
            cleanup_count.set(cleanup_count.get() + 1);
        });
    }

    assert_eq!(image_cleanup_count.get(), 0);
}

#[test]
fn texture_creation_failure_destroys_acquired_image_once() {
    let image_cleanup_count = std::rc::Rc::new(std::cell::Cell::new(0));
    let texture_cleanup_count = std::rc::Rc::new(std::cell::Cell::new(0));
    {
        let cleanup_count = std::rc::Rc::clone(&image_cleanup_count);
        let _guard = EglImageGuard::new(fake_egl_image(), move |_| {
            cleanup_count.set(cleanup_count.get() + 1);
        });
        let _texture: Result<(), ()> = Err(());
        assert!(_texture.is_err());
    }

    assert_eq!(image_cleanup_count.get(), 1);
    assert_eq!(texture_cleanup_count.get(), 0);
}

#[test]
fn binding_failure_deletes_texture_and_destroys_image_once() {
    let image_cleanup_count = std::rc::Rc::new(std::cell::Cell::new(0));
    let texture_cleanup_count = std::rc::Rc::new(std::cell::Cell::new(0));
    {
        let cleanup_count = std::rc::Rc::clone(&image_cleanup_count);
        let _guard = EglImageGuard::new(fake_egl_image(), move |_| {
            cleanup_count.set(cleanup_count.get() + 1);
        });
        texture_cleanup_count.set(texture_cleanup_count.get() + 1);
    }

    assert_eq!(image_cleanup_count.get(), 1);
    assert_eq!(texture_cleanup_count.get(), 1);
}

#[test]
fn successful_construction_transfers_image_without_double_cleanup() {
    let image_cleanup_count = std::rc::Rc::new(std::cell::Cell::new(0));
    let texture_cleanup_count = std::rc::Rc::new(std::cell::Cell::new(0));
    let image = {
        let cleanup_count = std::rc::Rc::clone(&image_cleanup_count);
        let guard = EglImageGuard::new(fake_egl_image(), move |_| {
            cleanup_count.set(cleanup_count.get() + 1);
        });
        guard.disarm()
    };

    texture_cleanup_count.set(texture_cleanup_count.get() + 1);
    image_cleanup_count.set(image_cleanup_count.get() + 1);
    let _ = image;

    assert_eq!(image_cleanup_count.get(), 1);
    assert_eq!(texture_cleanup_count.get(), 1);
}
