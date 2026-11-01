//! Building a session on one rung, and on the first rung of its [`ladder`] that works.
//!
//! Every provider option goes through `with_arbitrary_config`, from the maps [`provider_options`] writes, and every
//! dispatch is built with `error_on_failure()`. `ort` otherwise registers a provider silently, logging a refusal and
//! carrying on without it, and the runtime rejects an options update wholesale: one bad key would cost the provider and
//! show only as a slow session. With it, the rung fails with the runtime's reason, and the ladder moves on.
//!
//! Every rung that attaches TensorRT is built under one process-wide lock. A timing cache is one file the runtime reads
//! at the start of a build and rewrites at the end with no locking of its own, so two such builds sharing it would
//! interleave into it. A failed one empties it: a half-written timing cache fails every later TensorRT build.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::sync::{Mutex, OnceLock, PoisonError};

use ort::environment::Environment;
use ort::ep::ArbitrarilyConfigurableExecutionProvider;
use ort::session::Session;
use ort::session::builder::SessionBuilder;

use super::logger::{out_of_memory_mark, out_of_memory_since, says_out_of_memory};
use super::tuning::SessionSettings;
use super::{CachePaths, ExecutionProvider, RungError, SessionError, Supported, Tuning, ladder, provider_options};

/// Held across every [`build`] whose rung attaches TensorRT. It guards an ordering, not data.
static TENSORRT_BUILD: Mutex<()> = Mutex::new(());

/// The first CUDA device's memory, which sizes TensorRT's workspace. Probed by the first rung that attaches TensorRT,
/// since the probe loads the CUDA driver, and kept for the process.
static DEVICE_MEMORY: OnceLock<Option<u64>> = OnceLock::new();

/// What a session is built from.
#[derive(Debug, Clone, Copy)]
pub struct SessionRequest<'a> {
    /// The ONNX model file.
    pub model: &'a Path,
    /// The providers asked for, in order. See [`ladder`] for how they become the rungs tried.
    pub providers: &'a [ExecutionProvider],
    /// What this machine offers, usually [`Supported::detect`].
    pub supported: Supported,
    /// This model's tuning over the providers' baseline. [`Tuning::default`] changes nothing.
    pub tuning: &'a Tuning,
    /// The folders the providers compile into, usually [`CachePaths::under`] the model's own cache folder. Each is
    /// created when a rung whose provider writes it is tried. Key the engine folders by the model file's content,
    /// since CoreML keys its cache by the model's path. The timing folder may be shared by every model.
    pub cache: &'a CachePaths,
}

/// A session, and the providers of the rung it was built on. An empty list means the CPU.
#[derive(Debug)]
pub struct Built {
    /// The session.
    pub session: Session,
    /// The rung's providers, in attach order: never `Auto` or `CPU`.
    pub providers: Vec<ExecutionProvider>,
}

/// Build a session for `request.model` on the first rung of its [`ladder`] that works, with [`build`].
///
/// A rung that fails is logged at warning level with the model, the providers and the reason, and the next is tried.
/// The CPU is the last rung, so this fails only when the CPU cannot open the model either.
///
/// The build runs on the calling thread and can take minutes, when CoreML or TensorRT compile a large model for the
/// first time, so a GUI calls it off its UI thread. It needs the runtime [`load`](super::load)ed first.
///
/// # Errors
///
/// [`SessionError`] with the model's path and the CPU's reason: the file is missing or not an ONNX model, or the
/// runtime is not loaded.
pub fn session(request: &SessionRequest<'_>) -> Result<Built, SessionError> {
    let rungs = ladder(request.providers, request.supported);
    let (session, providers) = walk(request.model, &rungs, |rung| {
        build(request, rung).map_err(|error| error.reason)
    })?;
    Ok(Built { session, providers })
}

