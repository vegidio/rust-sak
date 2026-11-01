//! The execution providers a session can be asked for, which of them this machine offers, and the fallback ladder a
//! request becomes.
//!
//! Everything here is a pure function of its arguments, apart from [`Supported::detect`], which reads what the runtime's
//! load found. A machine is a handful of booleans, so a Mac, an NVIDIA workstation and a machine with no accelerator are
//! values any test can write down.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// An execution provider a session can be asked for.
///
/// The spellings ([`as_str`](Self::as_str), [`Display`](fmt::Display) and the serialized form) are stable and shared
/// with Open Photo AI, so a stored choice means the same in both apps. [`FromStr`] reads them case-insensitively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExecutionProvider {
    /// A request for the best this machine offers: every supported accelerator in [`AUTO_ORDER`]. Never a provider a
    /// session runs on.
    Auto,
    /// The CPU. Always offered, takes no configuration, and the last rung of every ladder.
    #[serde(rename = "CPU")]
    Cpu,
    /// Apple's CoreML, on macOS.
    #[serde(rename = "CoreML")]
    CoreMl,
    /// NVIDIA's CUDA provider.
    #[serde(rename = "CUDA")]
    Cuda,
    /// NVIDIA's TensorRT provider, which compiles an engine from the graph.
    #[serde(rename = "TensorRT")]
    TensorRt,
    /// The runtime's WebGPU plugin: any GPU that Metal, D3D12 or Vulkan reaches.
    #[serde(rename = "WebGPU")]
    WebGpu,
}

impl ExecutionProvider {
    /// Every provider, in the order a user is most likely to reach for one.
    pub const ALL: [Self; 6] = [
        Self::Auto,
        Self::Cpu,
        Self::CoreMl,
        Self::Cuda,
        Self::TensorRt,
        Self::WebGpu,
    ];

    /// The user-facing spelling: `Auto`, `CPU`, `CoreML`, `CUDA`, `TensorRT` or `WebGPU`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Cpu => "CPU",
            Self::CoreMl => "CoreML",
            Self::Cuda => "CUDA",
            Self::TensorRt => "TensorRT",
            Self::WebGpu => "WebGPU",
        }
    }

    /// Whether this is hardware a session attaches: neither the request (`Auto`) nor the CPU, which is attached by
    /// nobody and is what the runtime falls back to on its own.
    pub const fn is_accelerator(self) -> bool {
        !matches!(self, Self::Auto | Self::Cpu)
    }
}

impl fmt::Display for ExecutionProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A name that is none of the six [`ExecutionProvider`]s.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown execution provider `{name}`; expected one of Auto, CPU, CoreML, CUDA, TensorRT or WebGPU")]
pub struct UnknownProvider {
    /// The text that was given.
    pub name: String,
}

impl FromStr for ExecutionProvider {
    type Err = UnknownProvider;

    /// The provider `text` names, ignoring case.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|provider| provider.as_str().eq_ignore_ascii_case(text))
            .ok_or_else(|| UnknownProvider { name: text.to_owned() })
    }
}

/// The order [`ExecutionProvider::Auto`] prefers accelerators in, best first. WebGPU is last: it reaches the most GPUs,
/// and is the slowest of them where a vendor's own provider is there.
pub const AUTO_ORDER: [ExecutionProvider; 4] = [
    ExecutionProvider::TensorRt,
    ExecutionProvider::Cuda,
    ExecutionProvider::CoreMl,
    ExecutionProvider::WebGpu,
];

/// The providers this machine offers.
///
/// What a session can be *asked* for, not a promise that it builds: an offered provider can still fail at build time,
/// and the [`ladder`] then moves on to the next rung.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Supported {
    /// Always `true`. A field rather than an implicit truth, so a list can be rendered without a special case.
    pub cpu: bool,
    /// On macOS.
    pub coreml: bool,
    /// As the caller reported it: the GPU can run the CUDA the caller installed, and CUDA and cuDNN are installed.
    pub cuda: bool,
    /// As the caller reported it, and only alongside CUDA.
    pub tensorrt: bool,
    /// The runtime's load registered the WebGPU plugin, and it offered at least one device.
    pub webgpu: bool,
}

impl Supported {
    /// What this machine offers: the CPU, CoreML on macOS, WebGPU as the runtime's [`load`](super::load) found it, and
    /// CUDA and TensorRT as the caller reports them. Only the caller knows which NVIDIA libraries it installed.
    /// TensorRT is offered only with CUDA, since it runs on it.
    ///
    /// Before a successful load, WebGPU is not offered.
    pub fn detect(cuda: bool, tensorrt: bool) -> Self {
        Self {
            cpu: true,
            // No OS-version probe: the runtime's macOS build already needs a newer macOS than CoreML's ML Program
            // format does (12). Were that ever wrong, the CoreML rung would fail and the ladder would move on.
            coreml: cfg!(target_os = "macos"),
            cuda,
            tensorrt: cuda && tensorrt,
            webgpu: super::load::webgpu_offered(),
        }
    }

