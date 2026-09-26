//! Smoke tests that use rust-sak the way a downstream crate does.
//!
//! Everything else in the suite is an in-crate `#[cfg(test)]` module, which can reach private items and cannot tell
//! whether a name is actually exported. These can only see the public API, so they are what catches a re-export that
//! was dropped, a type that stopped being nameable from outside, or a feature that does not compile on its own.
//!
//! Each block is gated on its own feature, so this file is meaningful under any feature set — including none.

#[cfg(feature = "crypto")]
mod crypto {
    use rust_sak::crypto::{sha256_bytes, sha256_file, sha256_string, xxh3_bytes, xxh3_file, xxh3_string};

    #[test]
    fn hashes_are_reachable_and_agree_across_input_kinds() {
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(sha256_string("abc"), expected);
        assert_eq!(sha256_bytes(b"abc"), expected);
        assert_eq!(xxh3_bytes(b"abc"), xxh3_string("abc"));

        let file = std::env::temp_dir().join(format!("rust_sak_api_crypto_{}", std::process::id()));
        std::fs::write(&file, b"abc").unwrap();
        assert_eq!(sha256_file(&file).unwrap(), expected);
        assert_eq!(xxh3_file(&file).unwrap(), xxh3_string("abc"));
        std::fs::remove_file(&file).unwrap();
    }
}

#[cfg(feature = "fs")]
mod fs {
    use rust_sak::fs::{
        ArchiveFormat, CopyOptions, ExtractOptions, ExtractProgress, FsError, ListOptions, copy_files, file_exists,
        list_path, mk_temp_dir, move_files, user_config_dir,
    };

    #[test]
    fn the_filesystem_helpers_round_trip() {
        let source = mk_temp_dir("api-source").unwrap();
        std::fs::write(source.path().join("keep.txt"), b"hello").unwrap();
        std::fs::write(source.path().join("drop.log"), b"noise").unwrap();
        assert!(file_exists(source.path().join("keep.txt")));

        let wanted = list_path(source.path(), &ListOptions::new().extension("txt")).unwrap();
        assert_eq!(wanted.len(), 1);

        let copied = mk_temp_dir("api-copied").unwrap();
        assert_eq!(
            copy_files(&wanted, copied.path(), &CopyOptions::new()).unwrap().files,
            1
        );

        let moved = mk_temp_dir("api-moved").unwrap();
        assert_eq!(move_files(wanted, moved.path(), &CopyOptions::new()).unwrap().files, 1);
    }

    #[test]
    fn the_hardening_errors_are_nameable_from_outside() {
        // `IllegalPath` carrying its reason is part of the contract, not an internal detail.
        let err = user_config_dir("my-app", "../../escape").unwrap_err();
        assert!(matches!(err, FsError::IllegalPath { .. }), "got {err:?}");

        assert_eq!(ArchiveFormat::from_path("photos.zip"), Some(ArchiveFormat::Zip));
        assert_eq!(ArchiveFormat::from_path("backup.TAR.XZ"), Some(ArchiveFormat::TarXz));
        assert_eq!(ArchiveFormat::from_path("notes.txt"), None);

        // The options builder is consuming, so this also checks it is usable without naming private types.
        let _ = ExtractOptions::new().max_entries(10).symlinks(false).file_mode(0o644);

        // The progress hook's argument has to be nameable and readable from outside the crate, or a caller cannot
        // write the callback at all.
        let _ = ExtractOptions::new().on_progress(|progress: &ExtractProgress| {
            let _ = (
                progress.entries,
                progress.bytes,
                progress.total_entries,
                progress.total_bytes,
            );
        });
    }
}

#[cfg(feature = "github")]
mod github {
    use rust_sak::github::{GithubError, Release, get_latest_release, is_outdated_release};

    #[test]
    fn the_release_api_is_nameable() {
        // Type-level only: awaiting these would reach the network.
        drop(get_latest_release("owner", "repo"));
        drop(is_outdated_release("owner", "repo", "1.0.0"));

        let release = Release {
            tag_name: "v1.2.0".into(),
            name: None,
            html_url: String::new(),
            published_at: None,
            prerelease: false,
            draft: false,
        };
        assert_eq!(release.tag_name, "v1.2.0");

        let _: fn(GithubError) -> String = |error| error.to_string();
    }
}