/// Build a session for `request.model` on exactly `rung`: its providers attached in order, each with its baseline and
/// `request.tuning` over it, then the tuning's session settings, then the model. An empty rung is the CPU, which
/// attaches nothing; `Auto` and `CPU` inside a rung are ignored, as [`ladder`] never puts them there.
///
/// `request.providers` and `request.supported` are not read: the caller has chosen the rung, usually from [`ladder`].
/// The cache folders the rung's providers write are created first. A rung that attaches TensorRT waits for any other
/// such build in the process to finish, and a failure empties `request.cache.timing`, keeping the folder, with an
/// `info` record naming it.
///
/// The build runs **on the calling thread**, which is a contract: the runtime logs a build's records there, so a
/// caller's span entered around it decorates them, and the out-of-memory count the error is classified by is this
/// thread's. It can take minutes and needs the runtime [`load`](super::load)ed first.
///
/// # Errors
///
/// [`RungError`] with the rung's providers, the runtime's reason, and whether the device ran out of memory.
///
/// # Panics
///
/// Never on its own account; a panic inside the runtime's builder propagates.
pub fn build(request: &SessionRequest<'_>, rung: &[ExecutionProvider]) -> Result<Session, RungError> {
    let rung: Vec<Accelerator> = rung.iter().filter_map(|&provider| Accelerator::of(provider)).collect();
    let providers: Vec<ExecutionProvider> = rung.iter().map(|accelerator| accelerator.provider()).collect();
    let tensorrt = rung.contains(&Accelerator::TensorRt);
    let mark = out_of_memory_mark();
    serialized(tensorrt, &request.cache.timing, || attempt(request, &rung))
        .map_err(|reason| rung_error(providers, reason, mark))
}

/// One attempt on `rung`, with the runtime's reason when it fails.
fn attempt(request: &SessionRequest<'_>, rung: &[Accelerator]) -> Result<Session, String> {
    let environment = super::load::environment().ok_or_else(|| "the ONNX Runtime is not loaded".to_owned())?;
    create_folders(request.cache, rung).map_err(|cause| format!("its cache folder could not be created: {cause}"))?;
    let device_memory = if rung.contains(&Accelerator::TensorRt) {
        *DEVICE_MEMORY.get_or_init(|| {
            crate::sysinfo::cuda_info()
                .and_then(|info| info.devices.into_iter().next())
                .and_then(|device| device.total_memory)
        })
    } else {
        None
    };
    build_rung(&environment, request, rung, device_memory).map_err(|error| error.to_string())
}

/// The error for a failed attempt on `providers`, taken after `mark`: out of memory when the runtime logged a record
/// saying so on this thread since, or its reason says so. Never on the CPU, which releasing device memory cannot help.
fn rung_error(providers: Vec<ExecutionProvider>, reason: String, mark: u64) -> RungError {
    let out_of_memory = !providers.is_empty() && (out_of_memory_since(mark) || says_out_of_memory(&reason));
    RungError {
        providers,
        reason,
        out_of_memory,
    }
}

/// Creates the folders `rung`'s providers compile into, and no others.
fn create_folders(paths: &CachePaths, rung: &[Accelerator]) -> io::Result<()> {
    for accelerator in rung {
        match accelerator {
            Accelerator::CoreMl => std::fs::create_dir_all(&paths.coreml)?,
            Accelerator::TensorRt => {
                std::fs::create_dir_all(&paths.tensorrt)?;
                std::fs::create_dir_all(&paths.timing)?;
            }
            Accelerator::Cuda | Accelerator::WebGpu => {}
        }
    }
    Ok(())
}

/// Runs `build`, under [`TENSORRT_BUILD`] when `tensorrt`, emptying `timing` when such a build fails. A build without
/// TensorRT neither waits nor discards: it never touches the timing cache.
fn serialized<T, E>(tensorrt: bool, timing: &Path, build: impl FnOnce() -> Result<T, E>) -> Result<T, E> {
    if !tensorrt {
        return build();
    }
    // A poisoned lock means an earlier build panicked; the next one still has to be the only one.
    let _serialized = TENSORRT_BUILD.lock().unwrap_or_else(PoisonError::into_inner);
    build().inspect_err(|_| {
        // Swallowed: this is already returning the build's error, which a cache that would not go is not worth
        // replacing.
        let _ = empty(timing);
        tracing::info!(
            path = %timing.display(),
            "the TensorRT timing cache was discarded after a failed build; the next engine build will be slower"
        );
    })
}

