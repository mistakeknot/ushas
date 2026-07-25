//! Minimal reproduction for shadow-work-shb1: does `addPresentedHandler` fire
//! for a `CAMetalLayer` driven entirely from Rust via objc2?
//!
//! Mirrors `scripts/present-probe-detached.swift`, which gets a callback for
//! every present (842/842) on a layer attached to no view and no window. If
//! this reports 0, the defect reproduces in ~80 lines with no Bevy, no wgpu and
//! no MetalFX involved, and can be bisected against the Swift version.
//!
//! Run: cargo run -p bevy_metalfx --features frame-interpolation --example present_repro

#[cfg(all(target_os = "macos", feature = "frame-interpolation"))]
fn main() {
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject};
    use objc2::{class, msg_send};
    use objc2_metal::{MTLCommandBuffer, MTLCommandQueue, MTLDevice, MTLDrawable};
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    #[link(name = "Metal", kind = "framework")]
    extern "C" {
        fn MTLCreateSystemDefaultDevice() -> *mut AnyObject;
    }

    let callbacks = Arc::new(AtomicU64::new(0));
    let presented = Arc::new(AtomicU64::new(0));
    let mut encoded = 0u64;

    unsafe {
        let device_raw = MTLCreateSystemDefaultDevice();
        assert!(!device_raw.is_null(), "no Metal device");
        let device: &ProtocolObject<dyn MTLDevice> = &*(device_raw as *const _);
        let queue = device.newCommandQueue().expect("no command queue");

        // Detached layer — the Swift control proves attachment is irrelevant.
        let layer: *mut AnyObject = msg_send![class!(CAMetalLayer), layer];
        let _: () = msg_send![layer, setDevice: device_raw];
        let _: () = msg_send![layer, setPixelFormat: 80usize]; // BGRA8Unorm
        let _: () = msg_send![layer, setFramebufferOnly: true];
        let _: () = msg_send![layer, setMaximumDrawableCount: 3usize];
        let size = (400.0f64, 300.0f64);
        #[repr(C)]
        struct CGSize {
            w: f64,
            h: f64,
        }
        unsafe impl objc2::Encode for CGSize {
            const ENCODING: objc2::Encoding = objc2::Encoding::Struct(
                "CGSize",
                &[objc2::Encoding::Double, objc2::Encoding::Double],
            );
        }
        let _: () = msg_send![layer, setDrawableSize: CGSize { w: size.0, h: size.1 }];

        // One shared block, exactly as present.rs now does.
        let cb = Arc::clone(&callbacks);
        let pr = Arc::clone(&presented);
        let block = RcBlock::new(move |d: NonNull<ProtocolObject<dyn MTLDrawable>>| {
            cb.fetch_add(1, Ordering::Relaxed);
            if d.as_ref().presentedTime() > 0.0 {
                pr.fetch_add(1, Ordering::Relaxed);
            }
        });
        let block_ptr = RcBlock::into_raw(block);

        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(6) {
            let raw: *mut AnyObject = msg_send![layer, nextDrawable];
            if raw.is_null() {
                continue;
            }
            let drawable: Retained<ProtocolObject<dyn MTLDrawable>> =
                match Retained::retain(raw.cast()) {
                    Some(d) => d,
                    None => continue,
                };
            drawable.addPresentedHandler(block_ptr as *mut _);

            let buf = queue.commandBuffer().expect("no command buffer");
            buf.presentDrawable(&drawable);
            buf.commit();
            encoded += 1;
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
        std::thread::sleep(std::time::Duration::from_millis(800));
    }

    println!(
        "RESULT-RUST encoded={} callbacks={} presented={}",
        encoded,
        callbacks.load(Ordering::Relaxed),
        presented.load(Ordering::Relaxed)
    );
}

#[cfg(not(all(target_os = "macos", feature = "frame-interpolation")))]
fn main() {
    println!("macOS + frame-interpolation only");
}
