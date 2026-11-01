# `ai` module

Installs what ONNX inference needs (the ONNX Runtime, and NVIDIA's CUDA, cuDNN and TensorRT on machines whose GPU can use them) from **Hugging Face folders the caller names**, verifies every download against the **hash the Hugging Face API reports**, loads the runtime once per process, and **builds sessions** on the best execution provider the machine offers, falling back towards the CPU when one cannot take a model.

Nothing in this module names a dependency, a version, a folder, a size, a hash, a model, a tuning or a place on disk. The caller passes each dependency as its folder URL, such as `https://huggingface.co/vegidio/general/tree/main/deps/cuda/13.3.0`, and the root folder to install under. A new version on Hugging Face needs only a new URL in the calling app.

## Enabling

The module is gated behind the `ai` Cargo feature, which turns on `crypto`, `fetch`, `fs` and `sysinfo` as well:

```toml
[dependencies]
rust-sak = { git = "https://github.com/vegidio/rust-sak", features = ["ai"] }
```

`install` is async and needs a **Tokio** runtime, as `fetch` does. `load` and the rest are synchronous.

### The `ort` pin

`ai` builds **`ort = "=2.0.0-rc.15"`** with no default features and `std`, `load-dynamic`, `api-30`, `tracing`, `coreml`, `cuda` and `tensorrt` (the last three only make the providers' types exist; WebGPU is a plugin and needs no feature). `download-binaries` and `copy-dylibs` stay off: the runtime comes from Hugging Face at run time, not from the build.

The pinned crate is re-exported as **`rust_sak::ai::ort`**, so a consumer names `Session`, tensors and values from it without depending on `ort` itself. A consumer that does depend on it must ask for the same version and API level, so that one `ort` is compiled.

Linking `ai` sets `ORT_DISABLE_TELEMETRY=1` before `main`, which is the only full opt-out of ONNX Runtime's telemetry uploader on macOS and Linux.

## Public API

| Item                                  | What it does                                                                                                                                                                                                                         |
|---------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `Source::parse(url)`                  | Parses a Hugging Face folder URL (`https://huggingface.co/<owner>/<repo>/tree/<revision>/<folder>`) into its repo, revision and folder. The folder's last two segments are the dependency's name and version.                        |
| `Dependency::runtime(name, source)`   | The ONNX Runtime. Installed only while its library is in its folder, and a platform its folder has no archive for fails the install.                                                                                                |
| `Dependency::libraries(name, source)` | Libraries no one file stands for, such as CUDA. Its marker alone decides, and a platform its folder has no archive for is skipped.                                                                                                   |
| `install(root, &deps, on_event)`      | Installs each dependency that is missing or out of date into `dirs::dir_for(root, source)`, in order, reporting `Event`s. Fails with an `InstallError` naming the dependency.                                                        |
| `is_present(root, &dep)`              | Local only: whether a dependency's marker (and, for the runtime, its library) is there. For deciding whether to load, with no network.                                                                                              |
| `load(runtime_dir)`                   | Loads the runtime installed in `runtime_dir` and starts the process's environment, its logs going to `tracing` at warning and above, then registers the WebGPU plugin beside it. Once per process: a later call returns at once. A failed load can be tried again. |
| `runtime_library(dir)`                | The runtime library in `dir`: `libonnxruntime*.dylib`, `libonnxruntime.so*` or `onnxruntime.dll`, never a `*_providers_*` plugin.                                                                                                   |
| `asset_for(listing, os, arch)`        | The one `*_<macos\|linux\|windows>_<arm64\|x64>.7z` in a folder's listing, for `std::env::consts` spellings.                                                                                                                        |
| `dirs::dir_for(root, source)`         | `<root>/<name>/<version>`.                                                                                                                                                                                                           |
| `gpu::meets(&GpuRequirement)`         | Whether this machine has an NVIDIA GPU whose driver supports at least `min_driver` (CUDA version) and whose first CUDA device has at least `min_compute` (compute capability). A probe that fails counts as no GPU, with a warning. |
| `gpu::meets_with(gpus, cuda, req)`    | The same comparison over probe results the caller already has. Pure.                                                                                                                                                                |
| `hf::Hub`                             | A Hugging Face client for one repository: `resolve(revision)` gives the commit it points at, `list(commit, folder)` gives each file's `size` and `hash`, and `url(commit, path)` builds the `resolve` URL.                          |
| `hf::Hash`                            | `Sha256` (the LFS hash) or `GitBlobSha1` (the object id of a file kept outside LFS), with `verify_file(path)`.                                                                                                                       |
| `ExecutionProvider`                   | `Auto`, `Cpu`, `CoreMl`, `Cuda`, `TensorRt`, `WebGpu`, spelled `Auto`, `CPU`, `CoreML`, `CUDA`, `TensorRT`, `WebGPU` (as Open Photo AI does) by `Display`, `as_str` and serde. `FromStr` ignores case; an unknown name is an `UnknownProvider` listing the six. |
| `Supported::detect(cuda, tensorrt)`   | What this machine offers: the CPU, CoreML on macOS, WebGPU when the load registered its plugin with a device, and CUDA and TensorRT as the caller reports them (TensorRT only with CUDA). `supports(provider)` and `available()`. |
| `ladder(&requested, supported)`       | The rungs a session is tried on. Pure. See "Sessions".                                                                                                                                                                               |
| `Tuning`                              | A model's tuning over the providers' baseline. `Tuning::default()` changes nothing.                                                                                                                                                 |
| `provider_options(p, &tuning, &paths, device_memory)` | The options provider `p` is attached with: its baseline, then the tuning. Pure.                                                                                                                                     |
| `trt_workspace_bytes(device_memory)`  | TensorRT's workspace: a quarter of the card, at most 4 GiB, and 4 GiB when the memory is unknown.                                                                                                                                  |
| `CachePaths::under(cache_dir)`        | The provider caches under a model's folder: `tensorrt/`, `tensorrt/timing/` and `coreml/`.                                                                                                                                         |
| `session(&SessionRequest)`            | Builds a session on the first rung that works, returning `Built { session, providers }`, or a `SessionError { model, reason }` with the CPU's reason.                                                                              |
| `libpath::prepare(&dirs, guard)`      | Puts the library folders on the loader's search path. `unsafe`: the first statement of `main`. See "Library path".                                                                                                                 |
| `libpath::search_path(&dirs, inherited)` | `dirs`, then the inherited entries, with repeats and empty entries removed. Pure.                                                                                                                                              |

### Events and errors

`on_event` receives `Event::Progress { dependency, fraction }` and `Event::Verifying { dependency }`. A dependency's fraction runs from 0 to 1: the download fills 0 to 0.8 (a resumed partial counted from the first report), and the extraction fills 0.8 to 1. It never decreases, and is exactly 1 only once the marker is written. A dependency that is already current reports nothing.

`InstallError { dependency, failure }` displays as `could not install NVIDIA cuDNN: the server answered with HTTP 404`. `Failure` is one of `Network`, `Http(status)`, `DiskFull`, `Io(reason)`, `Damaged`, `UnsupportedPlatform { os, arch }` and `Malformed(reason)`.

## How an install works

For each dependency, in the order given:

1. The source's revision (`main`) is resolved to a commit, once per repository per call. Everything after uses that commit, so a file re-uploaded mid-install cannot look damaged.
2. The folder is listed at that commit, and the archive for this platform is picked from the listing.
3. The dependency is **current** when its marker holds the hash the API reports now (and, for the runtime, its library is a file). It is skipped, with no event.
4. Otherwise the archive is downloaded into `.staging/` from `resolve/<commit>/<path>`, resuming a partial whose hash is still the API's, even after `main` moved to a new commit. It is verified against the API's hash, extracted in staging with every entry treated as untrusted, and moved into place: the marker deleted first, every other entry but staging removed, the extracted entries moved in, and the marker written last.

When the API cannot be reached (no network, a timeout, or a 5xx after the retries), a dependency that `is_present` is used as it is, with a warning. Only a missing install needs the network. Every request retries transient failures 5 times with Fibonacci backoff, and times out after 30 seconds of connecting or of silence.

Dropping the `install` future stops it. A download keeps its partial to resume from. An extraction that has begun runs to its end on its blocking thread, so a marker never stands over an incomplete install.

### On disk

```text
<root>/<name>/<version>/                     installed: the archive's entries, at the paths it gives them
<root>/<name>/<version>/.installed           the archive's verified hash: written last, deleted first
<root>/<name>/<version>/.staging/<asset>     the archive (and the download's .part and sidecar while it downloads)
<root>/<name>/<version>/.staging/extract/    this attempt's extraction
```

Apps that pass the same root share the installs, and different versions of a dependency live side by side.

### One version per folder

Every app that installs from a folder accepts whatever archive the API reports there, including an archive re-uploaded after the app was released. **A version folder on Hugging Face must only ever hold that version.** Putting 1.31 bytes in `deps/onnx/1.30.0/` would break every released app on its next start. Publish a new version in a new folder, and point new builds at its URL.

The hash comes from the same host as the file, so verification catches corruption, truncation and a re-upload during a resume, not a compromised repository. Integrity rests on HTTPS to Hugging Face and on control of the repository.

## Usage

```rust,no_run
use std::path::Path;

use rust_sak::ai::{self, Dependency, Event, Source, gpu::GpuRequirement};

const GPU: GpuRequirement = GpuRequirement { min_driver: (13, 0), min_compute: (7, 5) };

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let folder = |path: &str| Source::parse(&format!("https://huggingface.co/vegidio/general/tree/main/{path}"));
let runtime = folder("deps/onnx/1.30.0")?;

let mut dependencies = vec![Dependency::runtime("ONNX Runtime", runtime.clone())];
if ai::gpu::meets(&GPU) {
    dependencies.push(Dependency::libraries("NVIDIA CUDA", folder("deps/cuda/13.3.0")?));
    dependencies.push(Dependency::libraries("NVIDIA cuDNN", folder("deps/cudnn/9.23.1")?));
    dependencies.push(Dependency::libraries("NVIDIA TensorRT", folder("deps/tensorrt/10.14.1")?));
}

let root = Path::new("/path/the/app/chose/io.vinicius.ai");
ai::install(root, &dependencies, |event| {
    if let Event::Progress { dependency, fraction } = event {
        println!("{dependency}: {:.0}%", fraction * 100.0);
    }
})
.await?;

ai::load(&ai::dirs::dir_for(root, &runtime))?;
# Ok(())
# }
```

The Hugging Face client works on its own too:

```rust,no_run
use rust_sak::ai::hf::Hub;

# async fn run() -> Result<(), rust_sak::ai::hf::HfError> {
let hub = Hub::new("vegidio/general");
let commit = hub.resolve("main").await?;
for file in hub.list(&commit, "siglip2").await? {
    println!("{} {} bytes, {:?}, from {}", file.path, file.size, file.hash, hub.url(&commit, &file.path));
}
# Ok(())
# }
```

## Sessions

A session is asked for with an **ordered list of providers**, which becomes a ladder of attempts:

1. `Auto` expands in its place to every offered accelerator in the order TensorRT, CUDA, CoreML, WebGPU (`AUTO_ORDER`).
2. `CPU` ends the list: anything after it is ignored.
3. Providers the machine does not offer are dropped, and so are repeats, keeping the first.
4. With the accelerators left written `a1 … an`, the rungs are `[a1 … an]`, `[a2 … an]`, … `[an]`, then the CPU alone (`[]`).

Each rung keeps every accelerator below it attached, so a node one of them declines goes to the next accelerator, not to the CPU. A Mac's `Auto` is `[CoreML, WebGPU]`, `[WebGPU]`, then the CPU; asking for `CUDA` alone gives `[CUDA]`, then the CPU, and never attaches TensorRT. The ladder always ends on the CPU, so a session builds whenever the model file is valid.

`session` tries each rung on a fresh builder, every provider attached with `error_on_failure()`, so a provider that rejects a setting fails its rung instead of being quietly dropped. A failed rung is logged with `tracing::warn!` (the model, the providers and the reason) and the next one is tried. `Built::providers` is the rung that worked; an empty list means the CPU. The build runs on the calling thread and needs the runtime loaded.

### Tuning and the baseline

Each provider is attached with its **baseline**, then the caller's `Tuning` for that model, which can override it. The baseline describes the provider, so it lives here; the tuning describes the model, so each app passes its own (the fields of Open Photo AI's `EpProfile`):

| Provider | Baseline                                                                                                                                                                                                  | `Tuning` fields                                     |
|----------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|-----------------------------------------------------|
| CoreML   | `ModelFormat=MLProgram`, `RequireStaticInputShapes=1`, `EnableOnSubgraphs=0`, `ModelCacheDirectory=<cache>/coreml`, `MLComputeUnits=ALL`, `SpecializationStrategy=Default`                                | `coreml_compute_units`, `coreml_specialization`     |
| CUDA     | device 0, `enable_cuda_graph=0`, `cudnn_conv_algo_search=EXHAUSTIVE`, `cudnn_conv_use_max_workspace=1`, `do_copy_in_default_stream=1`, `gpu_mem_limit=0`, `prefer_nhwc=0`                                | `cuda_prefer_nhwc`                                  |
| TensorRT | device 0, FP32 (`trt_fp16_enable=0`, `trt_int8_enable=0`), `trt_cuda_graph_enable=0`, engine and timing caches on under `<cache>/tensorrt`, `trt_engine_hw_compatible=0`, `trt_builder_optimization_level=5`, `trt_max_workspace_size` from `trt_workspace_bytes` | `trt_options`, a raw overlay applied last |
| WebGPU   | the plugin's own defaults                                                                                                                                                                                 | `webgpu_prefer_nchw`, `webgpu_force_cpu_nodes`      |
| session  | parallel execution, memory planner on, no optimizer disabled                                                                                                                                              | `execution_mode`, `disable_mem_pattern`, `disabled_optimizers` (joined by `;`) |

Graph capture is off on both NVIDIA providers: with it on, TensorRT returns zeros from a session's second run, and CUDA fails the capture. TensorRT's workspace is sized from `sysinfo::cuda_info`'s first device. Precision is not a tuning concept: an app with FP16 and FP32 variants passes each its own tuning.

### Cache folders

`SessionRequest::cache_dir` is one folder per model, from the caller. CoreML keeps its compiled model in `<cache_dir>/coreml/`, and TensorRT its engines in `<cache_dir>/tensorrt/` and its timing cache in `<cache_dir>/tensorrt/timing/`. They are created when a rung that compiles is tried. CoreML keys its cache by the model's path, so **key the folder by the model file's content** (its hash), or a re-downloaded model at the same path can run on a stale compile.

### WebGPU

WebGPU ships inside the runtime's archive as a plugin (`libonnxruntime_providers_webgpu.dylib`, `.so`, or `onnxruntime_providers_webgpu.dll`). `load` registers it after the environment starts, when `sysinfo::is_webgpu_supported()` says the machine can run WebGPU (on Linux, a Vulkan GPU driver), and `Supported::detect` offers WebGPU when it offered a device. Sessions attach it through its first device. A machine without WebGPU, a missing plugin, a refused one or one with no device leaves WebGPU unoffered, with the reason logged; the load never fails because of it. Each Vulkan device is logged first, which is what makes a WebGPU failure on a two-GPU Linux machine readable.

### Usage

```rust,no_run
use std::path::Path;

use rust_sak::ai::{self, ExecutionProvider, SessionRequest, Supported, Tuning};

# fn run(runtime_dir: &Path, cuda_installed: bool, tensorrt_installed: bool) -> Result<(), Box<dyn std::error::Error>> {
ai::load(runtime_dir)?;
let supported = Supported::detect(cuda_installed, tensorrt_installed);

let requested: ExecutionProvider = "auto".parse()?;
let tuning = Tuning::default();
let built = ai::session(&SessionRequest {
    model: Path::new("/models/scrfd.onnx"),
    providers: &[requested],
    supported,
    tuning: &tuning,
    cache_dir: Path::new("/cache/scrfd/0123456789abcdef"),
})?;
println!("ready on {:?}", built.providers);
let _session: ai::ort::session::Session = built.session;
# Ok(())
# }
```

## Library path

The NVIDIA libraries are installed in their own folders, which the platform's loader does not search. `libpath::prepare(&dirs, guard)` puts `dirs` (the runtime's folder and the three NVIDIA folders) ahead of the search path, creating any that are missing, since a loader skips an entry missing at start for the rest of the process:

