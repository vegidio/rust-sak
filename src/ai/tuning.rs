//! What a session is built with: each provider's baseline options, the caller's per-model [`Tuning`] over them, and
//! the cache folders the providers compile into.
//!
//! Nothing here calls the runtime or touches the disk, apart from [`CachePaths::create`]. The option maps are in each
//! provider's own key spelling, so they read line for line against its documentation.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use super::ExecutionProvider;

/// How a caller tunes one model's sessions, over each provider's baseline.
///
/// The default changes nothing: a model nobody has measured runs on the baseline. The right settings are a property of
/// the model (its op mix, its shapes, its precision), not of the provider, so an app passes one per model. Precision is
/// not a field: an app with FP16 and FP32 variants of a model passes each variant its own tuning.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tuning {
    /// Which of the Mac's engines CoreML may dispatch the model to.
    pub coreml_compute_units: CoreMlComputeUnits,
    /// How CoreML compiles the model for the device.
    pub coreml_specialization: CoreMlSpecialization,
    /// Whether independent branches of the graph may run on separate threads. A session setting, so it applies on
    /// every provider.
    pub execution_mode: ExecutionMode,
    /// Turns off the runtime's static memory planner, which over-allocates when shapes vary between runs.
    pub disable_mem_pattern: bool,
    /// Graph transformers to switch off, by the runtime's names for them. An unrecognised name is ignored by the
    /// runtime, silently.
    pub disabled_optimizers: Vec<String>,
    /// Runs CUDA convolutions in NHWC rather than the runtime's NCHW. Measure it per model and precision: the layout
    /// transform mishandles a convolution whose weights are computed, which then fails at run time.
    pub cuda_prefer_nhwc: bool,
    /// Raw TensorRT options, applied after the baseline so they can override it. An unrecognised key makes the runtime
    /// reject the whole update, which fails the TensorRT rung.
    pub trt_options: BTreeMap<String, String>,
    /// Makes the WebGPU plugin prefer NCHW for its layout-sensitive kernels, rather than its own NHWC.
    pub webgpu_prefer_nchw: bool,
    /// Graph nodes, by name, the WebGPU plugin must leave to the CPU: the escape hatch for a node whose WebGPU kernel is
    /// broken. Only nodes in the file can be named, and an unknown name is ignored.
    pub webgpu_force_cpu_nodes: Vec<String>,
}

/// The engines CoreML may dispatch a model to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CoreMlComputeUnits {
    /// CoreML chooses between the CPU, the GPU and the Neural Engine.
    #[default]
    All,
    /// Off the Neural Engine, for graphs it handles badly.
    CpuAndGpu,
    /// Off the GPU, leaving it for other work.
    CpuAndNeuralEngine,
}

impl CoreMlComputeUnits {
    /// The value of the runtime's `MLComputeUnits` option.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::All => "ALL",
            Self::CpuAndGpu => "CPUAndGPU",
            Self::CpuAndNeuralEngine => "CPUAndNeuralEngine",
        }
    }
}

/// How CoreML compiles a model for the device.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CoreMlSpecialization {
    /// CoreML's own strategy.
    #[default]
    Default,
    /// Longer compiles for faster predictions. Needs macOS 15; older systems ignore it.
    FastPrediction,
}

impl CoreMlSpecialization {
    /// The value of the runtime's `SpecializationStrategy` option.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::FastPrediction => "FastPrediction",
        }
    }
}

/// Whether the runtime may run independent branches of a graph on separate threads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ExecutionMode {
    /// Independent branches may run on separate threads.
    #[default]
    Parallel,
    /// One node at a time, for graphs with no branch wide enough to pay for the thread handoff.
    Sequential,
}

/// Where the providers keep what they compile from one model.
///
/// [`CachePaths::under`] puts all three under one model's folder. The engine folders belong to that model, but the
/// timing cache need not: kernel timings carry between graphs, so a build against a timing cache other models seeded
/// is faster than one against an empty cache, and an app with several models usually points every model's `timing` at
/// one shared folder. [`build`](super::build) serialises TensorRT builds, so sharing it within a process is safe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachePaths {
    /// TensorRT's engine cache: `<cache_dir>/tensorrt/` under [`CachePaths::under`].
    pub tensorrt: PathBuf,
    /// TensorRT's timing cache: `<cache_dir>/tensorrt/timing/` under [`CachePaths::under`], or a folder shared by
    /// every model.
    pub timing: PathBuf,
    /// CoreML's compiled model: `<cache_dir>/coreml/` under [`CachePaths::under`].
    pub coreml: PathBuf,
}

