# `ai` module

Installs what ONNX inference needs (the ONNX Runtime, and NVIDIA's CUDA, cuDNN and TensorRT on machines whose GPU can use them, and the models themselves) from **Hugging Face folders the caller names**, verifies every download against the **hash the Hugging Face API reports**, loads the runtime once per process, and **builds sessions** on the best execution provider the machine offers, falling back towards the CPU when one cannot take a model.

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

| Item                                                       | What it does                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
|------------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `Folder::parse(url)`                                       | Parses a Hugging Face folder URL (`https://huggingface.co/<owner>/<repo>/tree/<revision>/<folder>`) whose folder has one segment or more, as in `models` or `models/insightface`, into its repo, revision and path.                                                                                                                                                                                                                                                         |
| `Listing::read(&folder)`                                   | Resolves the folder's revision to a commit, then lists every page of the folder at that commit: each file's `size` and `hash`, and the commit every download from it uses. A file the API reports no hash for is left out. Read once and passed to every `install_files` in that folder.                                                                                                                                                                                    |
| `Files { name, folder, select, dir, derived, trust }`      | One file install: the files of `folder` that `select` takes (`Select::Names` or `Select::Prefix`), installed into `dir`, with the folders computed from them in `derived`. See "How a file install works".                                                                                                                                                                                                                                                                  |
| `install_files(&files, Some(&listing) \| None, on_event)`  | Installs `files` against a listing, or, with none, uses an intact install as it is. Returns an `Outcome`: `Installed`, `Current`, `Adopted`, `Offline` or `Trusted`.                                                                                                                                                                                                                                                                                                        |
| `Listing::read_with(hub, &folder)`                         | `Listing::read` through a `Hub::for_folder(&folder)` configured by the caller, such as `.retries(0)` for a test of an unreachable or failing server. A hub of another repository is refused. The listing keeps its hub, and every `install_files` against it downloads through that hub's client, with its retries.                                                                                                                                                         |
| `files_present(&files)`                                    | Local only: whether `files.dir` holds an intact install (a record of verified files, each still there at its recorded size). What `install_files` uses with no listing. No network.                                                                                                                                                                                                                                                                                         |
| `Source::parse(url)`                                       | Parses a Hugging Face folder URL (`https://huggingface.co/<owner>/<repo>/tree/<revision>/<folder>`) into its repo, revision and folder. The folder's last two segments are the dependency's name and version.                                                                                                                                                                                                                                                               |
| `Dependency::runtime(name, source)`                        | The ONNX Runtime. Installed only while its library is in its folder, and a platform its folder has no archive for fails the install.                                                                                                                                                                                                                                                                                                                                        |
| `Dependency::libraries(name, source)`                      | Libraries no one file stands for, such as CUDA. Its marker alone decides, and a platform its folder has no archive for is skipped.                                                                                                                                                                                                                                                                                                                                          |
| `plan(root, &deps)`                                        | What an install will do, before anything moves: a `Planned { name, state, size }` per dependency, in order. `Present` (intact, sized from its marker, no network), `Missing` (sized from its folder's listing) or `Unavailable` (libraries with no archive for this platform).                                                                                                                                                                                              |
| `Plan::install(on_event)`                                  | Installs each `Missing` dependency into `dirs::dir_for(root, source)`, in order, from the plan's listing, reporting `Event`s. Fails with an `InstallError` naming the dependency.                                                                                                                                                                                                                                                                                           |
| `install(root, &deps, on_event)`                           | `plan`, then `Plan::install`.                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| `is_present(root, &dep)`                                   | Local only: whether a dependency is installed and intact (its marker is there, every file it records is there at its recorded size, and, for the runtime, its library is a file). No network.                                                                                                                                                                                                                                                                               |
| `load(runtime_dir)`                                        | `load_with` and `LoadOptions::default()`: the environment named `rust-sak`, its records going to `tracing`.                                                                                                                                                                                                                                                                                                                                                                 |
| `load_with(runtime_dir, &LoadOptions { name, logger })`    | Loads the runtime installed in `runtime_dir` and starts the process's environment under `name`, its records going to `logger` (or to `tracing`, in `ort`'s own shape, when `None`) at warning and above, each one counted first if it reports a failed device allocation (see "Out of memory"), then registers the WebGPU plugin beside it. The log level drops to fatal at process exit. Once per process: a later call returns at once. A failed load can be tried again. |
| `environment()`                                            | The process's environment once a load succeeded, the one every session is built under. It lives until the process exits.                                                                                                                                                                                                                                                                                                                                                    |
| `webgpu_devices(&environment)`                             | The devices the WebGPU plugin registered with the environment, in the runtime's order.                                                                                                                                                                                                                                                                                                                                                                                      |
| `runtime_library(dir)`                                     | The runtime library in `dir`: `libonnxruntime*.dylib`, `libonnxruntime.so*` or `onnxruntime.dll`, never a `*_providers_*` plugin.                                                                                                                                                                                                                                                                                                                                           |
| `asset_for(listing, os, arch)`                             | The one `*_<macos\|linux\|windows>_<arm64\|x64>.7z` in a folder's listing, for `std::env::consts` spellings.                                                                                                                                                                                                                                                                                                                                                                |
| `dirs::dir_for(root, source)`                              | `<root>/<name>/<version>`.                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| `gpu::check(&GpuRequirement)`                              | Whether this machine has an NVIDIA GPU whose driver supports at least `min_driver` (CUDA version) and whose first CUDA device has at least `min_compute` (compute capability), and if not, the `GpuRefusal`: `NoAdapter`, `NoDriver`, `DriverTooOld { driver, required }`, `NoDevice { driver }`, `DeviceTooOld { name, compute, required }` or `ProbeFailed(reason)`, each displaying as a sentence for a log.                                                             |
| `gpu::meets(&GpuRequirement)`                              | `check(..).is_ok()`.                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| `gpu::check_with(gpus, cuda, req)` / `gpu::meets_with(..)` | The same comparison over probe results the caller already has. Pure.                                                                                                                                                                                                                                                                                                                                                                                                        |
| `hf::Hub`                                                  | A Hugging Face client for one repository: `resolve(revision)` gives the commit it points at, `list(commit, folder)` gives each file's `size` and `hash`, and `url(commit, path)` builds the `resolve` URL.                                                                                                                                                                                                                                                                  |
| `hf::Hash`                                                 | `Sha256` (the LFS hash) or `GitBlobSha1` (the object id of a file kept outside LFS), with `verify_file(path)`.                                                                                                                                                                                                                                                                                                                                                              |
| `ExecutionProvider`                                        | `Auto`, `Cpu`, `CoreMl`, `Cuda`, `TensorRt`, `WebGpu`, spelled `Auto`, `CPU`, `CoreML`, `CUDA`, `TensorRT`, `WebGPU` (as Open Photo AI does) by `Display`, `as_str` and serde. `FromStr` ignores case; an unknown name is an `UnknownProvider` listing the six.                                                                                                                                                                                                             |
| `Supported::detect(cuda, tensorrt)`                        | What this machine offers: the CPU, CoreML on macOS, WebGPU when the load registered its plugin with a device, and CUDA and TensorRT as the caller reports them (TensorRT only with CUDA). `supports(provider)` and `available()`.                                                                                                                                                                                                                                           |
| `ladder(&requested, supported)`                            | The rungs a session is tried on. Pure. See "Sessions".                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `Tuning`                                                   | A model's tuning over the providers' baseline. `Tuning::default()` changes nothing.                                                                                                                                                                                                                                                                                                                                                                                         |
| `provider_options(p, &tuning, &paths, device_memory)`      | The options provider `p` is attached with: its baseline, then the tuning. Pure.                                                                                                                                                                                                                                                                                                                                                                                             |
| `trt_workspace_bytes(device_memory)`                       | TensorRT's workspace: a quarter of the card, at most 4 GiB, and 4 GiB when the memory is unknown.                                                                                                                                                                                                                                                                                                                                                                           |
| `CachePaths { tensorrt, timing, coreml }`                  | The folders the providers compile into. `CachePaths::under(cache_dir)` puts them under a model's folder: `tensorrt/`, `tensorrt/timing/` and `coreml/`. Override `timing` to share one timing cache between models.                                                                                                                                                                                                                                                         |
| `build(&SessionRequest, &rung)`                            | Builds a session on exactly one rung, on the calling thread, returning the `Session`, or a `RungError { providers, reason, out_of_memory }`. An empty rung is the CPU. See "Sessions".                                                                                                                                                                                                                                                                                      |
| `session(&SessionRequest)`                                 | `ladder` and `build` on each rung until one works, returning `Built { session, providers }`, or a `SessionError { model, reason }` with the CPU's reason.                                                                                                                                                                                                                                                                                                                   |
| `libpath::prepare(&dirs, guard)`                           | Puts the library folders on the loader's search path. `unsafe`: the first statement of `main`. See "Library path".                                                                                                                                                                                                                                                                                                                                                          |
| `libpath::search_path(&dirs, inherited)`                   | `dirs`, then the inherited entries, with repeats and empty entries removed. Pure.                                                                                                                                                                                                                                                                                                                                                                                           |

### Events and errors

`on_event` receives:

- `Event::Progress { dependency, phase, bytes, total, fraction }`. `phase` is `Phase::Download` (`bytes` on disk of the archive's `total`, a resumed partial included) or `Phase::Extract` (`bytes` written of the archive's declared `total`, 0 when it declares none); every download report comes before the first extraction one. A dependency's fraction runs from 0 to 1: the download fills 0 to 0.8 (a resumed partial counted from the first report), and the extraction fills 0.8 to 1. It never decreases, and is exactly 1 only once the marker is written.
- `Event::Verifying { dependency }`, while a complete archive found in staging is hashed.
- `Event::Current { dependency }`, once for each dependency that is installed and intact, including one another process finished while this one waited for its lock.

`InstallError { dependency, failure }` displays as `could not install NVIDIA cuDNN: the server answered with HTTP 404`. `Failure` is one of `Network`, `Http(status)`, `DiskFull`, `Io(reason)`, `Damaged { path, expected, actual }` (the file's path in the repository, and the API's and the download's hashes), `UnsupportedPlatform { os, arch }`, `Malformed(reason)`, and, for file installs, `NotListed` (the listing names no file for the selection) and `Unlisted` (no listing, and nothing intact to use).

An event's and an error's `dependency` is a `Cow<'static, str>`: a dependency's `&'static str` name, or a file install's `Files::name`, which may be built at run time. **Breaking in 26.11.0:** it was `&'static str`; match on it with `dependency.as_ref()` or compare it to a `&str`. `Failure::Damaged` gained its fields in the same release.

## How an install works

**The plan**, for each dependency, in the order given:

1. **Local first.** A dependency whose folder holds a marker, with every file the marker records still there at its recorded size (a link checked as a link, so the `libfoo.so -> libfoo.so.1` chains of the Linux archives are not read as corruption), and, for the runtime, its library, is `Present`. Nothing is asked of the network, so a launch on which everything is installed makes **no request at all**, online or not. The check is a `stat` per file, never a hash: the contents were verified when they were installed.
2. Otherwise the source's revision (`main`) is resolved to a commit, once per repository per call. Everything after uses that commit, so a file re-uploaded mid-install cannot look damaged.
3. The folder is listed at that commit, and the archive for this platform is picked from the listing. The dependency is `Missing`, sized from the listing.

**The install**, for each `Missing` dependency, in order:

1. The folder's **lock** is taken: the operating system's advisory lock on `<folder>.lock`, beside the folder and never removed, so the system releases it when the process holding it ends, however it ends. A process that finds it held waits, then checks the folder again, and reports `Current` for an install the other process finished rather than repeating it.
2. The archive is downloaded into `.staging/` from `resolve/<commit>/<path>`, resuming a partial whose hash is still the API's, even after `main` moved to a new commit. It is verified against the API's hash, extracted in staging with every entry treated as untrusted, and moved into place: the marker deleted first, every other entry but staging removed, the extracted entries moved in, and the marker, recording the archive's hash and size and every file with its size, written last.
3. The dependency's **other versions** (the other folders under `<root>/<name>/`) are removed, the marker of each first. One whose lock another process holds is left in place. A removal that fails is logged and does not fail the install.

Only a missing or damaged install needs the network. Every request retries transient failures 5 times with Fibonacci backoff, and times out after 30 seconds of connecting or of silence.

Dropping the `install` future stops it and releases the lock. A download keeps its partial to resume from. An extraction that has begun runs to its end on its blocking thread, so a marker never stands over an incomplete install.

### On disk

```text
<root>/<name>/<version>.lock                 locked by the process installing into the folder; never removed
<root>/<name>/<version>/                     installed: the archive's entries, at the paths it gives them
<root>/<name>/<version>/.installed           JSON: the archive's verified hash and size, and every file it produced
                                             with its size: written last, deleted first
<root>/<name>/<version>/.staging/<asset>     the archive (and the download's .part and sidecar while it downloads)
<root>/<name>/<version>/.staging/extract/    this attempt's extraction
```

Apps that pass the same root share the installs: a runtime one app installed is used by the next with no download. A root holds one version of each dependency once an install of it finishes, so **apps sharing a root should pin the same versions**: two apps pinning different ones replace each other's on every install.

### One version per folder

Every app that installs from a folder accepts whatever archive the API reports there, including an archive re-uploaded after the app was released. **A version folder on Hugging Face must only ever hold that version.** Putting 1.31 bytes in `deps/onnx/1.30.0/` would break every released app on its next start. Publish a new version in a new folder, and point new builds at its URL.

The hash comes from the same host as the file, so verification catches corruption, truncation and a re-upload during a resume, not a compromised repository. Integrity rests on HTTPS to Hugging Face and on control of the repository.

## How a file install works

An archive's version folder only ever holds one version, so the archive installer trusts a present install with no request. A **model folder is different**: it holds many models, and a model is republished at the same path when it changes. `install_files` is built for that:

- **Selection.** `Select::Names(names)` takes exactly those files, every one of which the listing must name. `Select::Prefix(p)` takes every file whose name starts with `p`, so `x.onnx` takes `x.onnx` and a sibling `x.onnx.data` holding its external weights, and not `xy.onnx`. Only the files directly inside the folder are listed.
- **One listing, many installs.** The caller reads `Listing::read(&folder)` once and passes it to each install in the folder. The listing pins the commit, so every file of one listing comes from one revision. A listing of another folder is refused as `Malformed`.
- **Republish-aware.** With a listing, an install is `Current` only when its record names exactly the selected files, each recorded hash equals the listing's, and each file is still on disk at its recorded size (a `stat`, never a hash). Anything else installs.
- **No listing.** An install whose record of verified files is intact is `Offline`: used as it is, with no request. Anything else fails as `Unlisted`, with nothing transferred, since nothing could verify it.

**The install**, under the folder's lock (`<dir>.lock`, beside it, as for archives):

1. The record is deleted, and every `derived` folder is emptied, the folder itself kept. Only then can a byte move, so an interruption never leaves new files beside what was compiled from old ones.
2. Each selected file already in `dir` at its listed size is kept when the record being replaced vouched for its hash, or else when it hashes to the listing's (`Event::Verifying`). This **adopts in place** a folder filled by an earlier release, or by hand, with nothing downloaded. Any other file is downloaded from `resolve/<commit>/<path>` into `<dir>/.staging/`, resuming a partial whose hash is still the listing's, verified, and renamed into `dir`. A file that does not match after a resume is downloaded once more from nothing; one that still does not, or a clean download that does not, is deleted and fails as `Damaged`.
3. Every entry of `dir` that is neither selected nor the installer's own is removed, which drops a file the listing no longer names and an earlier release's own bookkeeping.
4. The record, each file's name, size and hash, is written last.

The outcome is `Adopted` when every file was kept and `Installed` when any was downloaded. Progress is `Phase::Download` over the sum of the selected files' sizes; there is no `Extract` phase. The fraction never decreases, and is exactly 1 only after the record is written. Nothing is reported for `Current`, `Offline` or `Trusted`. Dropping the future stops the install and releases the lock; a download keeps its partial in staging to resume from.

**Trust.** Under `Trust::LocalFiles`, a folder holding every selected file (by the names given, else the listing's selection, else the recorded names, else the folder's own files the prefix takes) is used as it is, **with no hash and no request**, and the outcome is `Trusted`. It exists for running a model that has been re-exported and not yet published. Its identity is a stamp, SHA-256 over each file's name, size and modification time; when the stamp moves, the derived folders are emptied once and the new stamp recorded. A trusted record is never an intact install, so an install without trust (or with no listing) never uses it unchecked: it reinstalls, adopting whatever happens to match. A folder missing a selected file installs as under `Trust::Published`, every download verified.

```text
<dir>.lock                   locked by the process installing into the folder; never removed
<dir>/<file>                 installed: the selected files, by name
<dir>/.installed             JSON: {"kind":"files","files":[{path,size,hash}]} or {"kind":"trusted","stamp"}: written last,
                             deleted first
<dir>/.staging/<file>        a download in flight (and its .part and sidecar)
```

```rust,no_run
use std::path::Path;

use rust_sak::ai::{self, Event, Files, Folder, Listing, Outcome, Select, Trust};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let folder = Folder::parse("https://huggingface.co/vegidio/open-photo-ai/tree/main/models")?;
let listing = Listing::read(&folder).await.ok();
let config = Path::new("/path/the/app/chose");
for model in ["up_kyoto_4x_fp32", "dn_stockholm_fp32"] {
    let files = Files {
        name: model.into(),
        folder: folder.clone(),
        select: Select::Prefix(format!("{model}.onnx")),
        dir: config.join("models").join(model),
        derived: vec![config.join("engines").join(model)],
        trust: Trust::Published,
    };
    let outcome = ai::install_files(&files, listing.as_ref(), |event| {
        if let Event::Progress { bytes, total, .. } = event {
            println!("{model}: {bytes} of {total} bytes");
        }
    })
    .await?;
    if outcome == Outcome::Offline {
        println!("{model}: the listing could not be read; using the installed files");
    }
}
# Ok(())
# }
```

## Usage

```rust,no_run
use std::path::Path;

use rust_sak::ai::{self, Dependency, Event, LoadOptions, Phase, Source, gpu::GpuRequirement};

const GPU: GpuRequirement = GpuRequirement { min_driver: (13, 0), min_compute: (7, 5) };

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let folder = |path: &str| Source::parse(&format!("https://huggingface.co/vegidio/general/tree/main/{path}"));
let runtime = folder("deps/onnx/1.30.0")?;

let mut dependencies = vec![Dependency::runtime("ONNX Runtime", runtime.clone())];
match ai::gpu::check(&GPU) {
    Ok(()) => {
        dependencies.push(Dependency::libraries("NVIDIA CUDA", folder("deps/cuda/13.3.0")?));
        dependencies.push(Dependency::libraries("NVIDIA cuDNN", folder("deps/cudnn/9.23.1")?));
        dependencies.push(Dependency::libraries("NVIDIA TensorRT", folder("deps/tensorrt/10.14.1")?));
    }
    Err(refusal) => println!("no NVIDIA libraries: {refusal}"),
}

let root = Path::new("/path/the/app/chose/io.vinicius.ai");
let plan = ai::plan(root, &dependencies).await?;
for row in plan.dependencies() {
    println!("{}: {:?}, {} bytes", row.name, row.state, row.size);
}
plan.install(|event| match event {
    Event::Progress { dependency, phase: Phase::Download, bytes, total, .. } => {
        println!("{dependency}: {bytes} of {total} bytes");
    }
    Event::Current { dependency } => println!("{dependency} is already installed"),
    _ => {}
})
.await?;

let options = LoadOptions { name: "io.vinicius.myapp", ..LoadOptions::default() };
ai::load_with(&ai::dirs::dir_for(root, &runtime), &options)?;
let _environment = ai::environment().expect("loaded");
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

`build` tries one rung on a fresh builder, every provider attached with `error_on_failure()`, so a provider that rejects a setting fails its rung instead of being quietly dropped. It runs **on the calling thread**, as a contract: the runtime logs a build's records there, so a span the caller enters around it decorates them, and it needs the runtime loaded. `Auto` and `CPU` inside a rung are ignored; `request.providers` and `request.supported` are not read, since the caller chose the rung.

`session` is `ladder` and a loop over `build`: a failed rung is logged with `tracing::warn!` (the model, the providers and the reason) and the next one is tried. `Built::providers` is the rung that worked; an empty list means the CPU. An app whose policy sits between rungs (skipping one it has seen fail, retrying one after freeing memory) walks `ladder` itself and calls `build`.

### Out of memory

`RungError::out_of_memory` says whether the device ran out of memory during the build, the one failure that freeing other sessions' memory can cure. TensorRT's own error often says only that it *"failed to create engine"*; the cause is in a record logged on the way. So `load_with` always starts the environment with this module's logger, which counts each record reporting a failed device allocation (`OutOfMemory`, `out of memory`) on the thread that logged it, then passes the record on to `LoadOptions::logger` unchanged. A rung is out of memory when that count moved on the building thread during the build, or its error names a failed allocation (`out of memory`, `outofmemory`, `failed to allocate`, ignoring case). The CPU rung never is.

### TensorRT builds

Every `build` whose rung contains TensorRT holds one process-wide lock for the whole build, so two TensorRT builds never interleave into a shared timing cache (the runtime reads it at the start and rewrites it at the end with no locking of its own). A poisoned lock is taken anyway. When such a build fails, the timing folder is emptied, the folder itself kept, and an `info` record names its path: a half-written timing cache fails every later TensorRT build. A rung without TensorRT neither waits nor discards. The lock is in-process only, so two processes must not share a timing folder. The CUDA device's memory, which sizes TensorRT's workspace, is probed once per process, by the first TensorRT rung.

### Tuning and the baseline

Each provider is attached with its **baseline**, then the caller's `Tuning` for that model, which can override it. The baseline describes the provider, so it lives here; the tuning describes the model, so each app passes its own (the fields of Open Photo AI's `EpProfile`):

| Provider | Baseline                                                                                                                                                                                                                                                          | `Tuning` fields                                                                |
|----------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------|
| CoreML   | `ModelFormat=MLProgram`, `RequireStaticInputShapes=1`, `EnableOnSubgraphs=0`, `ModelCacheDirectory=<cache>/coreml`, `MLComputeUnits=ALL`, `SpecializationStrategy=Default`                                                                                        | `coreml_compute_units`, `coreml_specialization`                                |
| CUDA     | device 0, `enable_cuda_graph=0`, `cudnn_conv_algo_search=EXHAUSTIVE`, `cudnn_conv_use_max_workspace=1`, `do_copy_in_default_stream=1`, `gpu_mem_limit=0`, `prefer_nhwc=0`                                                                                         | `cuda_prefer_nhwc`                                                             |
| TensorRT | device 0, FP32 (`trt_fp16_enable=0`, `trt_int8_enable=0`), `trt_cuda_graph_enable=0`, engine and timing caches on under `<cache>/tensorrt`, `trt_engine_hw_compatible=0`, `trt_builder_optimization_level=5`, `trt_max_workspace_size` from `trt_workspace_bytes` | `trt_options`, a raw overlay applied last                                      |
| WebGPU   | the plugin's own defaults                                                                                                                                                                                                                                         | `webgpu_prefer_nchw`, `webgpu_force_cpu_nodes`                                 |
| session  | parallel execution, memory planner on, no optimizer disabled                                                                                                                                                                                                      | `execution_mode`, `disable_mem_pattern`, `disabled_optimizers` (joined by `;`) |

Graph capture is off on both NVIDIA providers: with it on, TensorRT returns zeros from a session's second run, and CUDA fails the capture. TensorRT's workspace is sized from `sysinfo::cuda_info`'s first device. Precision is not a tuning concept: an app with FP16 and FP32 variants passes each its own tuning.

### Cache folders

`SessionRequest::cache` is the `CachePaths` for one model, from the caller. `CachePaths::under(cache_dir)` puts CoreML's compiled model in `<cache_dir>/coreml/`, and TensorRT's engines in `<cache_dir>/tensorrt/` and its timing cache in `<cache_dir>/tensorrt/timing/`. Each folder is created when a rung whose provider writes it is tried, so a CPU-only model leaves none. CoreML keys its cache by the model's path, so **key the engine folders by the model file's content** (its hash), or a re-downloaded model at the same path can run on a stale compile.

The timing cache need not be per model: kernel timings carry between graphs, so a build against a cache other models seeded is faster. Point every model's `timing` at one folder to share it:

```rust,no_run
# use std::path::Path;
use rust_sak::ai::CachePaths;

let paths = CachePaths {
    timing: Path::new("/cache/engines/.timing").to_path_buf(),
    ..CachePaths::under(Path::new("/cache/engines/scrfd"))
};
```

**Breaking in 26.11.0:** `SessionRequest::cache_dir: &Path` became `cache: &CachePaths`. Pass `&CachePaths::under(dir)` for the old behaviour.

### WebGPU

WebGPU ships inside the runtime's archive as a plugin (`libonnxruntime_providers_webgpu.dylib`, `.so`, or `onnxruntime_providers_webgpu.dll`). `load` registers it after the environment starts, when `sysinfo::is_webgpu_supported()` says the machine can run WebGPU (on Linux, a Vulkan GPU driver), and `Supported::detect` offers WebGPU when it offered a device. Sessions attach it through its first device. A machine without WebGPU, a missing plugin, a refused one or one with no device leaves WebGPU unoffered, with the reason logged; the load never fails because of it. Each Vulkan device is logged first, which is what makes a WebGPU failure on a two-GPU Linux machine readable.

### Usage

```rust,no_run
use std::path::Path;

use rust_sak::ai::{self, CachePaths, ExecutionProvider, SessionRequest, Supported, Tuning};

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
    cache: &CachePaths::under(Path::new("/cache/scrfd/0123456789abcdef")),
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

Because it runs before anything can probe the GPU (a CUDA probe starts threads), the folders are created whether or not the machine will ever install into them. On Linux and Windows a machine without an NVIDIA GPU therefore keeps empty `cuda`, `cudnn` and `tensorrt` version folders in the shared root. They are harmless: an empty folder has no marker, so it reads as not installed, and it is removed like any other version folder when a newer version is installed.

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
- The file installer's rename over an existing file, and its removal of a file another process has mapped, are tested on macOS only; on Windows the rename replaces in place (`MoveFileEx`), untested.
- **Only macOS is tested.** The GPU paths (the NVIDIA probe passing, and the CUDA, cuDNN and TensorRT installs) have never run on real hardware. The folder lock is tested on macOS, between tasks and against a process that exited holding it; on Windows it is `LockFileEx`, untested. Their logic is covered by pure tests against constructed probe results, and the platform-specific code is written to be correct by construction, but it is **untested on Linux and Windows**.
- On macOS, the ladder, the option maps and the search path are tested as pure functions, and CoreML, WebGPU and CPU sessions are built for real, on the ladder and one rung at a time. The TensorRT lock, the timing-cache discard and the out-of-memory classification are tested against a fake build and constructed records. **CUDA and TensorRT sessions, the Linux restart, the WSL2 driver folder and the Windows `PATH` step have never run**: they are written to be correct by construction, after Open Photo AI's code, and are untested.
- CoreML is offered on every macOS the runtime loads on, with no OS-version probe: the 1.30 runtime's macOS build already needs a newer macOS than CoreML's ML Program format (12). Were that wrong, the CoreML rung would fail and the ladder would move on.
