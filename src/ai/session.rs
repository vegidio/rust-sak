//! Building a session on the first rung of its [`ladder`] that works.
//!
//! Every provider option goes through `with_arbitrary_config`, from the maps [`provider_options`] writes, and every
//! dispatch is built with `error_on_failure()`. `ort` otherwise registers a provider silently, logging a refusal and
//! carrying on without it, and the runtime rejects an options update wholesale: one bad key would cost the provider and
//! show only as a slow session. With it, the rung fails with the runtime's reason, and the ladder moves on.

use std::collections::BTreeMap;
use std::path::Path;

use ort::environment::Environment;
use ort::ep::ArbitrarilyConfigurableExecutionProvider;
use ort::session::Session;
use ort::session::builder::SessionBuilder;

use super::tuning::SessionSettings;
use super::{CachePaths, ExecutionProvider, SessionError, Supported, Tuning, ladder, provider_options};

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
    /// The model's own cache folder, which holds what the providers compile from it (see [`CachePaths`]). It is
    /// created when a rung that compiles is tried. Key it by the model file's content, since CoreML keys its cache by
    /// the model's path.
    pub cache_dir: &'a Path,
}

/// A session, and the providers of the rung it was built on. An empty list means the CPU.
#[derive(Debug)]
pub struct Built {
    /// The session.
    pub session: Session,
    /// The rung's providers, in attach order: never `Auto` or `CPU`.
    pub providers: Vec<ExecutionProvider>,
}

/// Build a session for `request.model` on the first rung of its [`ladder`] that works.
///
/// Each rung starts from a new builder: the session settings from the tuning, then each provider in the rung's order
/// with its baseline and tuning, then the model. A rung that fails is logged at warning level with the model, the
/// providers and the reason, and the next is tried. The CPU is the last rung, so this fails only when the CPU cannot
/// open the model either.
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
    let paths = CachePaths::under(request.cache_dir);
    // The CUDA probe loads the driver, so it is asked only when TensorRT may be attached.
    let device_memory = if rungs
        .iter()
        .flatten()
        .any(|&provider| provider == ExecutionProvider::TensorRt)
    {
        crate::sysinfo::cuda_info()
            .and_then(|info| info.devices.into_iter().next())
            .and_then(|device| device.total_memory)
    } else {
        None
    };
    let environment = super::load::environment();

    let (session, providers) = walk(request.model, &rungs, |rung| {
        let environment = environment
            .as_ref()
            .ok_or_else(|| "the ONNX Runtime is not loaded".to_owned())?;
        let rung: Vec<Accelerator> = rung.iter().filter_map(|&provider| Accelerator::of(provider)).collect();
        if rung.iter().any(|accelerator| accelerator.compiles()) {
            paths
                .create()
                .map_err(|cause| format!("its cache folder could not be created: {cause}"))?;
        }
        build_rung(environment, request, &rung, &paths, device_memory).map_err(|error| error.to_string())
    })?;
    Ok(Built { session, providers })
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

    /// Whether it writes into the cache folder.
    const fn compiles(self) -> bool {
        matches!(self, Self::CoreMl | Self::TensorRt)
    }
}

/// One attempt: a builder with `rung` attached in order, the tuning's session settings, and the model committed.
fn build_rung(
    environment: &Environment,
    request: &SessionRequest<'_>,
    rung: &[Accelerator],
    paths: &CachePaths,
    device_memory: Option<u64>,
) -> Result<Session, ort::Error> {
    let mut builder = Session::builder(environment)?;
    for &accelerator in rung {
        let options = provider_options(accelerator.provider(), request.tuning, paths, device_memory);
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
    use std::sync::{Arc, Mutex};

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};

    use super::ExecutionProvider::{CoreMl, Cuda, TensorRt, WebGpu};
    use super::*;

    /// Every warning logged while it is the default subscriber, as `field=value` pairs.
    #[derive(Clone, Default)]
    struct Warnings(Arc<Mutex<Vec<String>>>);

    impl Warnings {
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
            *metadata.level() == Level::WARN
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
}
