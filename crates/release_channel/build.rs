fn main() {
    const REVISION: &str = include_str!("WU_PLUS_REVISION");
    println!("cargo::rerun-if-changed=WU_PLUS_REVISION");
    let revision = REVISION.trim();
    assert!(
        revision
            .parse::<u64>()
            .is_ok_and(|value| value > 0 && value.to_string() == revision),
        "WU_PLUS_REVISION must contain a positive decimal integer"
    );
    println!("cargo::rustc-env=WU_PLUS_REVISION={revision}");

    println!("cargo::rustc-check-cfg=cfg(__do_not_set_zed_release_channel)");
    println!("cargo::rerun-if-env-changed=ZED_RELEASE_CHANNEL");

    // Enabling this `cfg` will cause a runtime panic if the
    // `ZED_RELEASE_CHANNEL` env var is not also set. TLDR: don't set the `cfg`
    // directly, just set the env var (hence the name).
    if std::env::var("ZED_RELEASE_CHANNEL").is_ok() {
        println!("cargo::rustc-cfg=__do_not_set_zed_release_channel");
    }
}
