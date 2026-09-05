//! Image-target captures have no swapchain, display, or presentation semantics.
use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::render::render_resource::{TextureFormat, TextureUsages};
use bevy::render::view::screenshot::Screenshot;

#[derive(Resource, Default)]
pub enum CaptureTarget {
    #[default]
    Window,
    Image(Handle<Image>),
}

impl CaptureTarget {
    pub fn screenshot(&self) -> Screenshot {
        match self {
            Self::Window => Screenshot::primary_window(),
            Self::Image(image) => Screenshot::image(image.clone()),
        }
    }

    pub fn image_render_target(&self) -> Option<RenderTarget> {
        match self {
            Self::Window => None,
            Self::Image(image) => Some(image.clone().into()),
        }
    }
}

pub fn render_image(width: u32, height: u32) -> Image {
    let mut image = Image::new_target_texture(width, height, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_retains_render_and_readback_usages() {
        let image = render_image(640, 360);
        let descriptor = &image.texture_descriptor;
        assert_eq!((descriptor.size.width, descriptor.size.height), (640, 360));
        assert_eq!(descriptor.format, TextureFormat::Rgba8UnormSrgb);
        assert!(descriptor.usage.contains(
            TextureUsages::RENDER_ATTACHMENT
                | TextureUsages::TEXTURE_BINDING
                | TextureUsages::COPY_SRC
        ));
    }

    #[test]
    fn capture_and_camera_share_the_same_image() {
        let mut images = Assets::<Image>::default();
        let handle = images.add(render_image(32, 32));
        let target = CaptureTarget::Image(handle.clone());
        assert_eq!(target.screenshot().0.as_image(), Some(&handle));
        assert_eq!(
            target.image_render_target().unwrap().as_image(),
            Some(&handle)
        );
        assert!(matches!(
            CaptureTarget::Window.screenshot().0,
            RenderTarget::Window(_)
        ));
        assert!(CaptureTarget::Window.image_render_target().is_none());
    }
}
