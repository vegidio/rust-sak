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
        ArchiveFormat, CopyOptions, ExtractOptions, FsError, ListOptions, copy_files, file_exists, list_path,
        mk_temp_dir, move_files, user_config_dir,
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
    }
}

#[cfg(feature = "image")]
mod image {
    use rust_sak::image::{
        EncodeOptions, ImageError, ImageFormat, decode_bytes, encode_writer, format_from_bytes, probe_bytes,
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
    use rust_sak::o11y::{Environment, Telemetry, Value};

    #[test]
    fn a_disabled_handle_is_fully_usable() {
        let telemetry = Telemetry::builder("http://127.0.0.1:1", "api-test")
            .version("1.2.3")
            .environment(Environment::Production)
            .enabled(false)
            .build()
            .unwrap();

        assert!(!telemetry.is_enabled());
        telemetry.event("api.smoke").field("count", 1_u64).info();
        telemetry.info("api.smoke");

        let before = telemetry.session_id();
        telemetry.renew_session();
        assert_ne!(before, telemetry.session_id());

        telemetry.flush().unwrap();
        telemetry.shutdown().unwrap();

        // `Value` is public because callers can name it when assembling fields dynamically.
        assert!(matches!(Value::from(1_i64), Value::Int(1)));
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
            .body(serde_json::json!({ "ok": true }));
    }
}