impl CachePaths {
    /// The cache folders under `cache_dir`, the timing cache included. Computes the paths only.
    pub fn under(cache_dir: &Path) -> Self {
        let tensorrt = cache_dir.join("tensorrt");
        Self {
            timing: tensorrt.join("timing"),
            tensorrt,
            coreml: cache_dir.join("coreml"),
        }
    }

    /// Creates the folders that are missing.
    ///
    /// # Errors
    ///
    /// The file error that stopped one being created.
    pub fn create(&self) -> io::Result<()> {
        for dir in [&self.tensorrt, &self.timing, &self.coreml] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

/// TensorRT's workspace ceiling with the card's memory unknown or large: 4 GiB.
const TRT_WORKSPACE_BYTES: u64 = 4 << 30;

/// TensorRT's workspace on a CUDA device with `device_memory` bytes: a quarter of the card, at most 4 GiB, and 4 GiB
/// when the memory is unknown.
///
/// The workspace is what each tactic TensorRT times may ask for. At 4 GiB on a 6 GB card the builder's requests failed
/// by the hundred and some builds failed outright, while a lower ceiling only skips tactics a card that size could not
/// run anyway.
pub fn trt_workspace_bytes(device_memory: Option<u64>) -> u64 {
    device_memory.map_or(TRT_WORKSPACE_BYTES, |bytes| (bytes / 4).min(TRT_WORKSPACE_BYTES))
}

/// Keeps CUDA graph capture off on both NVIDIA providers. With it on, TensorRT returns zeros from the second run on,
/// and CUDA fails the capture run.
const GRAPH_CAPTURE_DISABLED: &str = "0";

/// What separates the names in [`Tuning::disabled_optimizers`] in the runtime's config entry. The entries are not
/// trimmed, so there is no space after it.
const DISABLED_OPTIMIZER_SEPARATOR: &str = ";";

/// The options `provider` is attached with for a model tuned with `tuning`: the provider's baseline, with the tuning
/// over it. `paths` is written in as text and not touched, and `device_memory` (the CUDA device's, in bytes) sizes
/// TensorRT's workspace.
///
/// Empty for `Auto` and the CPU, which attach nothing, and for WebGPU unless the tuning sets something: the plugin runs
/// on its own defaults.
pub fn provider_options(
    provider: ExecutionProvider,
    tuning: &Tuning,
    paths: &CachePaths,
    device_memory: Option<u64>,
) -> BTreeMap<String, String> {
    match provider {
        ExecutionProvider::Auto | ExecutionProvider::Cpu => BTreeMap::new(),
        ExecutionProvider::CoreMl => coreml_options(tuning, paths),
        ExecutionProvider::Cuda => cuda_options(tuning),
        ExecutionProvider::TensorRt => tensorrt_options(tuning, paths, device_memory),
        ExecutionProvider::WebGpu => webgpu_options(tuning),
    }
}

fn options_from<const N: usize>(entries: [(&str, &str); N]) -> BTreeMap<String, String> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

/// CoreML compiles to an ML Program with static input shapes, and caches the compiled model. Both pins are correctness:
/// the older NeuralNetwork format may run an FP32 graph at half precision on the Neural Engine, and relaxing static
/// shapes on a fixed-shape graph has failed session creation outright.
fn coreml_options(tuning: &Tuning, paths: &CachePaths) -> BTreeMap<String, String> {
    options_from([
        ("EnableOnSubgraphs", "0"),
        ("MLComputeUnits", tuning.coreml_compute_units.as_str()),
        ("ModelCacheDirectory", &paths.coreml.display().to_string()),
        ("ModelFormat", "MLProgram"),
        ("RequireStaticInputShapes", "1"),
        ("SpecializationStrategy", tuning.coreml_specialization.as_str()),
    ])
}

/// CUDA on device 0 with graph capture off, an exhaustive convolution search and the full workspace, which pay off over
/// a session's many runs.
fn cuda_options(tuning: &Tuning) -> BTreeMap<String, String> {
    options_from([
        ("cudnn_conv_algo_search", "EXHAUSTIVE"),
        ("cudnn_conv_use_max_workspace", "1"),
        ("device_id", "0"),
        ("do_copy_in_default_stream", "1"),
        ("enable_cuda_graph", GRAPH_CAPTURE_DISABLED),
        ("gpu_mem_limit", "0"),
        ("prefer_nhwc", if tuning.cuda_prefer_nhwc { "1" } else { "0" }),
    ])
}

/// TensorRT on device 0 at FP32, graph capture off, engine and timing caches on, and an engine built for this card
/// only, which runs faster than a hardware-compatible one. The tuning's raw options go on last.
fn tensorrt_options(tuning: &Tuning, paths: &CachePaths, device_memory: Option<u64>) -> BTreeMap<String, String> {
    let mut options = options_from([
        ("device_id", "0"),
        (
            "trt_max_workspace_size",
            &trt_workspace_bytes(device_memory).to_string(),
        ),
        ("trt_fp16_enable", "0"),
        ("trt_int8_enable", "0"),
        ("trt_engine_hw_compatible", "0"),
        ("trt_cuda_graph_enable", GRAPH_CAPTURE_DISABLED),
        ("trt_builder_optimization_level", "5"),
        ("trt_engine_cache_enable", "1"),
        ("trt_engine_cache_path", &paths.tensorrt.display().to_string()),
        ("trt_timing_cache_enable", "1"),
        ("trt_timing_cache_path", &paths.timing.display().to_string()),
    ]);
    options.extend(tuning.trt_options.clone());
    options
}

/// Only what the tuning sets, in the plugin's own key spelling.
fn webgpu_options(tuning: &Tuning) -> BTreeMap<String, String> {
    let mut options = BTreeMap::new();
    if tuning.webgpu_prefer_nchw {
        options.insert("preferredLayout".to_owned(), "NCHW".to_owned());
    }
    if !tuning.webgpu_force_cpu_nodes.is_empty() {
        // The plugin splits this option on newlines.
        options.insert("forceCpuNodeNames".to_owned(), tuning.webgpu_force_cpu_nodes.join("\n"));
    }
    options
}

/// The session-level settings, as the builder calls that carry them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionSettings {
    /// What `with_parallel_execution` is given.
    pub(crate) parallel: bool,
    /// What `with_memory_pattern` is given.
    pub(crate) mem_pattern: bool,
    /// The disabled optimizers as the runtime's config entry takes them, or `None` to leave the entry unwritten.
    pub(crate) disabled_optimizers: Option<String>,
}

impl SessionSettings {
    /// The settings `tuning` asks for. Empty optimizer names are dropped, since the runtime would match nothing with
    /// them.
    pub(crate) fn of(tuning: &Tuning) -> Self {
        let names: Vec<&str> = tuning
            .disabled_optimizers
            .iter()
            .map(String::as_str)
            .filter(|name| !name.is_empty())
            .collect();
        Self {
            parallel: tuning.execution_mode == ExecutionMode::Parallel,
            mem_pattern: !tuning.disable_mem_pattern,
            disabled_optimizers: (!names.is_empty()).then(|| names.join(DISABLED_OPTIMIZER_SEPARATOR)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCELERATORS: [ExecutionProvider; 4] = [
        ExecutionProvider::CoreMl,
        ExecutionProvider::Cuda,
        ExecutionProvider::TensorRt,
        ExecutionProvider::WebGpu,
    ];

    fn paths() -> CachePaths {
        CachePaths::under(Path::new("/cache/siglip2/0123456789abcdef"))
    }

    fn options(provider: ExecutionProvider, tuning: &Tuning) -> BTreeMap<String, String> {
        provider_options(provider, tuning, &paths(), None)
    }

    /// The keys whose values differ between `a` and `b`, including keys only one of them has.
    fn changed(a: &BTreeMap<String, String>, b: &BTreeMap<String, String>) -> Vec<String> {
        let keys: std::collections::BTreeSet<&String> = a
            .keys()
            .chain(b.keys())
            .filter(|key| a.get(*key) != b.get(*key))
            .collect();
        keys.into_iter().cloned().collect()
    }

    #[test]
    fn the_cache_folders_are_under_the_callers_folder() {
        let paths = paths();
        assert_eq!(paths.tensorrt, Path::new("/cache/siglip2/0123456789abcdef/tensorrt"));
        assert_eq!(
            paths.timing,
            Path::new("/cache/siglip2/0123456789abcdef/tensorrt/timing")
        );
        assert_eq!(paths.coreml, Path::new("/cache/siglip2/0123456789abcdef/coreml"));
        assert!(!Path::new("/cache").exists(), "computing the paths created nothing");
    }

    #[test]
    fn the_cache_folders_are_created_on_request() {
        let dir = crate::fs::mk_temp_dir("rust-sak-ai-tuning").unwrap();
        let paths = CachePaths::under(&dir.path().join("model"));
        paths.create().unwrap();
        paths.create().unwrap();
        for folder in [&paths.tensorrt, &paths.timing, &paths.coreml] {
            assert!(folder.is_dir(), "{}", folder.display());
        }
    }

    #[test]
    fn the_default_tuning_gives_exactly_the_baseline() {
        let tuning = Tuning::default();
        // As the platform joins them, with backslashes on Windows.
        let paths = paths();
        let (coreml_dir, tensorrt_dir, timing_dir) = (
            paths.coreml.display().to_string(),
            paths.tensorrt.display().to_string(),
            paths.timing.display().to_string(),
        );
        let coreml = options(ExecutionProvider::CoreMl, &tuning);
        assert_eq!(
            coreml,
            options_from([
                ("EnableOnSubgraphs", "0"),
                ("MLComputeUnits", "ALL"),
                ("ModelCacheDirectory", &coreml_dir),
                ("ModelFormat", "MLProgram"),
                ("RequireStaticInputShapes", "1"),
                ("SpecializationStrategy", "Default"),
            ])
        );
        assert_eq!(
            options(ExecutionProvider::Cuda, &tuning),
            options_from([
                ("cudnn_conv_algo_search", "EXHAUSTIVE"),
                ("cudnn_conv_use_max_workspace", "1"),
                ("device_id", "0"),
                ("do_copy_in_default_stream", "1"),
                ("enable_cuda_graph", "0"),
                ("gpu_mem_limit", "0"),
                ("prefer_nhwc", "0"),
            ])
        );
        assert_eq!(
            options(ExecutionProvider::TensorRt, &tuning),
            options_from([
                ("device_id", "0"),
                ("trt_builder_optimization_level", "5"),
                ("trt_cuda_graph_enable", "0"),
                ("trt_engine_cache_enable", "1"),
                ("trt_engine_cache_path", &tensorrt_dir),
                ("trt_engine_hw_compatible", "0"),
                ("trt_fp16_enable", "0"),
                ("trt_int8_enable", "0"),
                ("trt_max_workspace_size", "4294967296"),
                ("trt_timing_cache_enable", "1"),
                ("trt_timing_cache_path", &timing_dir),
            ])
        );
        assert!(options(ExecutionProvider::WebGpu, &tuning).is_empty());
        assert!(options(ExecutionProvider::Cpu, &tuning).is_empty());
        assert!(options(ExecutionProvider::Auto, &tuning).is_empty());
    }

    #[test]
    fn each_typed_field_changes_only_its_key() {
        let cases: [(Tuning, ExecutionProvider, &str, &str); 6] = [
            (
                Tuning {
                    coreml_compute_units: CoreMlComputeUnits::CpuAndGpu,
                    ..Tuning::default()
                },
                ExecutionProvider::CoreMl,
                "MLComputeUnits",
                "CPUAndGPU",
            ),
            (
                Tuning {
                    coreml_compute_units: CoreMlComputeUnits::CpuAndNeuralEngine,
                    ..Tuning::default()
                },
                ExecutionProvider::CoreMl,
                "MLComputeUnits",
                "CPUAndNeuralEngine",
            ),
            (
                Tuning {
                    coreml_specialization: CoreMlSpecialization::FastPrediction,
                    ..Tuning::default()
                },
                ExecutionProvider::CoreMl,
                "SpecializationStrategy",
                "FastPrediction",
            ),
            (
                Tuning {
                    cuda_prefer_nhwc: true,
                    ..Tuning::default()
                },
                ExecutionProvider::Cuda,
                "prefer_nhwc",
                "1",
            ),
            (
                Tuning {
                    webgpu_prefer_nchw: true,
                    ..Tuning::default()
                },
                ExecutionProvider::WebGpu,
                "preferredLayout",
                "NCHW",
            ),
            (
                Tuning {
                    webgpu_force_cpu_nodes: vec!["node_pow".into(), "node_clamp".into()],
                    ..Tuning::default()
                },
                ExecutionProvider::WebGpu,
                "forceCpuNodeNames",
                "node_pow\nnode_clamp",
            ),
        ];
        for (tuning, provider, key, value) in cases {
            let tuned = options(provider, &tuning);
            assert_eq!(
                changed(&options(provider, &Tuning::default()), &tuned),
                [key],
                "{tuning:?}"
            );
            assert_eq!(tuned[key], value);
            for other in ACCELERATORS.into_iter().filter(|other| *other != provider) {
                assert_eq!(
                    options(other, &tuning),
                    options(other, &Tuning::default()),
                    "{other} for {key}"
                );
            }
        }
    }

    #[test]
    fn the_session_fields_change_no_provider_option() {
        let tuning = Tuning {
            execution_mode: ExecutionMode::Sequential,
            disable_mem_pattern: true,
            disabled_optimizers: vec!["GeluFusion".into()],
            ..Tuning::default()
        };
        for provider in ACCELERATORS {
            assert_eq!(
                options(provider, &tuning),
                options(provider, &Tuning::default()),
                "{provider}"
            );
        }
    }

    #[test]
    fn the_raw_tensorrt_overlay_overrides_a_baseline_key_and_adds_others() {
        let tuning = Tuning {
            trt_options: [("trt_builder_optimization_level", "3"), ("trt_dla_enable", "1")]
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .collect(),
            ..Tuning::default()
        };
        let tuned = options(ExecutionProvider::TensorRt, &tuning);
        assert_eq!(
            changed(&options(ExecutionProvider::TensorRt, &Tuning::default()), &tuned),
            ["trt_builder_optimization_level", "trt_dla_enable"]
        );
        assert_eq!(tuned["trt_builder_optimization_level"], "3");
        assert_eq!(tuned["trt_dla_enable"], "1");
    }

    #[test]
    fn the_tensorrt_workspace_is_a_quarter_of_the_card_up_to_4_gib() {
        assert_eq!(trt_workspace_bytes(Some(6 << 30)), 3 << 29, "1.5 GiB");
        assert_eq!(trt_workspace_bytes(Some(8 << 30)), 2 << 30);
        assert_eq!(trt_workspace_bytes(Some(16 << 30)), 4 << 30);
        assert_eq!(trt_workspace_bytes(Some(32 << 30)), 4 << 30);
        assert_eq!(trt_workspace_bytes(None), 4 << 30);
        let small = provider_options(ExecutionProvider::TensorRt, &Tuning::default(), &paths(), Some(6 << 30));
        assert_eq!(small["trt_max_workspace_size"], (3u64 << 29).to_string());
    }

    #[test]
    fn graph_capture_is_off_on_both_nvidia_providers() {
        assert_eq!(
            options(ExecutionProvider::TensorRt, &Tuning::default())["trt_cuda_graph_enable"],
            "0"
        );
        assert_eq!(
            options(ExecutionProvider::Cuda, &Tuning::default())["enable_cuda_graph"],
            "0"
        );
    }

    #[test]
    fn the_default_session_settings_are_parallel_with_the_planner_and_no_optimizer_off() {
        assert_eq!(
            SessionSettings::of(&Tuning::default()),
            SessionSettings {
                parallel: true,
                mem_pattern: true,
                disabled_optimizers: None
            }
        );
    }

    #[test]
    fn disabled_optimizers_are_joined_by_a_semicolon_with_no_spaces() {
        let tuning = Tuning {
            execution_mode: ExecutionMode::Sequential,
            disable_mem_pattern: true,
            disabled_optimizers: vec![
                "ReshapeFusion".into(),
                String::new(),
                "SimplifiedLayerNormFusion".into(),
            ],
            ..Tuning::default()
        };
        assert_eq!(
            SessionSettings::of(&tuning),
            SessionSettings {
                parallel: false,
                mem_pattern: false,
                disabled_optimizers: Some("ReshapeFusion;SimplifiedLayerNormFusion".into()),
            }
        );
        let only_empty = Tuning {
            disabled_optimizers: vec![String::new()],
            ..Tuning::default()
        };
        assert_eq!(SessionSettings::of(&only_empty).disabled_optimizers, None);
    }

    #[test]
    fn the_runtimes_spellings_are_pinned() {
        assert_eq!(CoreMlComputeUnits::All.as_str(), "ALL");
        assert_eq!(CoreMlComputeUnits::CpuAndGpu.as_str(), "CPUAndGPU");
        assert_eq!(CoreMlComputeUnits::CpuAndNeuralEngine.as_str(), "CPUAndNeuralEngine");
        assert_eq!(CoreMlSpecialization::Default.as_str(), "Default");
        assert_eq!(CoreMlSpecialization::FastPrediction.as_str(), "FastPrediction");
    }
}