/// Removes everything inside `dir`, keeping `dir`.
fn empty(dir: &Path) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(entry.path())?;
        } else {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

/// A provider a rung attaches: the hardware, without `Auto` or the CPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Accelerator {
    CoreMl,
    Cuda,
    TensorRt,
    WebGpu,
}

impl Accelerator {
    const fn of(provider: ExecutionProvider) -> Option<Self> {
        match provider {
            ExecutionProvider::Auto | ExecutionProvider::Cpu => None,
            ExecutionProvider::CoreMl => Some(Self::CoreMl),
            ExecutionProvider::Cuda => Some(Self::Cuda),
            ExecutionProvider::TensorRt => Some(Self::TensorRt),
            ExecutionProvider::WebGpu => Some(Self::WebGpu),
        }
    }

    const fn provider(self) -> ExecutionProvider {
        match self {
            Self::CoreMl => ExecutionProvider::CoreMl,
            Self::Cuda => ExecutionProvider::Cuda,
            Self::TensorRt => ExecutionProvider::TensorRt,
            Self::WebGpu => ExecutionProvider::WebGpu,
        }
    }
}

/// One attempt: a builder with `rung` attached in order, the tuning's session settings, and the model committed.
fn build_rung(
    environment: &Environment,
    request: &SessionRequest<'_>,
    rung: &[Accelerator],
    device_memory: Option<u64>,
) -> Result<Session, ort::Error> {
    let mut builder = Session::builder(environment)?;
    for &accelerator in rung {
        let options = provider_options(accelerator.provider(), request.tuning, request.cache, device_memory);
        let dispatch = match accelerator {
            Accelerator::CoreMl => configure(ort::ep::CoreML::default(), &options).build(),
            Accelerator::Cuda => configure(ort::ep::CUDA::default(), &options).build(),
            Accelerator::TensorRt => configure(ort::ep::TensorRT::default(), &options).build(),
            Accelerator::WebGpu => {
                builder = attach_webgpu(environment, builder, &options)?;
                continue;
            }
        };
        builder = builder.with_execution_providers([dispatch.error_on_failure()])?;
    }

    let settings = SessionSettings::of(request.tuning);
    builder = builder
        .with_parallel_execution(settings.parallel)?
        .with_memory_pattern(settings.mem_pattern)?;
    if let Some(disabled) = &settings.disabled_optimizers {
        builder = builder.with_disabled_optimizers(disabled)?;
    }

    builder.commit_from_file(request.model)
}

/// `provider` with every entry of `options` applied.
fn configure<E: ArbitrarilyConfigurableExecutionProvider>(provider: E, options: &BTreeMap<String, String>) -> E {
    options.iter().fold(provider, |provider, (key, value)| {
        provider.with_arbitrary_config(key, value)
    })
}

/// Attaches the WebGPU plugin through its first device, the only way a plugin registered as a library can be attached.
/// One device, because the runtime refuses one append whose devices come from different factories. Its options are
/// prefixed with the provider's name, which `ort` strips again.
fn attach_webgpu(
    environment: &Environment,
    builder: SessionBuilder,
    options: &BTreeMap<String, String>,
) -> Result<SessionBuilder, ort::Error> {
    let device = super::webgpu::devices(environment)
        .next()
        .ok_or_else(|| ort::Error::new("the WebGPU plugin offers no device"))?;
    let options: Vec<(String, String)> = options
        .iter()
        .map(|(key, value)| (format!("{}.{key}", super::webgpu::WEBGPU_EP), value.clone()))
        .collect();
    Ok(builder.with_devices([device], Some(&options))?)
}