#[cfg(feature = "image")]
mod image {
    use rust_sak::image::{
        EncodeOptions, ImageError, ImageFormat, decode_bytes, encode_writer, format_from_bytes, probe_bytes, rotate,
    };

    #[test]
    fn a_png_round_trips_through_the_public_api() {
        let original = image::DynamicImage::ImageRgba8(image::RgbaImage::new(4, 4));

        let mut bytes = Vec::new();
        encode_writer(&original, &mut bytes, ImageFormat::Png, None).unwrap();

        assert_eq!(format_from_bytes(&bytes).unwrap(), ImageFormat::Png);
        assert_eq!(probe_bytes(&bytes).unwrap().width, 4);
        assert_eq!(decode_bytes(&bytes).unwrap().width(), 4);

        // A mismatch between the named format and the options is a public, matchable error.
        let err = encode_writer(
            &original,
            &mut Vec::new(),
            ImageFormat::Png,
            Some(EncodeOptions::Jpeg { quality: 80 }),
        )
        .unwrap_err();
        assert!(matches!(err, ImageError::FormatMismatch { .. }), "got {err:?}");
    }

    #[test]
    fn rotate_is_reachable_and_expands_the_canvas() {
        let original = image::DynamicImage::ImageRgb8(image::RgbImage::new(240, 120));
        let rotated = rotate(&original, 30.0);

        assert_eq!((rotated.width(), rotated.height()), (268, 224));
        // Rotating always yields alpha, so a downstream caller can name the promoted type.
        assert!(matches!(rotated, image::DynamicImage::ImageRgba8(_)));
    }
}

#[cfg(feature = "memo")]
mod memo {
    use std::time::Duration;

    use rust_sak::memo::{CacheOpts, KeyBuilder, Memo, key_from};

    #[test]
    fn a_value_is_computed_once_and_then_served_from_the_cache() {
        let memo = Memo::memory(CacheOpts::new()).unwrap();
        let ttl = Duration::from_secs(60);
        let mut calls = 0;

        for _ in 0..2 {
            let value: u32 = memo
                .get_or_compute("answer", ttl, || {
                    calls += 1;
                    Ok::<_, std::convert::Infallible>(42)
                })
                .unwrap();
            assert_eq!(value, 42);
        }

        assert_eq!(calls, 1, "the second call should have been served from the cache");
        assert!(memo.path().is_none());
    }

    #[test]
    fn keys_are_deterministic_and_order_sensitive() {
        assert_eq!(key_from(["a", "b"]), key_from(["a", "b"]));
        assert_ne!(key_from(["a", "b"]), key_from(["b", "a"]));
        assert_eq!(KeyBuilder::new().part("a").part(&1_u32).finish().len(), 64);
    }
}

#[cfg(feature = "memo-async")]
mod memo_async {
    use std::time::Duration;

    use rust_sak::memo::{CacheOpts, Memo};

    #[tokio::test]
    async fn the_async_variant_is_reachable() {
        let memo = Memo::memory(CacheOpts::new()).unwrap();
        let value: String = memo
            .get_or_compute_async("greeting", Duration::from_secs(60), || async {
                Ok::<_, std::convert::Infallible>("hello".to_owned())
            })
            .await
            .unwrap();

        assert_eq!(value, "hello");
    }
}

#[cfg(feature = "o11y")]
mod o11y {
    use std::sync::LazyLock;
    use std::time::Duration;

    use rust_sak::o11y::{
        self, Config, ConfigBuilder, Environment, Level, NO_HEADERS, O11yError, Signal, Value, log, metric, trace,
    };

    static ORDERS: LazyLock<metric::Counter> = LazyLock::new(|| metric::counter("orders_total"));
    static ORDER_VALUE: LazyLock<metric::Histogram> =
        LazyLock::new(|| metric::histogram("order_value_dollars").with_buckets(&[10.0, 50.0, 100.0, 500.0]));
    static QUEUE_DEPTH: LazyLock<metric::Gauge> = LazyLock::new(|| metric::gauge("queue_depth"));

