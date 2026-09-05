use std::sync::atomic::{AtomicBool, Ordering};
static STOP: AtomicBool = AtomicBool::new(false);
pub fn request_stop() {
    STOP.store(true, Ordering::Relaxed);
}
pub fn stop_requested() -> bool {
    STOP.load(Ordering::Relaxed)
}
extern "C" fn signal_stop(_: libc::c_int) {
    request_stop();
}
pub fn install() {
    unsafe {
        libc::signal(libc::SIGINT, signal_stop as *const () as libc::sighandler_t);
        libc::signal(
            libc::SIGTERM,
            signal_stop as *const () as libc::sighandler_t,
        );
    }
}
