//! Bounded, image-scoped diagnostics for the native Stage hardware regression.
use super::VulkanTexture;
use std::cell::Cell;

thread_local! {
    static IMAGE: Cell<Option<ash::vk::Image>> = const { Cell::new(None) };
}

pub(crate) fn is_traced(texture: &VulkanTexture) -> bool {
    IMAGE.with(|image| image.get() == Some(texture.image()))
}

pub(crate) fn trace_texture_for_test<T>(texture: &VulkanTexture, run: impl FnOnce() -> T) -> T {
    struct Restore(Option<ash::vk::Image>);
    impl Drop for Restore {
        fn drop(&mut self) {
            IMAGE.with(|image| image.set(self.0));
        }
    }
    let _restore = Restore(IMAGE.with(|image| image.replace(Some(texture.image()))));
    run()
}