    #[trace::instrument(skip(secret))]
    fn submit_to_processor(order_id: &str, secret: &str) -> Result<(), std::fmt::Error> {
        let _ = (order_id, secret);
        Ok(())
    }

    /// One test, because `init` succeeds once per process and this binary has its own copy of the statics.
    #[test]
    fn the_whole_surface_is_reachable_from_outside_the_crate() {
        // Instruments are touched before `init`, which is the shape the `LazyLock` pattern forces and must work.
        ORDERS.increment(1);
        ORDERS.add_with_tags(1, &[("region", "eu-west-1")]);
        ORDER_VALUE.record(129.5);
        QUEUE_DEPTH.set(12);

        assert_eq!(ORDERS.value(&[]), 1);
        assert_eq!(ORDERS.value(&[("region", "eu-west-1")]), 1);
        assert_eq!(ORDER_VALUE.count(&[]), 1);
        assert_eq!(ORDER_VALUE.sum(&[]), 129.5);
        assert_eq!(QUEUE_DEPTH.value(&[]), Some(12.0));

        assert!(!o11y::is_enabled());
        assert!(o11y::session_id().is_none());
        assert!(o11y::machine_id().is_none());

        let config: Config = Config::builder("http://127.0.0.1:1", [("Authorization", "Bearer secret")])
            .service_name("api-test")
            .service_version("1.2.3")
            .environment(Environment::Production)
            .min_level(Level::Debug)
            .flush_interval(Duration::from_millis(50))
            .max_batch_size(8)
            .max_buffered(64)
            .timeout(Duration::from_millis(200))
            .enabled(false)
            .geolocation(false)
            .on_export_error(|error| {
                // Naming the type here is the point: a caller has to be able to match on it.
                assert!(!matches!(error, O11yError::AlreadyInitialized));
            })
            .build();

        o11y::init(config).unwrap();

        // Disabled, so nothing leaves the process and every one of these is a no-op that must still compile.
        let _span = trace::span!("checkout", order_id = "ord_8812", attempt = 1u32);
        trace::current().set_attribute("region", "eu-west-1");
        trace::current().add_event("submitted to processor");

        log::debug!("cache lookup", hit = false);
        log::info!("order received", order_id = "ord_8812", amount = 129.5);
        log::warn!("large order flagged", threshold = 10_000.0);
        log::error!("payment failed", error = %std::fmt::Error);

        submit_to_processor("ord_8812", "hunter2").unwrap();

        // The runtime emit, span contexts and owned spans: the paths a bridge from another logging API takes.
        let fields: log::Fields = vec![(std::borrow::Cow::Borrowed("codec"), Value::from("avif"))];
        let context = trace::SpanContext::from_traceparent("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
            .expect("a well-formed traceparent");
        assert_eq!(context.span_id_hex(), "00f067aa0ba902b7");
        assert!(
            trace::current().context().is_none(),
            "disabled, so the `checkout` guard never opened"
        );
        assert!(!trace::enabled(), "telemetry was disabled");

        log::emit(Level::Info, "encode.started", fields.clone());
        log::emit_in(context, Level::Warn, "encode.slow", Vec::new());

        let owned: trace::OwnedSpan = trace::start("encode", trace::Parent::Context(context), fields);
        assert_eq!(
            owned.context(),
            None,
            "a span started while tracing is off records nothing"
        );
        owned.set_attribute("threads", 8u32);
        owned.add_event_with("tile.done", Vec::new());
        owned.add_link(context);
        owned.set_error(&std::fmt::Error);
        owned.end();
        drop(trace::start("root", trace::Parent::Root, Vec::new()));
        drop(trace::start("current", trace::Parent::Current, Vec::new()));

        #[cfg(feature = "o11y-tracing")]
        {
            let _layer: o11y::tracing::TracingLayer = o11y::tracing::layer()
                .map_field(|_name, value| Some(value))
                .fold_spans(|metadata| metadata.target() == "ort");
        }

        o11y::renew_session();
        o11y::flush();
        o11y::shutdown();
        o11y::shutdown();

        // A second `init` is refused rather than silently reconfiguring.
        let refused = o11y::init(Config::builder("http://127.0.0.1:1", NO_HEADERS).build());
        assert!(matches!(refused, Err(O11yError::AlreadyInitialized)));

        // These are public because callers name them when assembling fields or handling export errors.
        assert!(matches!(Value::from(1_i64), Value::Int(1)));
        assert_eq!(Signal::Logs.path(), "/v1/logs");
        assert_eq!(Level::Info.severity_text(), "INFO");
        assert_eq!(Environment::Custom("staging".into()).to_string(), "staging");

        // `ConfigBuilder` is nameable, so a caller can build configuration behind their own helper.
        let _builder: ConfigBuilder = Config::builder("http://127.0.0.1:1", NO_HEADERS);
    }
}

#[cfg(feature = "sysinfo")]
mod sysinfo {
    use rust_sak::sysinfo::{cpu_info, gpu_info, memory_info};

