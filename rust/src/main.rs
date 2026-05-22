#[link(name = "mpv", kind = "dylib")]
extern "C" {
    fn mpv_client_api_version() -> std::os::raw::c_ulong;
}

fn main() {
    let v = unsafe { mpv_client_api_version() };
    println!("tvplayer 0.1.0 (libmpv client api {:#x})", v);
}