    /// Whether this machine offers `provider`. [`Auto`](ExecutionProvider::Auto) and the CPU are offered everywhere.
    pub const fn supports(self, provider: ExecutionProvider) -> bool {
        match provider {
            ExecutionProvider::Auto => true,
            ExecutionProvider::Cpu => self.cpu,
            ExecutionProvider::CoreMl => self.coreml,
            ExecutionProvider::Cuda => self.cuda,
            ExecutionProvider::TensorRt => self.tensorrt,
            ExecutionProvider::WebGpu => self.webgpu,
        }
    }

    /// Every provider a session can run on here, in [`ExecutionProvider::ALL`] order. `Auto` is a request rather than
    /// a provider, so it is not listed.
    pub fn available(self) -> Vec<ExecutionProvider> {
        ExecutionProvider::ALL
            .into_iter()
            .filter(|&provider| provider != ExecutionProvider::Auto && self.supports(provider))
            .collect()
    }
}

/// The rungs a session for `requested` is tried on, on a machine offering `supported`, in order.
///
/// - `Auto` expands in its place to every offered accelerator in [`AUTO_ORDER`].
/// - `CPU` ends the list: anything after it is ignored.
/// - Providers the machine does not offer are dropped, and so are repeats, keeping the first.
///
/// With the accelerators left written `a1 … an`, the rungs are `[a1 … an]`, `[a2 … an]`, and so on to `[an]`, then the
/// CPU alone (`[]`). Each rung keeps every accelerator below it attached, so a node one of them declines goes to the
/// next one rather than straight to the CPU. A rung never holds `Auto` or `CPU`, and the ladder always ends on `[]`.
///
/// ```
/// use rust_sak::ai::{ExecutionProvider::*, Supported, ladder};
///
/// let mac = Supported { cpu: true, coreml: true, cuda: false, tensorrt: false, webgpu: true };
/// assert_eq!(ladder(&[Auto], mac), [vec![CoreMl, WebGpu], vec![WebGpu], vec![]]);
/// assert_eq!(ladder(&[Cuda], mac), [vec![]]);
/// ```
pub fn ladder(requested: &[ExecutionProvider], supported: Supported) -> Vec<Vec<ExecutionProvider>> {
    let mut chain: Vec<ExecutionProvider> = Vec::new();
    for &provider in requested {
        let expanded: &[ExecutionProvider] = match provider {
            ExecutionProvider::Cpu => break,
            ExecutionProvider::Auto => &AUTO_ORDER,
            _ => std::slice::from_ref(&provider),
        };
        for &accelerator in expanded {
            if supported.supports(accelerator) && !chain.contains(&accelerator) {
                chain.push(accelerator);
            }
        }
    }

    let mut rungs: Vec<Vec<ExecutionProvider>> = (0..chain.len()).map(|from| chain[from..].to_vec()).collect();
    rungs.push(Vec::new());
    rungs
}

#[cfg(test)]
mod tests {
    use super::ExecutionProvider::{Auto, CoreMl, Cpu, Cuda, TensorRt, WebGpu};
    use super::*;

    fn machine(coreml: bool, cuda: bool, tensorrt: bool, webgpu: bool) -> Supported {
        Supported {
            cpu: true,
            coreml,
            cuda,
            tensorrt,
            webgpu,
        }
    }

    fn mac() -> Supported {
        machine(true, false, false, true)
    }

    fn nvidia() -> Supported {
        machine(false, true, true, false)
    }

    fn bare() -> Supported {
        machine(false, false, false, false)
    }

    #[test]
    fn names_are_read_whatever_their_case() {
        for text in ["coreml", "CoreML", "COREML"] {
            assert_eq!(text.parse(), Ok(CoreMl), "{text}");
        }
        for provider in ExecutionProvider::ALL {
            let name = provider.as_str();
            for text in [name.to_owned(), name.to_lowercase(), name.to_uppercase()] {
                assert_eq!(text.parse(), Ok(provider), "{text}");
            }
            assert_eq!(provider.to_string(), name);
        }
    }

    #[test]
    fn an_unknown_name_is_refused_with_the_six_names() {
        let error = "vulkan".parse::<ExecutionProvider>().unwrap_err();
        assert_eq!(error, UnknownProvider { name: "vulkan".into() });
        assert_eq!(
            error.to_string(),
            "unknown execution provider `vulkan`; expected one of Auto, CPU, CoreML, CUDA, TensorRT or WebGPU"
        );
        for text in ["", "cuda11", "CoreML "] {
            assert!(text.parse::<ExecutionProvider>().is_err(), "{text:?}");
        }
    }

    #[test]
    fn the_spellings_are_pinned() {
        assert_eq!(
            ExecutionProvider::ALL.map(ExecutionProvider::as_str),
            ["Auto", "CPU", "CoreML", "CUDA", "TensorRT", "WebGPU"]
        );
        assert_eq!(
            serde_json::to_string(&ExecutionProvider::ALL).unwrap(),
            r#"["Auto","CPU","CoreML","CUDA","TensorRT","WebGPU"]"#
        );
        for provider in ExecutionProvider::ALL {
            let json = serde_json::to_string(&provider).unwrap();
            assert_eq!(serde_json::from_str::<ExecutionProvider>(&json).unwrap(), provider);
        }
    }