    #[test]
    fn the_probes_answer() {
        let cpu = cpu_info();
        assert!(cpu.cores >= 1);
        assert!(!cpu.name.is_empty());

        assert!(memory_info().total > 0);

        // An empty list is a legitimate answer (a headless VM); only a hard failure is not.
        let _ = gpu_info().expect("enumerating GPUs should not fail on a supported platform");
    }
}

#[cfg(feature = "fetch")]
mod fetch {
    use rust_sak::fetch::{DownloadMode, Fetch, ProxySettings, RequestOptions};

    #[test]
    fn the_builders_are_usable_from_outside() {
        // No request is made: this is about the public surface being nameable and composable.
        let _fetch = Fetch::new()
            .header("Accept", "application/json")
            .retries(2)
            .disable_http2(true)
            .read_timeout(std::time::Duration::from_secs(5))
            .connect_timeout(std::time::Duration::from_secs(5))
            .download_mode(DownloadMode::Overwrite);

        // The proxy settings must be nameable and composable from outside, not just settable.
        let _via_proxy = Fetch::new().proxy(
            ProxySettings::new("http://proxy.example.com:3128")
                .basic_auth("user", "secret")
                .no_proxy("localhost"),
        );
        let _direct = Fetch::new().no_proxy();

        let _options = RequestOptions::new()
            .method(reqwest::Method::POST)
            .query("page", "2")
            .retry_non_idempotent(true)
            .resume_key("sha256:5cafbaae")
            .body(serde_json::json!({ "ok": true }));
    }
}

#[cfg(feature = "image-raw")]
mod image_raw {
    use rust_sak::image::{
        ImageError, RawFormat, RawImageInfo, decode_raw_bytes, decode_raw_file, is_raw_bytes, probe_raw_bytes,
        probe_raw_file,
    };

    #[test]
    fn the_raw_surface_is_reachable_from_outside_the_crate() {
        // Names only — the in-crate suite is what exercises the behaviour. What this catches is a `pub use` that was
        // never added, or a type that cannot be spelled from a downstream crate because a field's type is private.
        assert_eq!(RawFormat::from_extension("NEF"), Some(RawFormat::Nef));
        assert_eq!(RawFormat::from_path("/pictures/a.cr3"), Some(RawFormat::Cr3));
        assert_eq!(RawFormat::Dng.extension(), "dng");
        assert_eq!(RawFormat::from_magic(b"not raw"), None);
        assert!(!is_raw_bytes(b"not raw"));

        // A caller must be able to construct and read the info struct, not just receive one.
        let info = RawImageInfo {
            format: Some(RawFormat::Dng),
            width: 4,
            height: 4,
            bit_depth: None,
            make: String::new(),
            model: String::new(),
            is_dng: true,
        };
        assert_eq!(info.width, 4);

        // And the entry points, including that the RAW-specific error variants are nameable and matchable.
        assert!(matches!(decode_raw_bytes(b"not raw"), Err(ImageError::NotRaw)));
        assert!(matches!(probe_raw_bytes(b"not raw"), Err(ImageError::NotRaw)));
        assert!(matches!(
            decode_raw_file("/tmp/x.png"),
            Err(ImageError::UnknownExtension)
        ));
        assert!(matches!(
            probe_raw_file("/tmp/x.png"),
            Err(ImageError::UnknownExtension)
        ));
    }
}
