use once_cell::sync::Lazy;

static LOGGER_INIT: Lazy<()> = Lazy::new(|| {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Debug)
            .with_tag("Bilin-JNI"),
    );
});

pub(crate) fn init() {
    Lazy::force(&LOGGER_INIT);
}