    #[test]
    fn only_the_hardware_is_an_accelerator() {
        assert_eq!(
            ExecutionProvider::ALL.map(ExecutionProvider::is_accelerator),
            [false, false, true, true, true, true]
        );
    }

    #[test]
    fn a_mac_offers_cpu_coreml_and_webgpu() {
        assert_eq!(mac().available(), [Cpu, CoreMl, WebGpu]);
    }

    #[test]
    fn a_capable_nvidia_machine_offers_cpu_cuda_and_tensorrt() {
        assert_eq!(nvidia().available(), [Cpu, Cuda, TensorRt]);
    }

    #[test]
    fn a_machine_without_an_accelerator_offers_only_the_cpu() {
        assert_eq!(bare().available(), [Cpu]);
        assert!(bare().supports(Auto));
        assert!(bare().supports(Cpu));
    }

    #[test]
    fn detection_offers_the_cpu_coreml_on_macos_and_the_nvidia_providers_as_reported() {
        let detected = Supported::detect(false, false);
        assert!(detected.cpu);
        assert_eq!(detected.coreml, cfg!(target_os = "macos"));
        assert!(!detected.cuda && !detected.tensorrt);

        let both = Supported::detect(true, true);
        assert!(both.cuda && both.tensorrt);
        let cuda = Supported::detect(true, false);
        assert!(cuda.cuda && !cuda.tensorrt);
        assert!(!Supported::detect(false, true).tensorrt, "TensorRT runs on CUDA");
    }

    #[test]
    fn auto_on_a_mac_is_coreml_and_webgpu_then_webgpu_then_the_cpu() {
        assert_eq!(ladder(&[Auto], mac()), [vec![CoreMl, WebGpu], vec![WebGpu], vec![]]);
    }

    #[test]
    fn auto_on_a_capable_nvidia_machine_is_tensorrt_and_cuda_then_cuda_then_the_cpu() {
        assert_eq!(ladder(&[Auto], nvidia()), [vec![TensorRt, Cuda], vec![Cuda], vec![]]);
    }

    #[test]
    fn auto_follows_one_order_whatever_the_machine() {
        let everything = machine(true, true, true, true);
        assert_eq!(ladder(&[Auto], everything)[0], AUTO_ORDER);
        assert_eq!(ladder(&[Auto], bare()), [Vec::<ExecutionProvider>::new()]);
    }

    #[test]
    fn one_provider_by_name_is_not_widened() {
        assert_eq!(ladder(&[Cuda], nvidia()), [vec![Cuda], vec![]]);
    }

    #[test]
    fn a_provider_the_machine_does_not_offer_leaves_only_the_cpu() {
        assert_eq!(
            ladder(&[TensorRt], machine(false, true, false, false)),
            [Vec::<ExecutionProvider>::new()]
        );
        assert_eq!(ladder(&[CoreMl], nvidia()), [Vec::<ExecutionProvider>::new()]);
    }

    #[test]
    fn an_explicit_order_is_kept() {
        assert_eq!(
            ladder(&[WebGpu, CoreMl], mac()),
            [vec![WebGpu, CoreMl], vec![CoreMl], vec![]]
        );
    }

    #[test]
    fn the_cpu_ends_the_list() {
        assert_eq!(ladder(&[Cpu], mac()), [Vec::<ExecutionProvider>::new()]);
        assert_eq!(ladder(&[WebGpu, Cpu, CoreMl], mac()), [vec![WebGpu], vec![]]);
        assert_eq!(ladder(&[Cpu, Auto], mac()), [Vec::<ExecutionProvider>::new()]);
    }

    #[test]
    fn repeats_are_dropped_keeping_the_first() {
        assert_eq!(
            ladder(&[WebGpu, Auto], mac()),
            [vec![WebGpu, CoreMl], vec![CoreMl], vec![]]
        );
        assert_eq!(ladder(&[CoreMl, CoreMl], mac()), [vec![CoreMl], vec![]]);
    }

    #[test]
    fn the_ladder_is_never_empty_and_always_ends_on_the_cpu() {
        assert_eq!(ladder(&[], mac()), [Vec::<ExecutionProvider>::new()]);
        for provider in ExecutionProvider::ALL {
            for supported in [mac(), nvidia(), bare(), machine(true, true, true, true)] {
                let rungs = ladder(&[provider], supported);
                assert_eq!(rungs.last(), Some(&Vec::new()), "{provider} on {supported:?}");
                assert!(
                    rungs
                        .iter()
                        .flatten()
                        .all(|rung| rung.is_accelerator() && supported.supports(*rung)),
                    "{provider} on {supported:?}: {rungs:?}"
                );
            }
        }
    }
}