/// Tries `rungs` in order with `try_rung`, returning what the first one that works built, and its providers.
///
/// Each rung that fails is logged at warning level with the model, its providers and the reason. When every rung
/// fails, the error carries the model's path and the last rung's reason, which is the CPU's.
fn walk<T>(
    model: &Path,
    rungs: &[Vec<ExecutionProvider>],
    mut try_rung: impl FnMut(&[ExecutionProvider]) -> Result<T, String>,
) -> Result<(T, Vec<ExecutionProvider>), SessionError> {
    let mut last = String::from("the ladder has no rung");
    for rung in rungs {
        match try_rung(rung) {
            Ok(built) => return Ok((built, rung.clone())),
            Err(reason) => {
                tracing::warn!(
                    model = %model.display(),
                    providers = %names(rung),
                    %reason,
                    "a session could not be built on these providers; trying the next"
                );
                last = reason;
            }
        }
    }
    Err(SessionError {
        model: model.to_path_buf(),
        reason: last,
    })
}

/// The providers of `rung` as a reader sees them: `CoreML, WebGPU`, or `CPU` for the CPU alone.
fn names(rung: &[ExecutionProvider]) -> String {
    if rung.is_empty() {
        return ExecutionProvider::Cpu.to_string();
    }
    rung.iter()
        .map(|provider| provider.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};

    use super::ExecutionProvider::{Auto, CoreMl, Cpu, Cuda, TensorRt, WebGpu};
    use super::*;

    /// Every event logged at one level while it is the default subscriber, as `field=value` pairs.
    #[derive(Clone)]
    struct Warnings(Arc<Mutex<Vec<String>>>, Level);

    impl Default for Warnings {
        fn default() -> Self {
            Self::at(Level::WARN)
        }
    }

    impl Warnings {
        fn at(level: Level) -> Self {
            Self(Arc::default(), level)
        }

        fn take(&self) -> Vec<String> {
            std::mem::take(&mut self.0.lock().unwrap())
        }
    }

    struct Fields(String);

    impl Visit for Fields {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0.push_str(&format!("{}={value:?} ", field.name()));
        }

        fn record_str(&mut self, field: &Field, value: &str) {
            self.0.push_str(&format!("{}={value} ", field.name()));
        }
    }

    impl Subscriber for Warnings {
        fn enabled(&self, metadata: &Metadata<'_>) -> bool {
            *metadata.level() == self.1
        }
        fn new_span(&self, _: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }
        fn record(&self, _: &Id, _: &Record<'_>) {}
        fn record_follows_from(&self, _: &Id, _: &Id) {}
        fn event(&self, event: &Event<'_>) {
            let mut fields = Fields(String::new());
            event.record(&mut fields);
            self.0.lock().unwrap().push(fields.0);
        }
        fn enter(&self, _: &Id) {}
        fn exit(&self, _: &Id) {}
    }

    /// What `walk` returned with the fake builder: what it built and on which rung.
    type Walked = Result<(String, Vec<ExecutionProvider>), SessionError>;

    /// `walk` over `rungs` with a builder that fails the rungs in `failing`, with the warnings it logged.
    fn walked(rungs: &[Vec<ExecutionProvider>], failing: &[Vec<ExecutionProvider>]) -> (Walked, Vec<String>) {
        let warnings = Warnings::default();
        let result = tracing::subscriber::with_default(warnings.clone(), || {
            walk(Path::new("/models/scrfd.onnx"), rungs, |rung| {
                if failing.iter().any(|failed| failed == rung) {
                    Err(format!("{} declined", names(rung)))
                } else {
                    Ok(format!("built on {}", names(rung)))
                }
            })
        });
        (result, warnings.take())
    }

    #[test]
    fn the_first_rung_that_builds_is_kept_with_no_warning() {
        let rungs = [vec![CoreMl, WebGpu], vec![WebGpu], vec![]];
        let (result, warnings) = walked(&rungs, &[]);
        assert_eq!(
            result.unwrap(),
            ("built on CoreML, WebGPU".into(), vec![CoreMl, WebGpu])
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn a_failing_first_rung_is_warned_about_and_the_second_kept() {
        let rungs = [vec![TensorRt, Cuda], vec![Cuda], vec![]];
        let (result, warnings) = walked(&rungs, &[vec![TensorRt, Cuda]]);
        assert_eq!(result.unwrap(), ("built on CUDA".into(), vec![Cuda]));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("model=/models/scrfd.onnx"), "{}", warnings[0]);
        assert!(warnings[0].contains("providers=TensorRT, CUDA"), "{}", warnings[0]);
        assert!(
            warnings[0].contains("reason=TensorRT, CUDA declined"),
            "{}",
            warnings[0]
        );
    }

    #[test]
    fn every_rung_failing_yields_the_cpus_reason_and_the_models_path() {
        let rungs = [vec![CoreMl, WebGpu], vec![WebGpu], vec![]];
        let (result, warnings) = walked(&rungs, &rungs);
        let error = result.unwrap_err();
        assert_eq!(
            error,
            SessionError {
                model: PathBuf::from("/models/scrfd.onnx"),
                reason: "CPU declined".into()
            }
        );
        assert_eq!(error.to_string(), "/models/scrfd.onnx: CPU declined");
        assert_eq!(warnings.len(), 3, "one warning per failed rung: {warnings:?}");
        for (warning, providers) in warnings.iter().zip(["CoreML, WebGPU", "WebGPU", "CPU"]) {
            assert!(warning.contains(&format!("providers={providers} ")), "{warning}");
        }
    }

    #[test]
    fn the_cpu_alone_is_named_cpu() {
        assert_eq!(names(&[]), "CPU");
        assert_eq!(names(&[TensorRt, Cuda]), "TensorRT, CUDA");
    }

    #[test]
    fn an_empty_rung_attaches_nothing_and_creates_no_cache_folder() {
        // This binary never loads the runtime, so the attempt fails before the builder; what it shows is what the rung
        // was taken to be.
        let dir = crate::fs::mk_temp_dir("rust-sak-ai-session").unwrap();
        let paths = CachePaths::under(&dir.path().join("model"));
        let tuning = Tuning::default();
        let request = SessionRequest {
            model: Path::new("/nowhere/model.onnx"),
            providers: &[Auto],
            supported: Supported::detect(false, false),
            tuning: &tuning,
            cache: &paths,
        };
        for rung in [&[][..], &[Auto, Cpu]] {
            let error = build(&request, rung).unwrap_err();
            assert_eq!(error.providers, [], "{rung:?}");
            assert!(!error.out_of_memory);
            assert_eq!(error.to_string(), error.reason);
        }
        assert!(!dir.path().join("model").exists(), "the CPU compiles nothing");
    }

    #[test]
    fn only_the_folders_a_rungs_providers_write_are_created() {
        let dir = crate::fs::mk_temp_dir("rust-sak-ai-session").unwrap();
        let paths = CachePaths {
            timing: dir.path().join("shared-timing"),
            ..CachePaths::under(&dir.path().join("model"))
        };
        create_folders(&paths, &[Accelerator::Cuda, Accelerator::WebGpu]).unwrap();
        assert!(!dir.path().join("model").exists());
        create_folders(&paths, &[Accelerator::CoreMl]).unwrap();
        assert!(paths.coreml.is_dir());
        assert!(!paths.tensorrt.exists() && !paths.timing.exists());
        create_folders(&paths, &[Accelerator::TensorRt]).unwrap();
        assert!(paths.tensorrt.is_dir() && paths.timing.is_dir());
    }

    /// The runtime's message for the TensorRT failure from the field, which does not mention memory.
    const NO_ENGINE: &str = "TensorRT EP failed to create engine from network for fused node: TRTKernel_graph_0";

    /// A record of a failed device allocation, logged on this thread through the environment's logger.
    fn log_out_of_memory() {
        super::super::logger::wrap(Some(Arc::new(|_, _, _, _, _| {})))(
            ort::logging::LogLevel::Error,
            "",
            "app",
            "globWriter.cpp:496",
            "Error Code 2: OutOfMemory (Requested size was 1024 bytes.)",
        );
    }

    #[test]
    fn an_out_of_memory_record_on_the_build_thread_marks_the_rung() {
        let mark = out_of_memory_mark();
        assert!(!rung_error(vec![TensorRt, Cuda], NO_ENGINE.into(), mark).out_of_memory);
        log_out_of_memory();
        let error = rung_error(vec![TensorRt, Cuda], NO_ENGINE.into(), mark);
        assert!(error.out_of_memory);
        assert_eq!(error.providers, [TensorRt, Cuda]);
        assert_eq!(error.reason, NO_ENGINE);
    }

    #[test]
    fn an_out_of_memory_record_on_another_thread_does_not_mark_the_rung() {
        let mark = out_of_memory_mark();
        std::thread::spawn(log_out_of_memory).join().unwrap();
        assert!(!rung_error(vec![TensorRt, Cuda], NO_ENGINE.into(), mark).out_of_memory);
    }

    #[test]
    fn a_cuda_failure_that_says_out_of_memory_marks_the_rung() {
        let mark = out_of_memory_mark();
        assert!(rung_error(vec![Cuda], "CUDA failure 2: out of memory".into(), mark).out_of_memory);
    }

    #[test]
    fn a_cpu_failure_is_never_out_of_memory() {
        let mark = out_of_memory_mark();
        log_out_of_memory();
        assert!(!rung_error(Vec::new(), "Failed to allocate memory".into(), mark).out_of_memory);
    }

    #[test]
    fn two_tensorrt_builds_never_overlap() {
        let timing = crate::fs::mk_temp_dir("rust-sak-ai-session").unwrap();
        let inside = AtomicUsize::new(0);
        let overlapped = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(|| {
                    let outcome: Result<(), ()> = serialized(true, timing.path(), || {
                        if inside.fetch_add(1, Ordering::SeqCst) != 0 {
                            overlapped.fetch_add(1, Ordering::SeqCst);
                        }
                        std::thread::sleep(Duration::from_millis(50));
                        inside.fetch_sub(1, Ordering::SeqCst);
                        Ok(())
                    });
                    outcome.unwrap();
                });
            }
        });
        assert_eq!(overlapped.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_poisoned_lock_is_still_taken() {
        let timing = crate::fs::mk_temp_dir("rust-sak-ai-session").unwrap();
        let path = timing.path().to_path_buf();
        let panicked = std::thread::spawn(move || {
            let _: Result<(), ()> = serialized(true, &path, || panic!("a build panicked"));
        })
        .join();
        assert!(panicked.is_err());
        let outcome: Result<u8, ()> = serialized(true, timing.path(), || Ok(1));
        assert_eq!(outcome, Ok(1));
    }

    #[test]
    fn a_failed_tensorrt_build_empties_the_timing_folder_and_still_returns_its_error() {
        let timing = crate::fs::mk_temp_dir("rust-sak-ai-session").unwrap();
        std::fs::write(timing.path().join("timing.cache"), b"half of a timing cache").unwrap();
        std::fs::create_dir(timing.path().join("nested")).unwrap();
        std::fs::write(timing.path().join("nested/more"), b"x").unwrap();

        let warnings = Warnings::at(Level::INFO);
        let outcome: Result<(), &str> = tracing::subscriber::with_default(warnings.clone(), || {
            serialized(true, timing.path(), || Err("the engine build failed"))
        });

        assert_eq!(outcome, Err("the engine build failed"));
        assert!(timing.path().is_dir(), "the folder stays");
        assert_eq!(std::fs::read_dir(timing.path()).unwrap().count(), 0, "its contents go");
        let records = warnings.take();
        assert_eq!(records.len(), 1, "{records:?}");
        assert!(
            records[0].contains(&format!("path={} ", timing.path().display())),
            "{}",
            records[0]
        );
        assert!(records[0].contains("timing cache was discarded"), "{}", records[0]);
    }

    #[test]
    fn a_failed_build_without_tensorrt_leaves_the_timing_folder_alone() {
        let timing = crate::fs::mk_temp_dir("rust-sak-ai-session").unwrap();
        let cache = timing.path().join("timing.cache");
        std::fs::write(&cache, b"measured across every model").unwrap();

        let warnings = Warnings::at(Level::INFO);
        let outcome: Result<(), &str> = tracing::subscriber::with_default(warnings.clone(), || {
            serialized(false, timing.path(), || Err("the model would not open"))
        });

        assert_eq!(outcome, Err("the model would not open"));
        assert!(cache.is_file());
        assert!(warnings.take().is_empty());
    }
}