- **Linux**: glibc reads `LD_LIBRARY_PATH` only when the process starts, so `prepare` sets it and **restarts the process once** with the same arguments, with the `guard` variable set to `1`, which stops the restarted process from doing it again. Under WSL2 with an NVIDIA GPU, the active Windows driver's folder, found by initialising CUDA and reading `/proc/self/maps`, goes after the app's own, so the driver's own PTX compiler wins. A restart that fails is printed to stderr and the run goes on without the GPU libraries.
- **Windows**: `LoadLibrary` reads `PATH` when it is called, so the folders are prepended to it.
- **macOS**: nothing, and nothing is created.

Call it as the **first statement of `main`**, before logging is set up and before any thread exists: it is `unsafe` because it writes the environment and, on Linux, replaces the process image.

```rust,no_run
use std::path::PathBuf;

fn main() {
    let dirs: Vec<PathBuf> = vec![/* the runtime's folder, then CUDA's, cuDNN's and TensorRT's */];
    // SAFETY: the first statement of `main`, before any other thread exists.
    if let Err(cause) = unsafe { rust_sak::ai::libpath::prepare(&dirs, "MYAPP_REEXEC") } {
        eprintln!("the library folders could not be created: {cause}");
    }
    // ...
}
```

## Platform notes

- The archives exist for macOS arm64, Linux arm64/x64 and Windows arm64/x64 (the runtime), and Linux arm64/x64 and Windows x64 (the NVIDIA libraries). Intel Macs have no runtime archive. Finding that out takes the folder's listing, so the unsupported-platform failure comes after one API request.
- **Only macOS is tested.** The GPU paths (the NVIDIA probe passing, and the CUDA, cuDNN and TensorRT installs) have never run on real hardware. Their logic is covered by pure tests against constructed probe results, and the platform-specific code is written to be correct by construction, but it is **untested on Linux and Windows**.
- On macOS, the ladder, the option maps and the search path are tested as pure functions, and CoreML, WebGPU and CPU sessions are built for real. **CUDA and TensorRT sessions, the Linux restart, the WSL2 driver folder and the Windows `PATH` step have never run**: they are written to be correct by construction, after Open Photo AI's code, and are untested.
- CoreML is offered on every macOS the runtime loads on, with no OS-version probe: the 1.30 runtime's macOS build already needs a newer macOS than CoreML's ML Program format (12). Were that wrong, the CoreML rung would fail and the ladder would move on.
